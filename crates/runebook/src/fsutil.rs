use std::path::Path;

use crate::error::{Error, Result};

/// Write `contents` via a hidden temp file in the same directory, then rename.
pub(crate) fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Err(Error::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent"),
        });
    };
    std::fs::create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let file_name = path.file_name().unwrap_or_default();
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(file_name);
    tmp_name.push(".tmp");
    let tmp = parent.join(tmp_name);
    if let Err(source) = std::fs::write(&tmp, contents) {
        return Err(Error::Io { path: tmp, source });
    }
    if let Err(source) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}
