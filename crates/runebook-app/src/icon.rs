//! Application icon for the Runebook window.
//!
//! `assets/icon.png` is a 512px render of `assets/icon.svg`.
//! Regenerate that PNG when the drawing changes.
//!
//! On Linux, startup writes the PNG and a desktop file under `$XDG_DATA_HOME`.
//! When `XDG_DATA_HOME` is unset, that directory is `~/.local/share`.
//! The window id is [`APP_ID`]. The shell matches that id to the desktop file
//! and uses the PNG as the icon. GPUI also sends the same pixels to X11.

use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Window id. This matches `runebook.desktop`.
pub(crate) const APP_ID: &str = "runebook";

const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");

/// Pixels for [`gpui_kit::WindowOptions::icon`].
///
/// X11 uses this image. Other platforms ignore it.
pub(crate) fn rgba() -> Option<Arc<image::RgbaImage>> {
    match image::load_from_memory(ICON_PNG) {
        Ok(image) => Some(Arc::new(image.into_rgba8())),
        Err(error) => {
            eprintln!("runebook: could not read the application icon: {error}");
            None
        }
    }
}

/// Write the desktop icon for this Linux user.
///
/// A symlink at the icon file or the desktop file is left in place.
#[cfg(target_os = "linux")]
pub(crate) fn install() {
    let exec = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("runebook: could not find the executable path: {error}");
            return;
        }
    };
    let Some(root) = data_home() else {
        eprintln!("runebook: HOME is empty, so the application icon was not installed");
        return;
    };
    if let Err(error) = install_at(&root, &exec) {
        eprintln!("runebook: could not write the application icon: {error}");
    }
}

#[cfg(target_os = "linux")]
fn data_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".local/share"))
}

fn install_at(data_home: &Path, exec: &Path) -> io::Result<()> {
    let icon = data_home.join("runebook").join("icon.png");
    write_bytes(&icon, ICON_PNG)?;
    let desktop = data_home.join("applications").join("runebook.desktop");
    let text = desktop_entry(exec, &icon)?;
    write_bytes(&desktop, text.as_bytes())
}

fn desktop_entry(exec: &Path, icon: &Path) -> io::Result<String> {
    let exec = path_text(exec)?;
    let icon = path_text(icon)?;
    if !icon.starts_with('/') {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "icon path must be absolute",
        ));
    }
    Ok(format!(
        "\
[Desktop Entry]
Type=Application
Name=Runebook
Exec={quoted}
Icon={icon}
StartupWMClass={APP_ID}
Terminal=false
Categories=Office;
",
        quoted = quote_exec(exec),
    ))
}

fn path_text(path: &Path) -> io::Result<&str> {
    let text = path
        .to_str()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "path is not utf-8"))?;
    if text.contains('\n') || text.contains('\r') {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "path contains a newline",
        ));
    }
    Ok(text)
}

/// Quote an `Exec` value for a desktop file.
fn quote_exec(path: &str) -> String {
    let needs_quotes = path.bytes().any(|byte| {
        matches!(
            byte,
            b' ' | b'\t'
                | b'"'
                | b'\''
                | b'\\'
                | b'>'
                | b'<'
                | b'~'
                | b'|'
                | b'&'
                | b';'
                | b'$'
                | b'*'
                | b'?'
                | b'#'
                | b'('
                | b')'
                | b'`'
        )
    });
    if needs_quotes {
        let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        path.to_string()
    }
}

fn write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        match std::fs::symlink_metadata(parent) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::other(format!(
                    "refusing to follow a symlink: {}",
                    parent.display()
                )));
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(io::Error::other(format!(
                    "not a directory: {}",
                    parent.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {
                std::fs::create_dir_all(parent)?;
            }
            Err(error) => return Err(error),
        }
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(io::Error::other(format!(
                "refusing to follow a symlink: {}",
                path.display()
            )));
        }
        Ok(meta) if !meta.is_file() => {
            return Err(io::Error::other(format!("not a file: {}", path.display())));
        }
        Ok(_) if std::fs::read(path)? == bytes => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("runebook-icon-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    struct TempRoot(PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn png_decodes() {
        assert_eq!(&ICON_PNG[..8], b"\x89PNG\r\n\x1a\n");
        let image = rgba().expect("icon png decodes");
        assert_eq!(image.width(), 512);
        assert_eq!(image.height(), 512);
    }

    #[test]
    fn exec_path_with_a_space_is_quoted() {
        assert_eq!(
            quote_exec("/home/nick/.local/bin/runebook"),
            "/home/nick/.local/bin/runebook"
        );
        assert_eq!(
            quote_exec("/home/nick/My Notes/runebook"),
            "\"/home/nick/My Notes/runebook\""
        );
    }

    #[test]
    fn install_writes_the_desktop_icon() {
        let root = TempRoot(temp_root());
        let exec = Path::new("/home/nick/.local/bin/runebook");
        install_at(&root.0, exec).unwrap();
        let icon = root.0.join("runebook/icon.png");
        let desktop = root.0.join("applications/runebook.desktop");
        assert_eq!(std::fs::read(&icon).unwrap(), ICON_PNG);
        let text = std::fs::read_to_string(&desktop).unwrap();
        assert!(text.contains("Name=Runebook\n"));
        assert!(text.contains("Exec=/home/nick/.local/bin/runebook\n"));
        assert!(text.contains(&format!("Icon={}\n", icon.display())));
        assert!(text.contains("StartupWMClass=runebook\n"));
        install_at(&root.0, exec).unwrap();
        assert_eq!(std::fs::read(&icon).unwrap(), ICON_PNG);
    }

    #[cfg(unix)]
    #[test]
    fn install_refuses_a_symlink() {
        let root = TempRoot(temp_root());
        let dir = root.0.join("runebook");
        std::fs::create_dir_all(&dir).unwrap();
        let target = root.0.join("elsewhere.png");
        std::fs::write(&target, b"leave this file alone").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("icon.png")).unwrap();
        let error = install_at(&root.0, Path::new("/usr/bin/runebook")).unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");
        assert_eq!(std::fs::read(&target).unwrap(), b"leave this file alone");
    }
}
