//! Screen recording through `ffmpeg`.
//!
//! Linux X11 uses `x11grab`. Linux Wayland copies frames from the compositor.
//! macOS records the full display with `avfoundation`.
//! Arguments are a list. The Groq API key is never passed to this process.
#![cfg_attr(target_os = "macos", allow(dead_code))]

#[cfg(target_os = "linux")]
mod wayland;

use crate::locale;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// What the next recording should grab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScreenTarget {
    /// The whole display. On Wayland this is the focused output.
    Display,
    /// One X11 window. The value is the window id.
    Window(u64),
    /// One rectangle on a named Wayland output, in output-local pixels.
    Region {
        output: String,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

/// One menu row that starts a recording.
pub(crate) struct RecordSource {
    pub title: String,
    pub target: ScreenTarget,
}

/// A window the user can record.
pub(crate) struct RecordWindow {
    pub id: u64,
    pub title: String,
}

pub(crate) fn window_list_error() -> &'static str {
    crate::locale::text("capture.window_list")
}

const MAX_WINDOWS: usize = 50;

pub(crate) struct ScreenCapture {
    child: Option<Child>,
    output: PathBuf,
    pub(crate) file_name: String,
    #[cfg(target_os = "linux")]
    grab: Option<wayland::Grab>,
}

impl ScreenCapture {
    pub(crate) fn start(
        output: PathBuf,
        file_name: String,
        target: ScreenTarget,
    ) -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        if wayland_session() {
            let grab = wayland::Grab::start(output.clone(), target)?;
            return Ok(Self {
                child: None,
                output,
                file_name,
                grab: Some(grab),
            });
        }
        #[cfg(target_os = "macos")]
        {
            if !matches!(target, ScreenTarget::Display) {
                return Err(locale::text("screen.macos").to_string());
            }
            let args = ffmpeg_args_macos(&output);
            return spawn_ffmpeg(output, file_name, &args);
        }
        #[cfg(not(target_os = "macos"))]
        {
            if matches!(target, ScreenTarget::Region { .. }) {
                return Err(locale::text("screen.window_display").to_string());
            }
            let display_var = std::env::var("DISPLAY").ok();
            let display = require_display(display_var.as_deref())?;
            let args = ffmpeg_args(display, &output, &target);
            spawn_ffmpeg(output, file_name, &args)
        }
    }

    pub(crate) fn stop(mut self) -> Result<(), String> {
        self.stop_process();
        let meta = std::fs::metadata(&self.output)
            .map_err(|_| locale::text("screen.no_video").to_string())?;
        if meta.len() == 0 {
            return Err(locale::text("screen.no_video").to_string());
        }
        Ok(())
    }
}

impl ScreenCapture {
    fn stop_process(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(grab) = self.grab.take() {
            grab.finish();
        }
        if let Some(mut child) = self.child.take() {
            finish(&mut child, Duration::from_secs(5));
        }
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        self.stop_process();
    }
}

pub(crate) fn require_display(display: Option<&str>) -> Result<&str, String> {
    match display.map(str::trim).filter(|value| !value.is_empty()) {
        Some(display) => Ok(display),
        None => Err(locale::text("screen.x11").to_string()),
    }
}

pub(crate) fn wayland_session() -> bool {
    std::env::var("WAYLAND_DISPLAY")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

pub(crate) fn ffmpeg_args(display: &str, output: &Path, target: &ScreenTarget) -> Vec<String> {
    let mut args = vec![
        "-y".to_string(),
        "-f".to_string(),
        "x11grab".to_string(),
        "-framerate".to_string(),
        "30".to_string(),
    ];
    if let ScreenTarget::Window(id) = target {
        args.push("-window_id".to_string());
        args.push(format!("0x{id:x}"));
    }
    args.extend([
        "-i".to_string(),
        display.to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-crf".to_string(),
        "23".to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-preset".to_string(),
        "ultrafast".to_string(),
        output.display().to_string(),
    ]);
    args
}

#[cfg(any(target_os = "macos", test))]
fn ffmpeg_args_macos(output: &Path) -> Vec<String> {
    vec![
        "-y".to_string(),
        "-f".to_string(),
        "avfoundation".to_string(),
        "-framerate".to_string(),
        "30".to_string(),
        "-pixel_format".to_string(),
        "uyvy422".to_string(),
        "-i".to_string(),
        "Capture screen 0:none".to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-crf".to_string(),
        "23".to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-preset".to_string(),
        "ultrafast".to_string(),
        output.display().to_string(),
    ]
}

fn spawn_ffmpeg(
    output: PathBuf,
    file_name: String,
    args: &[String],
) -> Result<ScreenCapture, String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(locale::text("screen.ffmpeg_missing").to_string());
        }
        Err(err) => {
            return Err(locale::format(
                "screen.ffmpeg_start",
                &[("err", &err.to_string())],
            ));
        }
    };
    Ok(ScreenCapture {
        child: Some(child),
        output,
        file_name,
        #[cfg(target_os = "linux")]
        grab: None,
    })
}

