use std::{convert::Infallible, net::ToSocketAddrs, sync::Arc};

use anyhow::{anyhow, Context};
use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;
use hyper::{
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server, StatusCode,
};
use tokio::fs::File;
use tokio_util::codec::{BytesCodec, FramedRead};
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

    let app = Arc::new(App::new());

    let make_svc = make_service_fn(move |_conn| {
        let app = app.clone();
        async move {
            Ok::<_, Infallible>(service_fn(move |req| {
                let app = app.clone();
                async { Ok::<_, Infallible>(handler(req, app).await) }
            }))
        }
    });

    let server = Server::bind(&addr).serve(make_svc);
    info!("Listening on http://{}/", server.local_addr());

    server.await?;
    Ok(())
}

async fn handler(req: Request<Body>, app: Arc<App>) -> Response<Body> {
    let method = req.method();
    let uri_path = req.uri().path();

    if method != Method::GET {
        return not_found();
    }

    match web::web(app, MyRequest::GET(uri_path)).await {
        Ok(r) => match r {
            web::MyResponse::Html(x) => response_ok(x, "text/html"),
            web::MyResponse::Css(x) => response_ok(x, "text/css"),
            web::MyResponse::File(f) => serve_file(&f).await,
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

fn response_ok(data: impl Into<Body>, mime: &str) -> Response<Body> {
    Response::builder()
        .header("Content-Type", mime)
        .body(data.into())
        .unwrap()
}

fn not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .unwrap()
}

fn redirect(url: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header("Location", url)
        .body(Body::empty())
        .unwrap()
}

fn internal_error(msg: impl Into<Body>) -> Response<Body> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(msg.into())
        .unwrap()
}

async fn serve_file(path: &Utf8Path) -> Response<Body> {
    match File::open(path).await {
        Ok(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let stream = FramedRead::new(file, BytesCodec::new());
            response_ok(Body::wrap_stream(stream), mime.essence_str())
        }
        Err(_) => not_found(),
    }
}
