use std::{net::ToSocketAddrs, sync::Arc, time::Duration};

use anyhow::{anyhow, Context};
use axum::{
    body::Body,
    debug_handler,
    extract::State,
    http::{header, HeaderValue, Method, Request, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use dashmap::DashMap;
use tower::ServiceExt;
use tower_http::{services::ServeFile, set_header::SetResponseHeaderLayer};
use tracing::{info, warn};
use twox_hash::XxHash3_128;

use crate::web::{App, MyRequest};

mod cache;
mod markdown;
mod sass;
mod url;
mod web;

// Cached pages/styles and idle multi-mode sites are released after this long
// with no access; the next request recomputes/recreates them transparently.
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const SWEEP_PERIOD: Duration = Duration::from_secs(60);

#[derive(Parser)]
#[clap(version, about, long_about=None)]
struct Args {
    /// Address
    #[arg(short, long, default_value_t = {"localhost".into()})]
    bind: String,
    /// Port
    #[arg(short, long, default_value_t = 8080)]
    port: u16,
    /// Data directory
    #[arg(short, long, default_value_t = {".".into()})]
    directory: Utf8PathBuf,
    /// Serve each subdirectory as a site selected by the Host header
    #[arg(long)]
    multi: bool,
}

enum Sites {
    Single(Arc<App>),
    // Each request's Host header selects a same-named subdirectory of `root`.
    // Sites are discovered on demand and cached, so directories can be added
    // or removed without a restart.
    Multi {
        root: Utf8PathBuf,
        apps: DashMap<String, Arc<App>>,
    },
}

impl Sites {
    async fn resolve(&self, host: Option<&str>) -> Option<Arc<App>> {
        match self {
            Sites::Single(app) => Some(app.clone()),
            Sites::Multi { root, apps } => {
                let name = normalize_host(host?)?;
                if let Some(app) = apps.get(&name) {
                    app.touch();
                    return Some(app.clone());
                }
                if !is_site_name(&name) {
                    return None;
                }
                let dir = root.join(&name);
                if !tokio::fs::metadata(&dir)
                    .await
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
                {
                    return None;
                }
                Some(
                    apps.entry(name)
                        .or_insert_with(|| Arc::new(App::new(dir)))
                        .clone(),
                )
            }
        }
    }

    // Drop cache entries idle beyond `ttl`, and in multi mode idle sites too.
    fn sweep(&self, ttl: Duration) {
        match self {
            Sites::Single(app) => app.sweep(ttl),
            Sites::Multi { apps, .. } => {
                let now = tokio::time::Instant::now();
                apps.retain(|_, app| {
                    if app.idle_for(now) >= ttl {
                        false
                    } else {
                        app.sweep(ttl);
                        true
                    }
                });
            }
        }
    }
}

// A host maps to a same-named subdirectory; reject anything that is not a
// single safe path component (mirrors the startup filter, blocks traversal).
fn is_site_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.starts_with('_')
        && !name.contains('/')
        && !name.contains('\\')
}

// Lowercase, strip an optional `:port` and a trailing dot.
// IPv6 literals pass through unchanged (they never match a site).
fn normalize_host(host: &str) -> Option<String> {
    let host = host.trim();
    let host = match host.rsplit_once(':') {
        Some((h, port)) if !h.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    if !args.directory.is_dir() {
        return Err(anyhow!("data directory `{}` not found", args.directory));
    }

    let addr = (args.bind.as_str(), args.port)
        .to_socket_addrs()
        .context("invalid server address")?
        .next()
        .ok_or_else(|| anyhow!("cannot resolve server address"))?;

    let sites = if args.multi {
        // Warm the sites present at startup so their config is validated and
        // logged now; new directories are still picked up on demand later.
        let apps: DashMap<String, Arc<App>> = DashMap::new();
        for entry in args.directory.read_dir_utf8()? {
            let entry = entry?;
            let name = entry.file_name().to_ascii_lowercase();
            if !is_site_name(&name) || !entry.path().is_dir() {
                continue;
            }
            let app = Arc::new(App::new(entry.path().to_owned()));
            if let Err(err) = app.check_config().await {
                warn!("site `{name}`: {err:?} (serving 503 until `_config.toml` is valid)");
            }
            apps.insert(name, app);
        }
        let mut names: Vec<_> = apps.iter().map(|e| e.key().clone()).collect();
        names.sort();
        info!(
            "serving sites (discovered on demand): [{}]",
            names.join(", ")
        );
        Sites::Multi {
            root: args.directory,
            apps,
        }
    } else {
        let app = Arc::new(App::new(args.directory));
        if let Err(err) = app.check_config().await {
            warn!("{err:?} (serving 503 until `_config.toml` is valid)");
        }
        Sites::Single(app)
    };
    let app_state = Arc::new(sites);

    tokio::spawn({
        let sites = app_state.clone();
        async move {
            let mut ticker = tokio::time::interval(SWEEP_PERIOD);
            ticker.tick().await; // skip the immediate first tick
            loop {
                ticker.tick().await;
                sites.sweep(CACHE_TTL);
            }
        }
    });

    let app = Router::new()
        .fallback(handler)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .with_state(app_state);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let local_addr = listener.local_addr()?;

    info!("listening on http://{}/", local_addr);
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c()
                .await
                .expect("failed to install Ctrl+C handler")
        })
        .await?;
    Ok(())
}

