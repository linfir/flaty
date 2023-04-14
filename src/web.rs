use std::{collections::HashSet, sync::Arc};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use tracing::{debug, warn};

use crate::{
    cache::{Cachable, Cache},
    markdown::markdown,
    sass::sass,
    url::UrlPath,
};

// No dependency on the webserver

pub struct App {
    config: Cache<Arc<Config>>,
}

impl App {
    pub fn new() -> Self {
        App {
            config: Cache::new("_config.toml".into(), Arc::new(Config::default())),
        }
    }
}

#[derive(Debug, Default)]
struct Config {
    extensions: HashSet<String>,
}

#[derive(Deserialize)]
struct ConfigFile {
    extensions: Vec<String>,
}

impl Cachable for Config {
    fn recompute(src: &str) -> anyhow::Result<Self> {
        let cf: ConfigFile = toml::from_str(src)?;
        Ok(Config {
            extensions: cf.extensions.into_iter().collect(),
        })
    }
}

#[derive(Debug)]
#[allow(clippy::upper_case_acronyms)]
pub enum MyRequest<'a> {
    GET(&'a str),
}

pub enum MyResponse {
    Html(String),
    Css(String),
    File(Utf8PathBuf),
    Redirect(String),
}

pub enum MyError {
    NotFound,
    InvalidScss,
    CannotRead(Utf8PathBuf),
    Internal(String),
}

pub type MyResult = Result<MyResponse, MyError>;

pub async fn web(app: Arc<App>, req: MyRequest<'_>) -> MyResult {
    debug!("Request: {:?}", req);
    let MyRequest::GET(url) = req;
    let url = UrlPath::new(url).ok_or(MyError::NotFound)?;
    debug!(" - url {:?}", url);

    // Reloads config
    let config = match app.config.reload().await {
        Ok(cfg) => cfg,
        Err((cfg, err)) => {
            warn!("Error: {:?}", err);
            cfg
        }
    };

    if url.has_final_slash() {
        let html = render_page(url).await?;
        return Ok(MyResponse::Html(html));
    }

    if url.path() == "/default.css" {
        let doc = slurp("_style/default.scss").await?;
        let css = sass(doc).await?;
        return Ok(MyResponse::Css(css));
    }

    match url.extension() {
        Some(ext) if config.extensions.contains(ext) => {
            return Ok(MyResponse::File(url.relative_path().into()));
        }
        _ => (),
    }

    // TODO: instead of checking existence, read, process and cache
    if tokio::fs::try_exists(format!("{}/page.md", url.relative_path()))
        .await
        .unwrap_or(false)
    {
        return Ok(MyResponse::Redirect(format!("{}/", url.path())));
    }

    Err(MyError::NotFound)
}

async fn slurp(path: impl AsRef<Utf8Path>) -> Result<String, MyError> {
    let path = path.as_ref();
    tokio::fs::read_to_string(path)
        .await
        .map_err(|_| MyError::CannotRead(path.to_owned()))
}

async fn render_page(url: UrlPath<'_>) -> Result<String, MyError> {
    let doc = slurp(&format!("{}page.md", url.relative_path())).await?;
    let md = markdown(&doc).map_err(|_| MyError::NotFound)?;

    let tpl = slurp("_style/default.html").await?;
    let hbs = handlebars::Handlebars::new();
    let html = hbs
        .render_template(&tpl, &md)
        .map_err(|_| MyError::Internal("invalid template".into()))?;

    Ok(html)
}
