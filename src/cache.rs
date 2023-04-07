use std::{
    io::{self, Read},
    os::unix::prelude::MetadataExt,
    path::Path,
};

use tokio::{fs::File, io::AsyncReadExt};
use twox_hash::xxh3::hash128;

#[derive(Clone, Copy)]
pub struct Digest {
    mtime: i64,
    size: u64,
    hash: u128,
}

pub async fn try_reload(
    path: &Path,
    digest: Digest,
) -> Result<Option<(Digest, String)>, io::Error> {
    let mut file = File::open(path).await?;
    let meta = file.metadata().await?;

    let size = meta.size();
    let mtime = meta.mtime();
    if size == digest.size && mtime == digest.mtime {
        return Ok(None);
    }

    let mut contents = String::new();
    file.read_to_string(&mut contents).await?;
    let hash = hash128(contents.as_bytes());
    if hash == digest.hash {
        return Ok(None);
    }

    let digest = Digest { mtime, size, hash };

    Ok(Some((digest, contents)))
}
