use std::{collections::HashMap, path::Path};

use pulldown_cmark::{html, Parser};
use yaml_rust::{Yaml, YamlLoader};

pub enum MarkdownError {
    CannotReadFile,
    InvalidHeader,
}

pub fn markdown(path: &Path) -> Result<HashMap<String, String>, MarkdownError> {
    let doc = std::fs::read_to_string(path).map_err(|_| MarkdownError::CannotReadFile)?;
    let (h, doc) = parse_header(&doc)?;
    let mut h = h;
    let mut buf = String::new();
    html::push_html(&mut buf, Parser::new(doc));
    h.insert("contents".into(), buf);
    Ok(h)
}

fn parse_header(src: &str) -> Result<(HashMap<String, String>, &str), MarkdownError> {
    match split(src) {
        Some((a, b)) => {
            let mut yaml_vec =
                YamlLoader::load_from_str(a).map_err(|_| MarkdownError::InvalidHeader)?;
            if yaml_vec.len() != 1 {
                return Err(MarkdownError::InvalidHeader);
            }
            let yaml = yaml_vec.pop().unwrap();

            let mut h = HashMap::new();
            match yaml {
                Yaml::Hash(yaml_top) => {
                    for (k, v) in yaml_top.into_iter() {
                        match (k, v) {
                            (Yaml::String(k), Yaml::String(v)) => {
                                h.insert(k, v);
                            }
                            _ => return Err(MarkdownError::InvalidHeader),
                        }
                    }
                }
                _ => return Err(MarkdownError::InvalidHeader),
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
