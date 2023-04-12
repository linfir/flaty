use std::{
    convert::Infallible,
    net::ToSocketAddrs,
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Context};
use clap::Parser;
use http_body_util::Full;
use hyper::{
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use tokio::net::TcpListener;
use tracing::info;

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

    let addr = (args.bind.as_str(), args.port)
        .to_socket_addrs()
        .context("invalid server address")?
        .next()
        .ok_or_else(|| anyhow!("cannot resolve server address"))?;

    let app = Arc::new(Mutex::new(App::new()));

    let listener = TcpListener::bind(addr).await?;
    info!("Listening on http://{}/", listener.local_addr()?);

    loop {
        let (stream, _) = listener.accept().await?;
        let app = app.clone();
        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(
                    stream,
                    service_fn(move |req| {
                        let app = app.clone();
                        async { Ok::<_, Infallible>(handler(req, app).await) }
                    }),
                )
                .await
            {
                println!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn handler(req: Request<Incoming>, app: Arc<Mutex<App>>) -> Response<MyBody> {
    let method = req.method();
    let uri_path = req.uri().path();

    if method != Method::GET {
        return not_found();
    }

    match web(app, MyRequest::Get(uri_path)).await {
        Ok(r) => match r {
            web::MyResponse::Html(x) => response_ok(x, "text/html"),
            web::MyResponse::Css(x) => response_ok(x, "text/css"),
            web::MyResponse::File(f) => serve_file(&f).await,
        },
        Err(e) => match e {
            web::MyError::NotFound => not_found(),
            web::MyError::InvalidScss => internal_error("Invalid SCSS"),
            web::MyError::Internal(msg) => internal_error(msg),
            web::MyError::CannotRead(f) => {
                internal_error(format!("Cannot read file `{}`", f.display()))
            }
        },
    }
}

type MyBody = Full<Bytes>;

fn mybody(bytes: impl Into<Bytes>) -> MyBody {
    Full::new(bytes.into())
}

fn response_ok(data: impl Into<Bytes>, mime: &str) -> Response<MyBody> {
    Response::builder()
        .header("Content-Type", mime)
        .body(mybody(data.into()))
        .unwrap()
}

fn not_found() -> Response<MyBody> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(mybody(Bytes::new()))
        .unwrap()
}

fn internal_error(msg: impl Into<Bytes>) -> Response<MyBody> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(mybody(msg.into()))
        .unwrap()
}

async fn serve_file(file: &Path) -> Response<MyBody> {
    // TODO: there is no streaming
    // this is bad for large files
    match tokio::fs::read_to_string(file).await {
        Ok(x) => {
            let mime = mime_guess::from_path(file).first_or_octet_stream();
            response_ok(x, mime.essence_str())
        }
        Err(_) => not_found(),
    }
}
