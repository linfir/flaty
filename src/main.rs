use std::{net::ToSocketAddrs, sync::Arc};

use anyhow::{anyhow, Context};
use axum::{
    body::Body,
    debug_handler,
    extract::State,
    http::{Method, Request, StatusCode},
    response::{IntoResponse, Response},
    Router, Server,
};
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tracing::info;

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
        .with_context(|| format!("Cannot chdir to `{}`", &args.directory))?;

    let addr = (args.bind.as_str(), args.port)
        .to_socket_addrs()
        .context("invalid server address")?
        .next()
        .ok_or_else(|| anyhow!("cannot resolve server address"))?;

    let app_state = Arc::new(App::new());

    let app = Router::new().fallback(handler).with_state(app_state);

    let server = Server::bind(&addr).serve(app.into_make_service());
    let local_addr = server.local_addr();
    let server = server.with_graceful_shutdown(async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler")
    });

    info!("listening on http://{}/", local_addr);
    server.await?;
    Ok(())
}

#[debug_handler]
async fn handler(State(app): State<Arc<App>>, req: Request<Body>) -> Response {
    let method = req.method();
    let uri_path = req.uri().path();

    if method != Method::GET {
        return not_found();
    }

    match web::web(app, MyRequest::GET(uri_path)).await {
        Ok(r) => match r {
            web::MyResponse::Html(x) => response_ok(x, "text/html"),
            web::MyResponse::Css(x) => response_ok(x, "text/css"),
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

fn response_ok(data: impl Into<Body>, mime: &str) -> Response {
    Response::builder()
        .header("Content-Type", mime)
        .body(data.into())
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
