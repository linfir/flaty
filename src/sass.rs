use anyhow::anyhow;
use rsass::{compile_scss, output::Format};

use crate::cache::Cacheable;

#[derive(Clone, Default)]
pub struct Stylesheet(String);

impl Stylesheet {
    pub fn css(&self) -> &str {
        &self.0
    }
}

impl Cacheable for Stylesheet {
    fn compute(src: &str) -> anyhow::Result<Self> {
        let css = compile_scss(src.as_bytes(), Format::default())
            .map_err(|e| anyhow!("invalid scss: {e}"))?;
        let css = String::from_utf8(css).map_err(|e| anyhow!("invalid utf8 in css: {e}"))?;
        Ok(Stylesheet(css))
    }
}
