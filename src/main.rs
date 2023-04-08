use std::{
    net::ToSocketAddrs,
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Context};
use axum::{
    body::{Body, Full},
    extract::State,
    http::{Method, Request, StatusCode},
    response::{Html, IntoResponse, Response},
    Router,
};
use clap::Parser;
use tower::ServiceExt;
use tower_http::services::ServeFile;
use tracing::log::info;
use web::{MyError, MyResponse};

use crate::web::{web, App, MyRequest};

mod cache;
mod markdown;
mod sass;
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
    directory: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let args = Args::parse();
    std::env::set_current_dir(&args.directory)
        .with_context(|| format!("Cannot chdir to `{}`", &args.directory))?;

    let mut addr_iter = (args.bind.as_str(), args.port)
        .to_socket_addrs()
        .context("invalid server address")?;
    let address = addr_iter
        .next()
        .ok_or_else(|| anyhow!("cannot resolve server address"))?;

    let app = Arc::new(Mutex::new(App::new()));
    let router = Router::new().fallback(real_handler).with_state(app);
    let server = axum::Server::bind(&address).serve(router.into_make_service());
    info!("Listening on http://{}/", server.local_addr());
    server.await?;

    Ok(())
}

async fn real_handler(State(app): State<Arc<Mutex<App>>>, request: Request<Body>) -> Response {
    let method = request.method();
    let uri_path = request.uri().path();

    if method != Method::GET {
        return (StatusCode::NOT_FOUND, ()).into_response();
    }

    match web(app, MyRequest::Get(uri_path)).await {
        Ok(r) => match r {
            MyResponse::Html(x) => Html(x).into_response(),
            MyResponse::Css(x) => Response::builder()
                .header("Content-Type", "text/css")
                .body(Full::from(x))
                .unwrap()
                .into_response(),
            MyResponse::File(f) => ServeFile::new(f).oneshot(request).await.into_response(),
        },
        Err(e) => match e {
            MyError::NotFound => (StatusCode::NOT_FOUND, ()).into_response(),
            MyError::InvalidScss => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Invalid SCSS").into_response()
            }
            MyError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response(),
            MyError::CannotRead(f) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot read file `{}`", f.display()),
            )
                .into_response(),
        },
    }
}
