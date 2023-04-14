use std::{
    io,
    os::unix::prelude::MetadataExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error, Result};
use parking_lot::Mutex;
use tokio::{fs::File, io::AsyncReadExt};
use tracing::trace;
use twox_hash::xxh3::hash64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Digest {
    mtime: i64,
    size: u64,
    hash: u64,
}

pub async fn load_file(
    path: impl AsRef<Path>,
    digest: Option<Digest>,
) -> io::Result<Option<(Digest, String)>> {
    let mut file = File::open(path).await?;
    let meta = file.metadata().await?;

    let size = meta.size();
    let mtime = meta.mtime();
    if let Some(digest) = digest {
        if size == digest.size && mtime == digest.mtime {
            return Ok(None);
        }
    }

    let mut contents = String::new();
    file.read_to_string(&mut contents).await?;
    let hash = hash64(contents.as_bytes());
    if let Some(digest) = digest {
        if hash == digest.hash {
            return Ok(None);
        }
    }

    let digest = Digest { mtime, size, hash };

    Ok(Some((digest, contents)))
}

trait Cachable {
    fn recompute(src: &str) -> Result<Self>
    where
        Self: Sized;
}

struct Cache<T> {
    path: PathBuf,
    inner: Mutex<(Option<Digest>, T)>,
}

impl<T> Cache<T> {
    pub fn new_with(path: PathBuf, inner: T) -> Self {
        Cache {
            path,
            inner: Mutex::new((None, inner)),
        }
    }

    pub async fn reload_with(&self, f: impl FnOnce(&str) -> Result<T>) -> Result<T, (T, Error)>
    where
        T: Clone,
    {
        let (old_digest, old_val) = {
            let lock = self.inner.lock();
            lock.clone()
        };

        match load_file(&self.path, old_digest)
            .await
            .with_context(|| format!("Error reading file `{}`", self.path.display()))
        {
            Ok(None) => Ok(old_val),
            Ok(Some((digest, contents))) => {
                trace!("Reloading file `{}`", self.path.display());
                match f(&contents) {
                    Ok(val) => {
                        let mut lock = self.inner.lock();
                        *lock = (Some(digest), val.clone());
                        Ok(val)
                    }
                    Err(err) => Err((old_val, err)),
                }
            }
            Err(err) => Err((old_val, err)),
        }
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.inner.lock().1.clone()
    }

    pub fn new(path: PathBuf) -> Self
    where
        T: Default,
    {
        Self::new_with(path, T::default())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn reload(&self) -> Result<T, (T, Error)>
    where
        T: Cachable + Clone,
    {
        self.reload_with(T::recompute).await
    }
}