#[debug_handler]
async fn handler(State(sites): State<Arc<Sites>>, req: Request<Body>) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let Some(app) = sites.resolve(host).await else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };

    let method = req.method();
    let uri_path = req.uri().path();

    // HEAD is handled as GET; hyper drops the response body.
    if method != Method::GET && method != Method::HEAD {
        return error_page(&app, StatusCode::NOT_FOUND, "404.html", String::new()).await;
    }

    let if_none_match = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    let authorization = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let request = MyRequest::GET {
        path: uri_path,
        authorization,
    };

    match web::web(app.clone(), request).await {
        Ok(r) => match r {
            web::MyResponse::Html(x) => {
                cached(x, "text/html; charset=utf-8", if_none_match.as_deref())
            }
            web::MyResponse::Css(x) => {
                cached(x, "text/css; charset=utf-8", if_none_match.as_deref())
            }
            web::MyResponse::File(f) => serve_file(&f, req).await,
            web::MyResponse::Redirect(url) => redirect(&url),
        },
        Err(e) => {
            use StatusCode as S;
            match e {
                web::MyError::NotFound => {
                    error_page(&app, S::NOT_FOUND, "404.html", String::new()).await
                }
                web::MyError::Unauthorized => unauthorized(),
                web::MyError::Unavailable => {
                    error_page(
                        &app,
                        S::SERVICE_UNAVAILABLE,
                        "503.html",
                        "Service unavailable".into(),
                    )
                    .await
                }
                web::MyError::InvalidPage => {
                    error_page(
                        &app,
                        S::INTERNAL_SERVER_ERROR,
                        "500.html",
                        "Invalid page".into(),
                    )
                    .await
                }
                web::MyError::InvalidScss => {
                    error_page(
                        &app,
                        S::INTERNAL_SERVER_ERROR,
                        "500.html",
                        "Invalid SCSS".into(),
                    )
                    .await
                }
                web::MyError::Internal(msg) => {
                    error_page(&app, S::INTERNAL_SERVER_ERROR, "500.html", msg).await
                }
                // Details are logged; do not echo file paths to clients.
                web::MyError::CannotRead => {
                    error_page(
                        &app,
                        S::INTERNAL_SERVER_ERROR,
                        "500.html",
                        "Internal error".into(),
                    )
                    .await
                }
            }
        }
    }
}

// Serve a custom error page from `_style/{file}` if present, else a plain body.
async fn error_page(app: &App, status: StatusCode, file: &str, fallback: String) -> Response {
    let path = app.root().join("_style").join(file);
    if let Ok(html) = tokio::fs::read_to_string(&path).await {
        return Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(html))
            .unwrap()
            .into_response();
    }
    (status, fallback).into_response()
}

// Serve a generated body with an ETag; answer 304 when it is unchanged.
fn cached(body: String, mime: &str, if_none_match: Option<&str>) -> Response {
    let etag = format!("\"{:032x}\"", XxHash3_128::oneshot(body.as_bytes()));

    if if_none_match == Some(etag.as_str()) {
        return Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, &etag)
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::empty())
            .unwrap()
            .into_response();
    }

    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ETAG, &etag)
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(body))
        .unwrap()
        .into_response()
}

fn unauthorized() -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, "Basic realm=\"flaty\"")
        .body(Body::from("Unauthorized"))
        .unwrap()
        .into_response()
}

fn redirect(url: &str) -> Response {
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header("Location", url)
        .body(Body::empty())
        .unwrap()
        .into_response()
}

async fn serve_file(path: &Utf8Path, req: Request<Body>) -> Response {
    ServeFile::new(path).oneshot(req).await.into_response()
}

#[test]
fn test_normalize_host() {
    assert_eq!(
        normalize_host("Example.COM").as_deref(),
        Some("example.com")
    );
    assert_eq!(
        normalize_host("example.com:8080").as_deref(),
        Some("example.com")
    );
    assert_eq!(
        normalize_host("example.com.").as_deref(),
        Some("example.com")
    );
    assert_eq!(
        normalize_host(" example.com ").as_deref(),
        Some("example.com")
    );
    assert_eq!(normalize_host("[::1]:8080").as_deref(), Some("[::1]:8080"));
    assert_eq!(normalize_host(""), None);
    assert_eq!(normalize_host(":8080"), None);
}
