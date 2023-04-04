use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Context;
use axum::{
    extract::State,
    http::{Method, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    Router,
};
use clap::Parser;
use markdown::markdown;
use tracing::log::info;

mod markdown;

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

struct App {
    root: PathBuf,
    counter: Mutex<u64>,
}

impl App {
    fn new(root: PathBuf) -> Self {
        App {
            root,
            counter: Mutex::new(0),
        }
    }

    fn tick(&self) -> u64 {
        let mut lock = self.counter.lock().unwrap();
        *lock += 1;
        *lock
    }
}

async fn real_handler(uri: Uri, method: Method, State(app): State<Arc<App>>) -> Response {
    match handler(uri, method, app).await {
        Ok(x) => x,
        Err(e) => match e {
            NotFound => (StatusCode::NOT_FOUND, ()).into_response(),
        },
    }
}

async fn handler(uri: Uri, method: Method, app: Arc<App>) -> MyResult {
    if method != Method::GET {
        return NotFound.into_http();
    }

    if uri.path() != "/" {
        return NotFound.into_http();
    }

    let path = app.root.join("page.md");
    let mut md = markdown(&path).map_err(|_| NotFound)?;
    md.insert("counter".into(), app.tick().to_string());

    let tpl = std::fs::read_to_string(app.root.join("_style/default.html")).unwrap();
    let hbs = handlebars::Handlebars::new();
    let html = hbs.render_template(&tpl, &md).unwrap();

    Html(html).into_http()
}

enum MyError {
    NotFound,
}
use MyError::*;

type MyResult = Result<Response, MyError>;

trait IntoHttp {
    fn into_http(self) -> MyResult;
}

impl<T: IntoResponse> IntoHttp for T {
    fn into_http(self) -> MyResult {
        Ok(self.into_response())
    }
}

impl IntoHttp for MyError {
    fn into_http(self) -> MyResult {
        Err(self)
    }
}
