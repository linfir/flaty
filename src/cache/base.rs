use std::path::Path;

use anyhow::{Error, Result};
use parking_lot::Mutex;
use tokio::time::Instant;
use tracing::debug;

use super::{
    digest::{load_file, Digest},
    Cacheable,
};

#[derive(Default)]
pub struct CacheBase<T> {
    mutex: Mutex<Cached<T>>,
}

#[derive(Default)]
struct Cached<T> {
    last_check: Option<Instant>,
    digest: Option<Digest>,
    value: T,
}

impl<T> CacheBase<T> {
    pub async fn load(&self, path: impl AsRef<Path>) -> Result<T, (T, Error)>
    where
        T: Cacheable + Clone + Send + 'static,
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
                // Offload compute (may be CPU-heavy, e.g. scss) off the runtime.
                match tokio::task::spawn_blocking(move || T::compute(&contents)).await {
                    Ok(Ok(new_value)) => {
                        let value2 = new_value.clone();
                        let mut lock = self.mutex.lock();
                        lock.last_check = Some(Instant::now());
                        lock.digest = Some(digest);
                        lock.value = new_value;
                        Ok(value2)
                    }
                    Ok(Err(err)) => {
                        let mut lock = self.mutex.lock();
                        lock.last_check = Some(Instant::now());
                        lock.digest = Some(digest);
                        let err = err.context(format!("cannot process `{}`", path.display()));
                        Err((value, err))
                    }
                    Err(join_err) => {
                        let mut lock = self.mutex.lock();
                        lock.last_check = Some(Instant::now());
                        let err = Error::from(join_err)
                            .context(format!("compute panicked for `{}`", path.display()));
                        Err((value, err))
                    }
                }
            }
        }
    }
}
