//! Finder Quick Action that opens a selected folder in Runebook.
//!
//! Startup writes `~/Library/Services/Open in Runebook.workflow`.
//! Finder shows the item under Quick Actions for a folder.
//! A symlink at the workflow, or at either file inside it, is left in place.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::io::{self, ErrorKind};
use std::path::Path;
#[cfg(any(test, target_os = "macos"))]
use std::path::PathBuf;

const BUNDLE: &str = "Open in Runebook.workflow";

const INFO_PLIST: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
	<key>CFBundleName</key>
	<string>Open in Runebook</string>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>Open in Runebook</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.folder</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
";

/// Write the Quick Action for this macOS user.
///
/// A changed workflow is registered with `lsregister`. A missing tool is ignored.
#[cfg(target_os = "macos")]
pub(crate) fn install() {
    let exec = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("runebook: could not find the executable path: {error}");
            return;
        }
    };
    let Some(services) = services_dir() else {
        eprintln!("runebook: HOME is empty, so the Finder service was not installed");
        return;
    };
    let workflow = services.join(BUNDLE);
    match install_at(&services, &exec) {
        Ok(true) => refresh(&workflow),
        Ok(false) => {}
        Err(error) => eprintln!("runebook: could not write the Finder service: {error}"),
    }
}

#[cfg(target_os = "macos")]
fn services_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join("Library/Services"))
}

#[cfg(target_os = "macos")]
fn refresh(workflow: &Path) {
    let program = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    let _ = std::process::Command::new(program)
        .arg("-f")
        .arg(workflow)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Write the workflow under `services`.
///
/// Returns `true` when a file changed.
fn install_at(services: &Path, exec: &Path) -> io::Result<bool> {
    let script = shell_script(exec)?;
    let workflow = services.join(BUNDLE);
    refuse_symlink(&workflow)?;
    let info = workflow.join("Contents/Info.plist");
    let body = workflow.join("Contents/document.wflow");
    let mut changed = write_bytes(&info, INFO_PLIST.as_bytes())?;
    changed |= write_bytes(&body, document_wflow(&script).as_bytes())?;
    Ok(changed)
}

fn shell_script(exec: &Path) -> io::Result<String> {
    let exec = path_text(exec)?;
    let quoted = shell_quote(exec);
    Ok(format!(
        "for folder in \"$@\"; do\n  nohup {quoted} \"$folder\" >/dev/null 2>&1 &\ndone\n"
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

/// Quote `path` for a shell single-quoted string.
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

fn document_wflow(script: &str) -> String {
    let script = xml_escape(script);
    format!(
        "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
	<key>AMApplicationBuild</key>
	<string>523</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.path</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.path</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>{script}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/bash</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>5B1E6C0A-7F2D-4A11-9C3E-0A1B2C3D4E5F</string>
				<key>OutputUUID</key>
				<string>6C2F7D1B-803E-4B22-AD4F-1B2C3D4E5F60</string>
				<key>UUID</key>
				<string>7D308E2C-914F-4C33-BE50-2C3D4E5F6071</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
				<key>isViewVisible</key>
				<integer>0</integer>
			</dict>
			<key>isViewVisible</key>
			<integer>0</integer>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>serviceApplicationBundleIdentifier</key>
		<string>com.apple.finder</string>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<integer>0</integer>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn refuse_symlink(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(io::Error::other(format!(
            "refusing to follow a symlink: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_bytes(path: &Path, bytes: &[u8]) -> io::Result<bool> {
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
        Ok(_) if std::fs::read(path)? == bytes => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::write(path, bytes)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("runebook-finder-{}-{id}", std::process::id()));
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
    fn a_space_and_an_apostrophe_are_single_quoted() {
        assert_eq!(
            shell_quote("/Users/nick/bin/runebook"),
            "'/Users/nick/bin/runebook'"
        );
        assert_eq!(
            shell_quote("/Users/nick/My Notes/runebook"),
            "'/Users/nick/My Notes/runebook'"
        );
        assert_eq!(
            shell_quote("/Users/o'brien/runebook"),
            "'/Users/o'\\''brien/runebook'"
        );
    }

    #[test]
    fn a_newline_in_the_executable_is_rejected() {
        let error = shell_script(Path::new("/tmp/runebook\n")).unwrap_err();
        assert!(error.to_string().contains("newline"), "{error}");
    }

    #[test]
    fn install_writes_the_service() {
        let root = TempRoot(temp_root());
        let exec = Path::new("/home/nick/My Notes/runebook");
        assert!(install_at(&root.0, exec).unwrap());
        let info = root.0.join(BUNDLE).join("Contents/Info.plist");
        let body = root.0.join(BUNDLE).join("Contents/document.wflow");
        let plist = std::fs::read_to_string(&info).unwrap();
        assert!(plist.contains("<string>Open in Runebook</string>"));
        assert!(plist.contains("<string>public.folder</string>"));
        assert!(plist.contains("<string>com.apple.finder</string>"));
        let workflow = std::fs::read_to_string(&body).unwrap();
        assert!(workflow.contains("nohup '/home/nick/My Notes/runebook'"));
        assert!(workflow.contains("<integer>1</integer>"));
        assert!(workflow.contains("com.apple.Automator.servicesMenu"));
        assert!(!install_at(&root.0, exec).unwrap());
        assert_eq!(std::fs::read_to_string(&body).unwrap(), workflow);
    }

    #[test]
    fn a_new_executable_rewrites_the_script() {
        let root = TempRoot(temp_root());
        install_at(&root.0, Path::new("/usr/bin/runebook")).unwrap();
        assert!(install_at(&root.0, Path::new("/opt/runebook")).unwrap());
        let body = root.0.join(BUNDLE).join("Contents/document.wflow");
        let workflow = std::fs::read_to_string(&body).unwrap();
        assert!(workflow.contains("nohup '/opt/runebook'"));
        assert!(!workflow.contains("/usr/bin/runebook"));
    }

    #[cfg(unix)]
    #[test]
    fn install_refuses_a_symlink() {
        let root = TempRoot(temp_root());
        let workflow = root.0.join(BUNDLE);
        let elsewhere = root.0.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &workflow).unwrap();
        let error = install_at(&root.0, Path::new("/usr/bin/runebook")).unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");
        assert!(elsewhere.read_dir().unwrap().next().is_none());

        std::fs::remove_file(&workflow).unwrap();
        install_at(&root.0, Path::new("/usr/bin/runebook")).unwrap();
        let info = workflow.join("Contents/Info.plist");
        let held = root.0.join("held.plist");
        std::fs::write(&held, b"leave this file alone").unwrap();
        std::fs::remove_file(&info).unwrap();
        std::os::unix::fs::symlink(&held, &info).unwrap();
        let error = install_at(&root.0, Path::new("/usr/bin/runebook")).unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");
        assert_eq!(std::fs::read(&held).unwrap(), b"leave this file alone");
    }
}
