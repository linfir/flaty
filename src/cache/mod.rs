mod base;
mod cacheable;
mod digest;

use std::path::{Path, PathBuf};

use anyhow::Error;
use dashmap::DashMap;

use self::base::CacheBase;
pub use self::cacheable::Cacheable;

pub struct Cache<T> {
    path: PathBuf,
    cache: CacheBase<T>,
}

impl<T> Cache<T> {
    pub fn new(path: impl Into<PathBuf>) -> Self
    where
        T: Default,
    {
        Self {
            path: path.into(),
            cache: CacheBase::default(),
        }
    }

    pub async fn load(&self) -> Result<T, (T, Error)>
    where
        T: Cacheable + Clone,
    {
        self.cache.load(&self.path).await
    }
}

#[allow(unused)]
pub struct CacheMap<T> {
    map: DashMap<PathBuf, CacheBase<T>>,
}

impl<T> Default for CacheMap<T> {
    fn default() -> Self {
        Self {
            map: DashMap::new(),
        }
    }
}

impl<T> CacheMap<T> {
    #[allow(unused)]
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(unused)]
    pub async fn load(&self, path: impl AsRef<Path>) -> Result<T, (T, Error)>
    where
        T: Cacheable + Clone + Default,
    {
        let path = path.as_ref();
        self.map.entry(path.into()).or_default().load(path).await
    }
}
