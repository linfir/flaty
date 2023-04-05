use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
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

mod markdown;
mod sass;
mod web;

#[derive(Parser)]
struct Args {
    #[arg(default_value_t = {"127.0.0.1:8080".into()})]
    listen: String,
    #[arg(default_value_t = {".".into()})]
    root: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let args = Args::parse();
    let address: SocketAddr = args.listen.parse().context("invalid listen address")?;
    let root = PathBuf::from(args.root);
    let app = Arc::new(App::new(root));
    let router = Router::new().fallback(real_handler).with_state(app);

    info!("Listening on http://{}/", &address);
    axum::Server::bind(&address)
        .serve(router.into_make_service())
        .await?;

    Ok(())
}

async fn real_handler(State(app): State<Arc<App>>, request: Request<Body>) -> Response {
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
