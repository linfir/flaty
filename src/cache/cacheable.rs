use std::sync::Arc;

use anyhow::Result;

pub trait Cacheable {
    fn compute(src: &str) -> Result<Self>
    where
        Self: Sized;
}

impl<T: Cacheable> Cacheable for Arc<T> {
    fn compute(src: &str) -> Result<Self> {
        T::compute(src).map(Arc::new)
    }
}
