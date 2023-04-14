use std::{io, os::unix::prelude::MetadataExt, sync::Arc};

use anyhow::{Context, Error, Result};
use camino::{Utf8Path, Utf8PathBuf};
use parking_lot::Mutex;
use tokio::{fs::File, io::AsyncReadExt, time::Instant};
use tracing::debug;
use twox_hash::xxh3::hash128;

pub trait Cachable {
    fn recompute(src: &str) -> Result<Self>
    where
        Self: Sized;
}

impl<T: Cachable> Cachable for Arc<T> {
    fn recompute(src: &str) -> Result<Self> {
        T::recompute(src).map(Arc::new)
    }
}

pub struct Cache<T> {
    path: Utf8PathBuf,
    mutex: Mutex<Cached<T>>,
}

struct Cached<T> {
    last_check: Option<Instant>,
    digest: Option<Digest>,
    value: T,
}

impl<T> Cache<T> {
    pub fn new(path: Utf8PathBuf, value: T) -> Self {
        Cache {
            path,
            mutex: Mutex::new(Cached {
                last_check: None,
                digest: None,
                value,
            }),
        }
    }

    fn lock_and_update_last_check(&self) {
        let mut lock = self.mutex.lock();
        lock.last_check = Some(Instant::now());
    }

    pub async fn reload_with(&self, f: impl FnOnce(&str) -> Result<T>) -> Result<T, (T, Error)>
    where
        T: Clone,
    {
        let (digest, value) = {
            let lock = self.mutex.lock();
            if let Some(last_access) = lock.last_check {
                if last_access.elapsed().as_secs() < 2 {
                    return Ok(lock.value.clone());
                }
            }
            (lock.digest, lock.value.clone())
        };

        match load_file(&self.path, digest)
            .await
            .with_context(|| format!("Error reading file `{}`", self.path))
        {
            Ok((digest, None)) => {
                self.lock_and_update_last_check();
                let mut lock = self.mutex.lock();
                lock.last_check = Some(Instant::now());
                lock.digest = Some(digest);
                Ok(value)
            }
            Err(err) => {
                let mut lock = self.mutex.lock();
                lock.last_check = Some(Instant::now());
                Err((value, err))
            }
            Ok((digest, Some(contents))) => {
                debug!("Reloading file `{}`", self.path);
                match f(&contents) {
                    Ok(value) => {
                        let value2 = value.clone();
                        let mut lock = self.mutex.lock();
                        lock.last_check = Some(Instant::now());
                        lock.digest = Some(digest);
                        lock.value = value;
                        Ok(value2)
                    }
                    Err(err) => {
                        self.lock_and_update_last_check();
                        let mut lock = self.mutex.lock();
                        lock.last_check = Some(Instant::now());
                        lock.digest = Some(digest);
                        Err((value, err))
                    }
                }
            }
        }
    }

    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    pub async fn reload(&self) -> Result<T, (T, Error)>
    where
        T: Cachable + Clone,
    {
        self.reload_with(T::recompute).await
    }
}

#[derive(Clone, Copy)]
struct Digest {
    mtime: i64,
    size: u64,
    hash: u128,
}

async fn load_file(
    path: &Utf8Path,
    digest: Option<Digest>,
) -> io::Result<(Digest, Option<String>)> {
    let mut file = File::open(path).await?;
    let meta = file.metadata().await?;
    let size = meta.size();
    let mtime = meta.mtime();

    if let Some(digest) = digest {
        if size == digest.size && mtime == digest.mtime {
            return Ok((digest, None));
        }
    }

    let mut contents = String::new();
    file.read_to_string(&mut contents).await?;
    let hash = hash128(contents.as_bytes());
    let new_digest = Digest { mtime, size, hash };
    if let Some(digest) = digest {
        if hash == digest.hash {
            return Ok((new_digest, None));
        }
    }

    Ok((new_digest, Some(contents)))
}
