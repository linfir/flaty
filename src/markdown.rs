use std::collections::HashMap;

use anyhow::anyhow;
use pulldown_cmark::{html, Parser};
use toml::{Table, Value};

use crate::cache::Cacheable;

pub enum MarkdownError {
    InvalidHeader,
}

// A rendered page: frontmatter fields plus the "contents" HTML.
#[derive(Clone, Default)]
pub struct Page {
    fields: HashMap<String, String>,
}

impl Page {
    pub fn template(&self) -> &str {
        self.fields
            .get("template")
            .map(String::as_str)
            .unwrap_or("default")
    }

    pub fn fields(&self) -> &HashMap<String, String> {
        &self.fields
    }
}

impl Cacheable for Page {
    fn compute(src: &str) -> anyhow::Result<Self> {
        markdown(src).map_err(|_| anyhow!("invalid page header"))
    }
}

fn markdown(doc: &str) -> Result<Page, MarkdownError> {
    let (mut fields, doc) = parse_header(doc)?;
    let mut buf = String::new();
    html::push_html(&mut buf, Parser::new(doc));
    fields.insert("contents".into(), buf);
    Ok(Page { fields })
}

fn parse_header(src: &str) -> Result<(HashMap<String, String>, &str), MarkdownError> {
    match split(src) {
        Some((a, b)) => {
            let doc: Table = a.parse().map_err(|_| MarkdownError::InvalidHeader)?;
            let mut h = HashMap::new();
            for (k, v) in doc.into_iter() {
                match v {
                    Value::String(v) => {
                        h.insert(k, v);
                    }
                    _ => return Err(MarkdownError::InvalidHeader),
                }
            }
            Ok((h, b))
        }
        None => Ok((HashMap::new(), src)),
    }
}

fn split(data: &str) -> Option<(&str, &str)> {
    let data = data.trim_start().strip_prefix("---\n")?;
    let i = data.find("\n---\n")?;
    Some((&data[..i], &data[i + 5..]))
}
