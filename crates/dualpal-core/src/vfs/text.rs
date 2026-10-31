//! Reading and saving text files for the preview panel's quick editor.
//! Works on any [`FileSystem`], so remote files can be edited the same way.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::FileSystem;
use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFile {
    pub text: String,
    /// The file is longer than the bytes read; it must not be saved back.
    pub truncated: bool,
    /// The content does not look like text.
    pub binary: bool,
}

/// Read up to `max_bytes` of a file as UTF-8 text.
pub async fn read_text(fs: &dyn FileSystem, path: &str, max_bytes: usize) -> Result<TextFile> {
    let mut reader = fs.open_read(path).await?;
    let mut buf = Vec::with_capacity(max_bytes.min(1 << 20));
    let mut limited = (&mut reader).take(max_bytes as u64 + 1);
    limited
        .read_to_end(&mut buf)
        .await
        .map_err(|e| Error::io(path, e))?;
    let truncated = buf.len() > max_bytes;
    buf.truncate(max_bytes);

    let sniff = &buf[..buf.len().min(8192)];
    let mut binary = sniff.contains(&0);
    let text = match std::str::from_utf8(&buf) {
        Ok(s) => s.to_string(),
        // Cut in the middle of a multi-byte character at the read limit.
        Err(e) if truncated && e.error_len().is_none() => {
            String::from_utf8_lossy(&buf[..e.valid_up_to()]).into_owned()
        }
        Err(_) => {
            binary = true;
            String::from_utf8_lossy(&buf).into_owned()
        }
    };
    Ok(TextFile {
        text: if binary { String::new() } else { text },
        truncated,
        binary,
    })
}

/// Save text to a file. Writes a temporary sibling first and renames it over
/// the original, so a failed save never leaves a half-written file. The
/// original's permission bits are kept.
pub async fn write_text(fs: &dyn FileSystem, path: &str, text: &str) -> Result<()> {
    let original = fs.stat(path).await?;
    if original.is_dir() {
        return Err(Error::Invalid(format!("not a file: {path}")));
    }
    let dir = fs
        .parent(path)
        .ok_or_else(|| Error::Invalid(format!("cannot save {path}")))?;
    let name = fs.file_name(path).unwrap_or_default();
    let tmp = fs.join(&dir, &format!(".{name}.dualpal-save"));

    let result = async {
        let mut w = fs.open_write(&tmp).await?;
        w.write_all(text.as_bytes())
            .await
            .map_err(|e| Error::io(&tmp, e))?;
        w.flush().await.map_err(|e| Error::io(&tmp, e))?;
        w.shutdown().await.map_err(|e| Error::io(&tmp, e))?;
        drop(w);
        if let Some(mode) = original.mode {
            let _ = fs.set_mode(&tmp, mode).await;
        }
        fs.rename(&tmp, path).await
    }
    .await;
    if result.is_err() {
        let _ = fs.remove_file(&tmp).await;
    }
    result
}
