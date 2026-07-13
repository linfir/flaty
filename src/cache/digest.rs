use std::{io, os::unix::prelude::MetadataExt, path::Path};

use tokio::{fs::File, io::AsyncReadExt};
use twox_hash::xxh3::hash128;

#[derive(Clone, Copy)]
pub struct Digest {
    mtime: i64,
    size: u64,
    hash: u128,
}

pub async fn load_file(
    path: &Path,
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
