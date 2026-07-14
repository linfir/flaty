use std::{net::ToSocketAddrs, sync::Arc};

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
use tower::ServiceExt;
use tower_http::{services::ServeFile, set_header::SetResponseHeaderLayer};
use tracing::info;
use twox_hash::XxHash3_128;

use crate::web::{App, MyRequest};

mod cache;
mod markdown;
mod sass;
mod url;
mod web;

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

    let app_state = Arc::new(App::new(args.directory));

    app_state
        .check_config()
        .await
        .context("invalid or missing `_config.toml` (an empty file is fine)")?;

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
async fn handler(State(app): State<Arc<App>>, req: Request<Body>) -> Response {
    let method = req.method();
    let uri_path = req.uri().path();

    if method != Method::GET {
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
                web::MyError::CannotRead(f) => {
                    let msg = format!("Cannot read file `{}`", f);
                    error_page(&app, S::INTERNAL_SERVER_ERROR, "500.html", msg).await
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