/// Read the open windows from `xprop` and `xwininfo`.
///
/// # Errors
///
/// Returns [`window_list_error`] when either command fails.
pub(crate) fn list_windows() -> Result<Vec<RecordWindow>, String> {
    let clients = command_stdout("xprop", &["-root", "_NET_CLIENT_LIST"])?;
    let tree = command_stdout("xwininfo", &["-root", "-tree"])?;
    Ok(windows_from_text(&clients, &tree))
}

/// Windows the record menu can offer.
///
/// On X11 the rows come from the window manager. On Wayland they come from
/// `swaymsg` when Sway is running. macOS returns no rows. The menu still
/// offers the full display.
///
/// # Errors
///
/// Returns [`window_list_error`] when the list cannot be read.
pub(crate) fn list_record_sources() -> Result<Vec<RecordSource>, String> {
    #[cfg(target_os = "macos")]
    {
        return Ok(Vec::new());
    }
    #[cfg(not(target_os = "macos"))]
    {
        if wayland_session() {
            let tree = command_stdout("swaymsg", &["-t", "get_tree"])?;
            return sources_from_sway(&tree);
        }
        Ok(list_windows()?
            .into_iter()
            .map(|window| RecordSource {
                title: window.title,
                target: ScreenTarget::Window(window.id),
            })
            .collect())
    }
}

#[cfg(target_os = "linux")]
pub(super) fn focused_output_name() -> Option<String> {
    let text = command_stdout("swaymsg", &["-t", "get_outputs"]).ok()?;
    focused_output(&text)
}

fn command_stdout(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| window_list_error().to_string())?;
    if !output.status.success() {
        return Err(window_list_error().to_string());
    }
    String::from_utf8(output.stdout).map_err(|_| window_list_error().to_string())
}

pub(crate) fn windows_from_text(clients: &str, tree: &str) -> Vec<RecordWindow> {
    let mut windows = Vec::new();
    for id in client_ids(clients) {
        if windows.len() == MAX_WINDOWS {
            break;
        }
        let Some(title) = title_for(tree, id) else {
            continue;
        };
        windows.push(RecordWindow { id, title });
    }
    windows
}

fn client_ids(text: &str) -> Vec<u64> {
    let mut ids = Vec::new();
    for id in hex_ids(text) {
        if id != 0 && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

fn title_for(tree: &str, id: u64) -> Option<String> {
    for line in tree.lines() {
        let Some(title) = title_on_line(line, id) else {
            continue;
        };
        if !title.trim().is_empty() {
            return Some(title);
        }
    }
    None
}

fn title_on_line(line: &str, id: u64) -> Option<String> {
    let after = hex_ids_with_end(line)
        .find(|(parsed, _)| *parsed == id)
        .map(|(_, end)| end)?;
    let rest = line[after..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].trim().to_string())
}

fn hex_ids(text: &str) -> impl Iterator<Item = u64> + '_ {
    hex_ids_with_end(text).map(|(id, _)| id)
}

fn hex_ids_with_end(text: &str) -> impl Iterator<Item = (u64, usize)> + '_ {
    let bytes = text.as_bytes();
    let mut index = 0;
    std::iter::from_fn(move || {
        while index + 2 < bytes.len() {
            let start = index;
            index += 1;
            if bytes[start] != b'0' || !matches!(bytes[start + 1], b'x' | b'X') {
                continue;
            }
            if start > 0 && bytes[start - 1].is_ascii_hexdigit() {
                continue;
            }
            let hex_start = start + 2;
            let mut end = hex_start;
            while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
                end += 1;
            }
            if end == hex_start {
                continue;
            }
            let Ok(id) = u64::from_str_radix(&text[hex_start..end], 16) else {
                continue;
            };
            index = end;
            return Some((id, end));
        }
        None
    })
}

