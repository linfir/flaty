use std::{collections::HashSet, sync::Arc};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use tracing::{debug, error};

use crate::{
    cache::{Cache, CacheMap, Cacheable},
    markdown::Page,
    sass::Stylesheet,
    url::UrlPath,
};

// No dependency on the webserver

pub struct App {
    root: Utf8PathBuf,
    config: Cache<Arc<Config>>,
    pages: CacheMap<Arc<Page>>,
    templates: CacheMap<Arc<Template>>,
    styles: CacheMap<Arc<Stylesheet>>,
}

impl App {
    pub fn new(root: Utf8PathBuf) -> Self {
        App {
            config: Cache::new(root.join("_config.toml")),
            root,
            pages: CacheMap::default(),
            templates: CacheMap::default(),
            styles: CacheMap::default(),
        }
    }

    pub fn root(&self) -> &Utf8Path {
        &self.root
    }
}

// A page-layout template, cached as raw Handlebars source.
#[derive(Clone, Default)]
struct Template(String);

impl Cacheable for Template {
    fn compute(src: &str) -> anyhow::Result<Self> {
        Ok(Template(src.to_owned()))
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

impl Cacheable for Config {
    fn compute(src: &str) -> anyhow::Result<Self> {
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

#[derive(Debug)]
pub enum MyError {
    NotFound,
    InvalidPage,
    InvalidScss,
    CannotRead(Utf8PathBuf),
    Internal(String),
}

pub type MyResult = Result<MyResponse, MyError>;

pub async fn web(app: Arc<App>, req: MyRequest<'_>) -> MyResult {
    debug!("request: {:?}", req);
    let MyRequest::GET(url) = req;
    let url = UrlPath::new(url).ok_or(MyError::NotFound)?;

    // Reloads config
    let config = match app.config.load().await {
        Ok(cfg) => cfg,
        Err((cfg, err)) => {
            error!("{:?}", err);
            cfg
        }
    };

    if url.has_final_slash() {
        let html = render_page(&app, url).await?;
        return Ok(MyResponse::Html(html));
    }

    if let Some(name) = url.path().strip_prefix('/').filter(|p| !p.contains('/')) {
        if let Some(stem) = name.strip_suffix(".css") {
            if valid_asset_name(stem) {
                let scss_path = app.root.join(format!("_style/{stem}.scss"));
                // Don't create cache entries for missing stylesheets.
                if !tokio::fs::try_exists(&scss_path).await.unwrap_or(false) {
                    return Err(MyError::NotFound);
                }
                let css = match app.styles.load(&scss_path).await {
                    Ok(css) => css,
                    Err((_, err)) => {
                        error!("{:?}", err);
                        return Err(MyError::InvalidScss);
                    }
                };
                return Ok(MyResponse::Css(css.css().to_owned()));
            }
        }
    }

    match url.extension() {
        Some(ext) if config.extensions.contains(ext) => {
            return Ok(MyResponse::File(app.root.join(url.relative_path())));
        }
        _ => (),
    }

    if tokio::fs::try_exists(app.root.join(format!("{}/page.md", url.relative_path())))
        .await
        .unwrap_or(false)
    {
        return Ok(MyResponse::Redirect(format!("{}/", url.path())));
    }

    Err(MyError::NotFound)
}

async fn render_page(app: &App, url: UrlPath<'_>) -> Result<String, MyError> {
    let page_path = app.root.join(format!("{}page.md", url.relative_path()));
    // Don't create cache entries for missing pages.
    if !tokio::fs::try_exists(&page_path).await.unwrap_or(false) {
        return Err(MyError::NotFound);
    }
    let page = match app.pages.load(&page_path).await {
        Ok(page) => page,
        // The file exists (checked above), so a load failure is a bad page.
        Err((_, err)) => {
            error!("{:?}", err);
            return Err(MyError::InvalidPage);
        }
    };

    let template = page.template();
    if !valid_asset_name(template) {
        return Err(MyError::NotFound);
    }
    let tpl_path = app.root.join(format!("_style/{template}.html"));
    let tpl = match app.templates.load(&tpl_path).await {
        Ok(tpl) => tpl,
        Err((_, err)) => {
            error!("{:?}", err);
            return Err(MyError::CannotRead(tpl_path));
        }
    };

    let hbs = handlebars::Handlebars::new();
    let html = hbs
        .render_template(&tpl.0, page.fields())
        .map_err(|_| MyError::Internal("invalid template".into()))?;

    Ok(html)
}

// Frontmatter/URL supplied names must be bare identifiers, no path traversal.
fn valid_asset_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names() {
        assert!(valid_asset_name("default"));
        assert!(valid_asset_name("a-b_c"));
        assert!(!valid_asset_name(""));
        assert!(!valid_asset_name(".."));
        assert!(!valid_asset_name("a/b"));
        assert!(!valid_asset_name("a.b"));
    }

    // Runs against the checked-in `example_site` (cargo test CWD = crate root).
    async fn resp(path: &str) -> MyResult {
        let app = Arc::new(App::new("example_site".into()));
        web(app, MyRequest::GET(path)).await
    }

    #[tokio::test]
    async fn renders_home() {
        match resp("/").await.unwrap() {
            MyResponse::Html(h) => assert!(h.contains("Hello")),
            _ => panic!("expected html"),
        }
    }

    #[tokio::test]
    async fn renders_per_page_template() {
        match resp("/about/").await.unwrap() {
            MyResponse::Html(h) => assert!(h.contains("wide")),
            _ => panic!("expected html"),
        }
    }

    #[tokio::test]
    async fn redirects_without_slash() {
        match resp("/page1").await.unwrap() {
            MyResponse::Redirect(loc) => assert_eq!(loc, "/page1/"),
            _ => panic!("expected redirect"),
        }
    }

    #[tokio::test]
    async fn compiles_css() {
        match resp("/default.css").await.unwrap() {
            MyResponse::Css(c) => assert!(c.contains("color")),
            _ => panic!("expected css"),
        }
    }

    #[tokio::test]
    async fn missing_is_not_found() {
        assert!(matches!(resp("/nope/").await, Err(MyError::NotFound)));
    }
}
