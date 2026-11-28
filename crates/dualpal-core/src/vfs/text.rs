//! Reading and saving text files for the preview panel's quick editor.
//! Works on any [`FileSystem`], so remote files can be edited the same way.

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

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

/// Save text to an existing file. Writes a temporary sibling first and
/// renames it over the original, so a failed save never leaves a
/// half-written file. The original's permission bits are kept.
pub async fn write_text(fs: &dyn FileSystem, path: &str, text: &str) -> Result<()> {
    if fs.stat(path).await?.is_dir() {
        return Err(Error::Invalid(format!("not a file: {path}")));
    }
    super::write_file_atomic(fs, path, text.as_bytes()).await
}