pub(crate) fn sources_from_sway(tree: &str) -> Result<Vec<RecordSource>, String> {
    let root: serde_json::Value =
        serde_json::from_str(tree).map_err(|_| window_list_error().to_string())?;
    let mut sources = Vec::new();
    walk_sway(&root, None, &mut sources);
    Ok(sources)
}

pub(super) fn focused_output(text: &str) -> Option<String> {
    let outputs: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
    let focused = outputs.iter().find(|output| json_bool(output, "focused"));
    let chosen = focused.or_else(|| outputs.first())?;
    json_str(chosen, "name").map(str::to_string)
}

#[derive(Clone)]
struct OutputFrame {
    name: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn walk_sway(node: &serde_json::Value, output: Option<OutputFrame>, into: &mut Vec<RecordSource>) {
    if into.len() == MAX_WINDOWS {
        return;
    }
    let output = if json_str(node, "type") == Some("output") {
        output_frame(node).or(output)
    } else {
        output
    };
    if let Some(source) = source_from_node(node, output.as_ref()) {
        into.push(source);
    }
    for key in ["nodes", "floating_nodes"] {
        let Some(children) = node.get(key).and_then(serde_json::Value::as_array) else {
            continue;
        };
        for child in children {
            walk_sway(child, output.clone(), into);
        }
    }
}

fn source_from_node(
    node: &serde_json::Value,
    output: Option<&OutputFrame>,
) -> Option<RecordSource> {
    let leaf = json_array_empty(node, "nodes") && json_array_empty(node, "floating_nodes");
    let title = json_str(node, "name")?.trim().to_string();
    let hidden = node.get("visible").is_some() && !json_bool(node, "visible");
    if title.is_empty() || !leaf || hidden {
        return None;
    }
    if json_str(node, "app_id").is_none()
        && node
            .get("window")
            .and_then(serde_json::Value::as_i64)
            .is_none()
    {
        return None;
    }
    let output = output?;
    let rect = node.get("rect")?;
    let (x, y, width, height) = clip_region(output, rect)?;
    Some(RecordSource {
        title,
        target: ScreenTarget::Region {
            output: output.name.clone(),
            x,
            y,
            width,
            height,
        },
    })
}

fn output_frame(node: &serde_json::Value) -> Option<OutputFrame> {
    let name = json_str(node, "name")?.to_string();
    if name.is_empty() {
        return None;
    }
    let rect = node.get("rect")?;
    Some(OutputFrame {
        name,
        x: json_i32(rect, "x")?,
        y: json_i32(rect, "y")?,
        width: json_i32(rect, "width")?,
        height: json_i32(rect, "height")?,
    })
}

fn clip_region(output: &OutputFrame, window: &serde_json::Value) -> Option<(i32, i32, i32, i32)> {
    let x = json_i32(window, "x")?;
    let y = json_i32(window, "y")?;
    let width = json_i32(window, "width")?;
    let height = json_i32(window, "height")?;
    let x0 = x.max(output.x);
    let y0 = y.max(output.y);
    let x1 = x
        .saturating_add(width)
        .min(output.x.saturating_add(output.width));
    let y1 = y
        .saturating_add(height)
        .min(output.y.saturating_add(output.height));
    let width = x1 - x0;
    let height = y1 - y0;
    if width <= 0 || height <= 0 {
        return None;
    }
    Some((x0 - output.x, y0 - output.y, width, height))
}

fn json_str<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(serde_json::Value::as_str)
}

