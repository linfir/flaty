mod base;
mod cacheable;
mod digest;

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Error;
use dashmap::DashMap;
use tokio::time::Instant;

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
        T: Cacheable + Clone + Send + 'static,
    {
        self.cache.load(&self.path).await
    }
}

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

    pub async fn load(&self, path: impl AsRef<Path>) -> Result<T, (T, Error)>
    where
        T: Cacheable + Clone + Default + Send + 'static,
    {
        let path = path.as_ref();
        self.map.entry(path.into()).or_default().load(path).await
    }

    // Drop entries not checked within `ttl`, releasing their cached value.
    pub fn sweep(&self, ttl: Duration) {
        let now = Instant::now();
        self.map.retain(|_, base| match base.last_check() {
            Some(t) => now.saturating_duration_since(t) < ttl,
            None => true, // keep in-progress/never-loaded entries
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default, Debug)]
    struct Upper(String);

    impl Cacheable for Upper {
        fn compute(src: &str) -> Result<Self, Error> {
            Ok(Upper(src.to_uppercase()))
        }
    }

    #[tokio::test]
    async fn sweep_drops_idle_entries() {
        let dir = std::env::temp_dir().join(format!("flaty-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.txt");
        std::fs::write(&path, "hi").unwrap();

        let cache: CacheMap<Upper> = CacheMap::default();
        assert_eq!(cache.load(&path).await.unwrap().0, "HI");
        assert_eq!(cache.map.len(), 1);

        // Zero TTL: any entry is stale and dropped.
        cache.sweep(Duration::ZERO);
        assert_eq!(cache.map.len(), 0);

        // A subsequent load recomputes correctly.
        std::fs::write(&path, "bye").unwrap();
        assert_eq!(cache.load(&path).await.unwrap().0, "BYE");
        assert_eq!(cache.map.len(), 1);

        // A large TTL keeps the fresh entry.
        cache.sweep(Duration::from_secs(3600));
        assert_eq!(cache.map.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }
}
