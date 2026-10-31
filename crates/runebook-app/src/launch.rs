//! Optional vault path from the process arguments.
//!
//! The first value is the program name, as in [`std::env::args_os`].
//! No path keeps the last vault. One path opens that folder.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// Why the process arguments cannot choose a vault.
#[derive(Debug)]
pub(crate) enum LaunchError {
    /// Zero is fine. Two paths, an empty path, or a path that starts with `-` is not.
    Usage,
    /// The path is not Unicode text.
    NotUtf8,
    /// The current directory could not be read for a relative path.
    Absolute(io::Error),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage => formatter.write_str("runebook accepts one folder"),
            Self::NotUtf8 => formatter.write_str("the folder path must be utf-8"),
            Self::Absolute(error) => {
                write!(
                    formatter,
                    "could not make the folder path absolute: {error}"
                )
            }
        }
    }
}

/// The vault folder from `args`, or `None` when the user passed no path.
pub(crate) fn vault_path(
    args: impl IntoIterator<Item = impl Into<OsString>>,
) -> Result<Option<PathBuf>, LaunchError> {
    let mut args = args.into_iter();
    let _program = args.next();
    let Some(raw) = args.next() else {
        return Ok(None);
    };
    if args.next().is_some() {
        return Err(LaunchError::Usage);
    }
    let raw = raw.into();
    let Some(text) = raw.to_str() else {
        return Err(LaunchError::NotUtf8);
    };
    if text.is_empty() || text.starts_with('-') {
        return Err(LaunchError::Usage);
    }
    let path = std::path::absolute(Path::new(text)).map_err(LaunchError::Absolute)?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_path_keeps_the_saved_vault() {
        let path = vault_path(["runebook"]).unwrap();
        assert!(path.is_none());
    }

    #[test]
    fn one_absolute_path_is_that_folder() {
        let path = vault_path(["runebook", "/tmp/runebook-vault-arg"]).unwrap();
        assert_eq!(path.unwrap(), PathBuf::from("/tmp/runebook-vault-arg"));
    }

    #[test]
    fn one_relative_path_is_absolute() {
        let path = vault_path(["runebook", "runebook-vault-arg"]).unwrap();
        let path = path.unwrap();
        assert!(path.is_absolute());
        assert!(path.ends_with("runebook-vault-arg"));
    }

    #[test]
    fn two_paths_are_rejected() {
        let error = vault_path(["runebook", "/tmp/one", "/tmp/two"]).unwrap_err();
        assert!(matches!(error, LaunchError::Usage));
    }

    #[test]
    fn a_flag_and_an_empty_path_are_rejected() {
        assert!(matches!(
            vault_path(["runebook", "-h"]).unwrap_err(),
            LaunchError::Usage
        ));
        assert!(matches!(
            vault_path(["runebook", ""]).unwrap_err(),
            LaunchError::Usage
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_path_is_rejected() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![0xff]);
        let error = vault_path([OsString::from("runebook"), bad]).unwrap_err();
        assert!(matches!(error, LaunchError::NotUtf8));
    }
}