fn json_bool(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn json_i32(value: &serde_json::Value, key: &str) -> Option<i32> {
    i32::try_from(value.get(key)?.as_i64()?).ok()
}

fn json_array_empty(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .is_none_or(Vec::is_empty)
}

fn finish(child: &mut Child, timeout: Duration) {
    let pid = child.id().to_string();
    let _ = Command::new("kill").args(["-INT", &pid]).status();
    if wait_timeout(child, timeout).is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn wait_timeout(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if start.elapsed() >= timeout => return None,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    #[cfg(target_os = "linux")]
    use super::wayland;
    use super::{
        ScreenTarget, ffmpeg_args, ffmpeg_args_macos, focused_output, require_display,
        sources_from_sway, windows_from_text,
    };

    #[test]
    fn ffmpeg_args_use_x11grab_and_do_not_spawn() {
        let args = ffmpeg_args(":0", Path::new("/tmp/note.mp4"), &ScreenTarget::Display);
        let expected: Vec<String> = [
            "-y",
            "-f",
            "x11grab",
            "-framerate",
            "30",
            "-i",
            ":0",
            "-c:v",
            "libx264",
            "-crf",
            "23",
            "-pix_fmt",
            "yuv420p",
            "-preset",
            "ultrafast",
            "/tmp/note.mp4",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(args, expected);
    }

    #[test]
    fn empty_display_is_rejected() {
        assert!(require_display(None).is_err());
        assert!(require_display(Some("")).is_err());
        assert!(require_display(Some("  ")).is_err());
        assert_eq!(require_display(Some(":0")).unwrap(), ":0");
    }

    #[test]
    fn ffmpeg_args_macos_use_avfoundation_and_do_not_spawn() {
        let args = ffmpeg_args_macos(Path::new("/tmp/note.mp4"));
        let format = args.iter().position(|arg| arg == "-f").unwrap();
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        assert!(format < input);
        assert_eq!(args[format + 1], "avfoundation");
        assert_eq!(args[input + 1], "Capture screen 0:none");
        assert_eq!(args.last().map(String::as_str), Some("/tmp/note.mp4"));
        assert!(!args.iter().any(|arg| arg.contains("Firefox")));
    }

    #[test]
    fn window_id_is_an_input_argument() {
        let args = ffmpeg_args(
            ":0",
            Path::new("/tmp/note.mp4"),
            &ScreenTarget::Window(0x2a),
        );
        let window_id = args.iter().position(|arg| arg == "-window_id").unwrap();
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        assert!(window_id < input);
        assert_eq!(args[window_id + 1], "0x2a");
        assert!(!args.iter().any(|arg| arg.contains("Firefox")));
    }

    #[test]
    fn client_list_and_tree_yield_the_titled_window() {
        let clients = "_NET_CLIENT_LIST(WINDOW): window id # 0x2a, 0x3b\n";
        let tree = "\
     0x2a \"Firefox\": (\"firefox\" \"Firefox\")  800x600+10+10  +10+10
     0x3b \"\": (\"x\" \"x\")  10x10+0+0  +0+0
";
        let windows = windows_from_text(clients, tree);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, 0x2a);
        assert_eq!(windows[0].title, "Firefox");
    }

    #[test]
    fn sway_tree_yields_one_window_region() {
        let tree = r#"{
            "type": "root",
            "nodes": [{
                "type": "output",
                "name": "DP-1",
                "rect": {"x": 0, "y": 0, "width": 1000, "height": 800},
                "nodes": [{
                    "type": "workspace",
                    "nodes": [
                        {
                            "type": "con",
                            "name": "Firefox",
                            "app_id": "firefox",
                            "visible": true,
                            "rect": {"x": 10, "y": 20, "width": 800, "height": 600},
                            "nodes": []
                        },
                        {
                            "type": "con",
                            "name": "",
                            "app_id": "hidden-title",
                            "visible": true,
                            "rect": {"x": 0, "y": 0, "width": 10, "height": 10},
                            "nodes": []
                        }
                    ]
                }]
            }]
        }"#;
        let sources = sources_from_sway(tree).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].title, "Firefox");
        assert_eq!(
            sources[0].target,
            ScreenTarget::Region {
                output: "DP-1".to_string(),
                x: 10,
                y: 20,
                width: 800,
                height: 600,
            }
        );
        assert_eq!(
            focused_output(r#"[{"name":"DP-1","focused":true}]"#).as_deref(),
            Some("DP-1")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wayland_smoke_writes_a_short_video() {
        if std::env::var_os("RUNEBOOK_WAYLAND_SMOKE").is_none() {
            return;
        }
        let path = std::env::temp_dir().join("runebook-wayland-smoke.mp4");
        let _ = std::fs::remove_file(&path);
        let grab = wayland::Grab::start(path.clone(), ScreenTarget::Display).expect("start");
        std::thread::sleep(Duration::from_millis(500));
        grab.finish();
        let meta = std::fs::metadata(&path).expect("video");
        assert!(meta.len() > 0);
        let _ = std::fs::remove_file(&path);
    }
}
