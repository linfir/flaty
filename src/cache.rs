use std::{io, os::unix::prelude::MetadataExt, path::Path, sync::Arc};

use anyhow::{Error, Result};
use parking_lot::Mutex;
use tokio::{fs::File, io::AsyncReadExt, time::Instant};
use tracing::debug;
use twox_hash::xxh3::hash128;

pub trait Cachable {
    fn compute(src: &str) -> Result<Self>
    where
        Self: Sized;
}

impl<T: Cachable> Cachable for Arc<T> {
    fn compute(src: &str) -> Result<Self> {
        T::compute(src).map(Arc::new)
    }
}

pub struct Cache<T> {
    mutex: Mutex<Cached<T>>,
}

#[derive(Default)]
struct Cached<T> {
    last_check: Option<Instant>,
    digest: Option<Digest>,
    value: T,
}

impl<T> Cache<T> {
    pub fn new() -> Self
    where
        T: Default,
    {
        Self {
            mutex: Mutex::new(Cached::default()),
        }
    }

    pub async fn load(&self, path: impl AsRef<Path>) -> Result<T, (T, Error)>
    where
        T: Cachable + Clone,
    {
        let path = path.as_ref();
        let (digest, value) = {
            let lock = self.mutex.lock();
            if let Some(last_access) = lock.last_check {
                if last_access.elapsed().as_secs() < 2 {
                    return Ok(lock.value.clone());
                }
            }
            (lock.digest, lock.value.clone())
        };

        match load_file(path, digest).await {
            Ok((digest, None)) => {
                let mut lock = self.mutex.lock();
                lock.last_check = Some(Instant::now());
                lock.digest = Some(digest);
                Ok(value)
            }
            Err(err) => {
                let mut lock = self.mutex.lock();
                lock.last_check = Some(Instant::now());
                let err = Error::from(err).context(format!("cannot read `{}`", path.display()));
                Err((value, err))
            }
            Ok((digest, Some(contents))) => {
                debug!("Reloading file `{}`", path.display());
                match T::compute(&contents) {
                    Ok(value) => {
                        let value2 = value.clone();
                        let mut lock = self.mutex.lock();
                        lock.last_check = Some(Instant::now());
                        lock.digest = Some(digest);
                        lock.value = value;
                        Ok(value2)
                    }
                    Err(err) => {
                        let mut lock = self.mutex.lock();
                        lock.last_check = Some(Instant::now());
                        lock.digest = Some(digest);
                        let err = err.context(format!("cannot process `{}`", path.display()));
                        Err((value, err))
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Digest {
    mtime: i64,
    size: u64,
    hash: u128,
}

async fn load_file(path: &Path, digest: Option<Digest>) -> io::Result<(Digest, Option<String>)> {
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
