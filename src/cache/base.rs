use std::path::Path;

use anyhow::{Error, Result};
use parking_lot::Mutex;
use tokio::time::Instant;
use tracing::{debug, error};

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
    // Whether `value` reflects a successful compute of the current file. False
    // on the default value and once a read/compute fails, so callers get an
    // error instead of a stale value until the file is valid again.
    ok: bool,
    value: T,
}

impl<T> CacheBase<T> {
    pub async fn load(&self, path: impl AsRef<Path>) -> Result<T, (T, Error)>
    where
        T: Cacheable + Clone + Send + 'static,
    {
        let path = path.as_ref();
        let unavailable = || Error::msg(format!("`{}` unavailable", path.display()));

        let (digest, value) = {
            let lock = self.mutex.lock();
            if let Some(last_access) = lock.last_check {
                if last_access.elapsed().as_secs() < 2 {
                    return if lock.ok {
                        Ok(lock.value.clone())
                    } else {
                        Err((lock.value.clone(), unavailable()))
                    };
                }
            }
            (lock.digest, lock.value.clone())
        };

        match load_file(path, digest).await {
            Ok((digest, None)) => {
                // File unchanged since last check: reuse the last outcome.
                let mut lock = self.mutex.lock();
                lock.last_check = Some(Instant::now());
                lock.digest = Some(digest);
                if lock.ok {
                    Ok(value)
                } else {
                    Err((value, unavailable()))
                }
            }
            Err(err) => {
                let mut lock = self.mutex.lock();
                lock.last_check = Some(Instant::now());
                lock.ok = false;
                // Forget the digest so a reappearing file is fully recomputed.
                lock.digest = None;
                let err = Error::from(err).context(format!("cannot read `{}`", path.display()));
                error!("{err:?}");
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
                        lock.ok = true;
                        lock.value = new_value;
                        Ok(value2)
                    }
                    Ok(Err(err)) => {
                        let mut lock = self.mutex.lock();
                        lock.last_check = Some(Instant::now());
                        lock.digest = Some(digest);
                        lock.ok = false;
                        let err = err.context(format!("cannot process `{}`", path.display()));
                        error!("{err:?}");
                        Err((value, err))
                    }
                    Err(join_err) => {
                        let mut lock = self.mutex.lock();
                        lock.last_check = Some(Instant::now());
                        lock.ok = false;
                        let err = Error::from(join_err)
                            .context(format!("compute panicked for `{}`", path.display()));
                        error!("{err:?}");
                        Err((value, err))
                    }
                }
            }
        }
    }
}
