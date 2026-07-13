use std::collections::HashMap;

use anyhow::anyhow;
use pulldown_cmark::{html, Parser};
use toml::{Table, Value};
use tracing::debug;

use crate::cache::Cacheable;

#[derive(Debug)]
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
                    Value::String(s) => {
                        h.insert(k, s);
                    }
                    Value::Integer(n) => {
                        h.insert(k, n.to_string());
                    }
                    Value::Float(n) => {
                        h.insert(k, n.to_string());
                    }
                    Value::Boolean(b) => {
                        h.insert(k, b.to_string());
                    }
                    Value::Datetime(d) => {
                        h.insert(k, d.to_string());
                    }
                    Value::Array(_) | Value::Table(_) => {
                        debug!("ignoring non-scalar frontmatter field `{}`", k);
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coerces_non_string_frontmatter() {
        let doc = "---\ntitle = \"T\"\ndraft = true\nn = 42\n---\nbody text";
        let page = markdown(doc).unwrap();
        assert_eq!(page.fields().get("title").map(String::as_str), Some("T"));
        assert_eq!(page.fields().get("draft").map(String::as_str), Some("true"));
        assert_eq!(page.fields().get("n").map(String::as_str), Some("42"));
        assert!(page.fields()["contents"].contains("body text"));
    }

    #[test]
    fn body_only_has_no_header_fields() {
        let page = markdown("just text").unwrap();
        assert!(page.fields()["contents"].contains("just text"));
        assert!(page.fields().get("title").is_none());
    }

    #[test]
    fn broken_header_errors() {
        assert!(markdown("---\ntitle = \"x\n---\nbody").is_err());
    }
}
