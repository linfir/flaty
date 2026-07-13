use std::collections::HashMap;

use pulldown_cmark::{html, Parser};
use toml::{Table, Value};

pub enum MarkdownError {
    InvalidHeader,
}

pub fn markdown(doc: &str) -> Result<HashMap<String, String>, MarkdownError> {
    let (h, doc) = parse_header(doc)?;
    let mut h = h;
    let mut buf = String::new();
    html::push_html(&mut buf, Parser::new(doc));
    h.insert("contents".into(), buf);
    Ok(h)
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
