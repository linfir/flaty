#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UrlPath<'a>(&'a str);

impl<'a> UrlPath<'a> {
    pub fn new(src: &'a str) -> Option<Self> {
        if valid(src) {
            Some(UrlPath(src))
        } else {
            None
        }
    }

    pub fn path(self) -> &'a str {
        self.0
    }

    pub fn relative_path(self) -> &'a str {
        &self.0[1..]
    }

    pub fn has_final_slash(self) -> bool {
        self.0.ends_with('/')
    }

    pub fn last(self) -> &'a str {
        let i = self.0.rfind('/').expect("invalid state");
        &self.0[i + 1..]
    }

    pub fn extension(self) -> Option<&'a str> {
        let name = self.last();
        let i = name.rfind('.')?;
        Some(&name[i + 1..])
    }

    #[allow(unused)]
    pub fn parent(self) -> Option<UrlPath<'a>> {
        let path = self.0;
        let path = path.strip_suffix('/').unwrap_or(path);
        let i = self.0.rfind('/')?;
        if i > 0 {
            Some(UrlPath(&path[..i]))
        } else {
            None
        }
    }
}

fn valid(url: &str) -> bool {
    match url.strip_prefix('/') {
        Some("") => true,
        Some(url) => {
            let url = url.strip_suffix('/').unwrap_or(url);
            for c in url.split('/') {
                if c.is_empty() || c.starts_with('.') || c.starts_with('_') {
                    return false;
                }
            }
            true
        }
        None => false,
    }
}

#[test]
fn test_url_path() {
    assert!(UrlPath::new("foo").is_none());
    assert!(UrlPath::new("/foo/bar.quz/").unwrap().extension().is_none());
    assert_eq!(
        UrlPath::new("/foo/bar.quz").unwrap().extension().unwrap(),
        "quz"
    );
}

// #[test]
// fn test_to_components() {
//     assert_eq!(to_components(""), None);
//     assert_eq!(to_components("bla"), None);
//     assert_eq!(to_components("/"), Some(vec![]));
//     assert_eq!(to_components("/bla"), Some(vec!["bla"]));
//     assert_eq!(to_components("/bla/"), Some(vec!["bla"]));
//     assert_eq!(to_components("/bla/blo"), Some(vec!["bla", "blo"]));
//     assert_eq!(to_components("/bla/blo/"), Some(vec!["bla", "blo"]));
// }
