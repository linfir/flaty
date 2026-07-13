use std::{net::ToSocketAddrs, sync::Arc};

use anyhow::{anyhow, Context};
use axum::{
    body::Body,
    debug_handler,
    extract::State,
    http::{header, Method, Request, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use tower::ServiceExt;
use tower_http::services::ServeFile;
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

    std::env::set_current_dir(&args.directory)
        .with_context(|| format!("Cannot chdir to `{}`", args.directory))?;

    let addr = (args.bind.as_str(), args.port)
        .to_socket_addrs()
        .context("invalid server address")?
        .next()
        .ok_or_else(|| anyhow!("cannot resolve server address"))?;

    let app_state = Arc::new(App::new());

    let app = Router::new().fallback(handler).with_state(app_state);

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
        return not_found();
    }

    let if_none_match = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);

    match web::web(app, MyRequest::GET(uri_path)).await {
        Ok(r) => match r {
            web::MyResponse::Html(x) => cached(x, "text/html", if_none_match.as_deref()),
            web::MyResponse::Css(x) => cached(x, "text/css", if_none_match.as_deref()),
            web::MyResponse::File(f) => serve_file(&f, req).await,
            web::MyResponse::Redirect(url) => redirect(&url),
        },
        Err(e) => match e {
            web::MyError::NotFound => not_found(),
            web::MyError::InvalidScss => internal_error("Invalid SCSS"),
            web::MyError::Internal(msg) => internal_error(msg),
            web::MyError::CannotRead(f) => internal_error(format!("Cannot read file `{}`", f)),
        },
    }
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

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, ()).into_response()
}

fn redirect(url: &str) -> Response {
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header("Location", url)
        .body(Body::empty())
        .unwrap()
        .into_response()
}

fn internal_error(msg: impl Into<String>) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, msg.into()).into_response()
}

async fn serve_file(path: &Utf8Path, req: Request<Body>) -> Response {
    ServeFile::new(path).oneshot(req).await.into_response()
}
