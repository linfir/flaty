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

    // Load the cached value; a missing file yields the default value instead
    // of an error. Only a present-but-invalid file is an error.
    pub async fn load_optional(&self) -> Result<T, (T, Error)>
    where
        T: Cacheable + Clone + Default + Send + 'static,
    {
        if !tokio::fs::try_exists(&self.path).await.unwrap_or(false) {
            return Ok(T::default());
        }
        self.cache.load(&self.path).await
    }
}

// Bound on entries per cache; oldest (by `last_check`) are evicted past it.
const MAX_ENTRIES: usize = 1024;

pub struct CacheMap<T> {
    map: DashMap<PathBuf, CacheBase<T>>,
    cap: usize,
}

impl<T> Default for CacheMap<T> {
    fn default() -> Self {
        Self::new(MAX_ENTRIES)
    }
}

impl<T> CacheMap<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            map: DashMap::new(),
            cap,
        }
    }

    pub async fn load(&self, path: impl AsRef<Path>) -> Result<T, (T, Error)>
    where
        T: Cacheable + Clone + Default + Send + 'static,
    {
        let path = path.as_ref();
        let result = self.map.entry(path.into()).or_default().load(path).await;
        self.enforce_cap();
        result
    }

    // Drop entries not checked within `ttl`, releasing their cached value.
    pub fn sweep(&self, ttl: Duration) {
        let now = Instant::now();
        self.map.retain(|_, base| match base.last_check() {
            Some(t) => now.saturating_duration_since(t) < ttl,
            None => true, // keep in-progress/never-loaded entries
        });
    }

    // Keep at most `cap` entries, evicting the least recently checked.
    fn enforce_cap(&self) {
        if self.map.len() <= self.cap {
            return;
        }
        // Collect keys + recency (iter holds shard locks, so remove after it ends).
        let mut entries: Vec<(PathBuf, Option<Instant>)> = self
            .map
            .iter()
            .map(|e| (e.key().clone(), e.value().last_check()))
            .collect();
        // Oldest first; None (in-flight/never-loaded) sorts last and is kept.
        entries.sort_by(|a, b| match (a.1, b.1) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        let excess = self.map.len().saturating_sub(self.cap);
        for (key, _) in entries.into_iter().take(excess) {
            self.map.remove(&key);
        }
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

    #[tokio::test]
    async fn cap_evicts_least_recent() {
        let dir = std::env::temp_dir().join(format!("flaty-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<_> = ["a", "b", "c"].iter().map(|n| dir.join(n)).collect();
        for p in &paths {
            std::fs::write(p, "x").unwrap();
        }

        let cache: CacheMap<Upper> = CacheMap::new(2);
        // Load a, b, c in order; small gaps keep `last_check` strictly ordered.
        for p in &paths {
            assert_eq!(cache.load(p).await.unwrap().0, "X");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Cap of 2: the oldest (a) is evicted, b and c remain.
        assert_eq!(cache.map.len(), 2);
        assert!(!cache.map.contains_key(&paths[0]));
        assert!(cache.map.contains_key(&paths[1]));
        assert!(cache.map.contains_key(&paths[2]));

        // Re-loading the evicted entry recomputes it.
        std::fs::write(&paths[0], "y").unwrap();
        assert_eq!(cache.load(&paths[0]).await.unwrap().0, "Y");
        assert_eq!(cache.map.len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }
}
