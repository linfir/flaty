use rsass::{compile_scss, output::Format};

use crate::web::MyError;

pub async fn sass(src: String) -> Result<String, MyError> {
    tokio::task::spawn_blocking(move || {
        let css =
            compile_scss(src.as_bytes(), Format::default()).map_err(|_| MyError::InvalidScss)?;
        String::from_utf8(css).map_err(|_| MyError::InvalidScss)
    })
    .await
    .map_err(|_| MyError::InvalidScss)?
}
