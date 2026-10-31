//! This window's position on a Sway output.
//!
//! The socket is read only. A miss returns nothing and prints no path.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const TREE_LIMIT: usize = 8 * 1024 * 1024;
const WALK_DEPTH: usize = 64;
const WALK_NODES: usize = 10_000;

#[derive(Clone, Copy)]
pub(super) struct Origin {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) output_x: i32,
    pub(super) output_y: i32,
    pub(super) visible: bool,
    pub(super) focused: bool,
    pub(super) other_focused: bool,
}

impl Origin {
    /// The page should leave the screen.
    ///
    /// Another focused window covers it. A click that focuses no window does not.
    pub(super) fn covered(self) -> bool {
        !self.visible || (self.other_focused && !self.focused)
    }
}

pub(super) fn sway_origin(app_id: &str, width: i32, height: i32) -> Option<Origin> {
    let tree = read_tree()?;
    layout_origin(&tree, u64::from(std::process::id()), app_id, width, height)
}

/// Why the note page should leave the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cover {
    /// The window is on screen and focused, or nothing else is focused.
    Clear,
    /// Another window is focused. This window is still on its workspace.
    Focus,
    /// This window is not on the current workspace, or it is gone.
    Absent,
}

pub(super) fn cover_of(origin: Option<Origin>) -> Cover {
    match origin {
        None => Cover::Absent,
        Some(origin) if !origin.visible => Cover::Absent,
        Some(origin) if origin.covered() => Cover::Focus,
        Some(_) => Cover::Clear,
    }
}

/// `None` when the Sway tree cannot be read.
pub(super) fn sway_cover(app_id: &str, width: i32, height: i32) -> Option<Cover> {
    let tree = read_tree()?;
    Some(cover_of(layout_origin(
        &tree,
        u64::from(std::process::id()),
        app_id,
        width,
        height,
    )))
}

fn read_tree() -> Option<serde_json::Value> {
    let path = std::env::var_os("SWAYSOCK")?;
    let mut stream = UnixStream::connect(path).ok()?;
    let timeout = Some(Duration::from_millis(50));
    stream.set_read_timeout(timeout).ok()?;
    stream.set_write_timeout(timeout).ok()?;
    let body = request_tree(&mut stream)?;
    serde_json::from_slice(&body).ok()
}

fn request_tree(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut header = [0u8; 14];
    header[0..6].copy_from_slice(b"i3-ipc");
    header[10..14].copy_from_slice(&4u32.to_ne_bytes());
    stream.write_all(&header).ok()?;
    let mut response = [0u8; 14];
    stream.read_exact(&mut response).ok()?;
    if &response[0..6] != b"i3-ipc" {
        return None;
    }
    let len = usize::try_from(u32::from_ne_bytes(response[6..10].try_into().ok()?)).ok()?;
    if len > TREE_LIMIT {
        return None;
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).ok()?;
    Some(body)
}

fn layout_origin(
    tree: &serde_json::Value,
    pid: u64,
    app_id: &str,
    width: i32,
    height: i32,
) -> Option<Origin> {
    let mut walk = Walk {
        hits: Vec::new(),
        outputs: Vec::new(),
        nodes: 0,
        other_focused: false,
    };
    walk_node(tree, 0, pid, app_id, &mut walk);
    choose(&walk.hits, &walk.outputs, walk.other_focused, width, height)
}

struct Walk {
    hits: Vec<Hit>,
    outputs: Vec<Rect>,
    nodes: usize,
    other_focused: bool,
}

struct Hit {
    rect: Rect,
    focused: bool,
    visible: bool,
}

struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

fn walk_node(node: &serde_json::Value, depth: usize, pid: u64, app_id: &str, walk: &mut Walk) {
    if depth > WALK_DEPTH || walk.nodes >= WALK_NODES {
        return;
    }
    walk.nodes += 1;
    note_other_focus(node, pid, app_id, walk);
    if node.get("type").and_then(|value| value.as_str()) == Some("output") {
        let name = node
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if !name.starts_with("__")
            && let Some(rect) = read_rect(node)
        {
            walk.outputs.push(rect);
        }
    }
    if is_our_window(node, pid, app_id)
        && let Some(rect) = read_rect(node)
        && rect.w > 0
        && rect.h > 0
    {
        walk.hits.push(Hit {
            rect,
            focused: flag(node, "focused"),
            visible: flag(node, "visible"),
        });
    }
    for key in ["nodes", "floating_nodes"] {
        let Some(children) = node.get(key).and_then(|value| value.as_array()) else {
            continue;
        };
        for child in children {
            walk_node(child, depth + 1, pid, app_id, walk);
        }
    }
}

fn note_other_focus(node: &serde_json::Value, pid: u64, app_id: &str, walk: &mut Walk) {
    if walk.other_focused || !flag(node, "focused") || is_our_window(node, pid, app_id) {
        return;
    }
    let named = node
        .get("app_id")
        .and_then(|value| value.as_str())
        .is_some();
    let owned = node.get("pid").and_then(|value| value.as_u64()).is_some();
    if named || owned {
        walk.other_focused = true;
    }
}

fn is_our_window(node: &serde_json::Value, pid: u64, app_id: &str) -> bool {
    match node.get("pid").and_then(|value| value.as_u64()) {
        Some(found) => found == pid,
        None => node.get("app_id").and_then(|value| value.as_str()) == Some(app_id),
    }
}

fn flag(node: &serde_json::Value, key: &str) -> bool {
    node.get(key)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn read_rect(node: &serde_json::Value) -> Option<Rect> {
    let rect = node.get("rect")?;
    Some(Rect {
        x: json_i32(rect.get("x")?)?,
        y: json_i32(rect.get("y")?)?,
        w: json_i32(rect.get("width")?)?,
        h: json_i32(rect.get("height")?)?,
    })
}

fn json_i32(value: &serde_json::Value) -> Option<i32> {
    i32::try_from(value.as_i64()?).ok()
}

fn choose(
    hits: &[Hit],
    outputs: &[Rect],
    other_focused: bool,
    width: i32,
    height: i32,
) -> Option<Origin> {
    let hit = if let Some(hit) = hits.iter().find(|hit| hit.focused) {
        hit
    } else {
        hits.iter()
            .filter(|hit| hit.visible)
            .min_by_key(|hit| hit.rect.w.abs_diff(width) + hit.rect.h.abs_diff(height))
            .or_else(|| hits.first())?
    };
    let center_x = hit.rect.x.saturating_add(hit.rect.w / 2);
    let center_y = hit.rect.y.saturating_add(hit.rect.h / 2);
    let output = outputs
        .iter()
        .find(|output| contains(output, center_x, center_y));
    let (output_x, output_y) = output.map(|output| (output.x, output.y)).unwrap_or((0, 0));
    Some(Origin {
        x: hit.rect.x.saturating_sub(output_x),
        y: hit.rect.y.saturating_sub(output_y),
        output_x,
        output_y,
        visible: hit.visible,
        focused: hit.focused,
        other_focused,
    })
}

fn contains(outer: &Rect, x: i32, y: i32) -> bool {
    x >= outer.x
        && y >= outer.y
        && x < outer.x.saturating_add(outer.w)
        && y < outer.y.saturating_add(outer.h)
}

#[cfg(test)]
mod tests {
    use super::layout_origin;
    use serde_json::json;

    #[test]
    fn the_window_origin_is_relative_to_its_output() {
        let tree = json!({
            "type": "root",
            "nodes": [{
                "type": "output",
                "name": "DP-4",
                "rect": {"x": 1920, "y": 0, "width": 1920, "height": 1080},
                "nodes": [{
                    "type": "con",
                    "app_id": "runebook",
                    "pid": 42,
                    "focused": true,
                    "visible": true,
                    "rect": {"x": 2000, "y": 40, "width": 1200, "height": 800}
                }]
            }]
        });
        let origin = layout_origin(&tree, 42, "runebook", 1200, 800).expect("origin");
        assert_eq!((origin.x, origin.y, origin.visible), (80, 40, true));
        assert_eq!((origin.output_x, origin.output_y), (1920, 0));
        assert!(origin.focused);
        assert!(!origin.other_focused);
        assert!(!origin.covered());
    }

    #[test]
    fn another_focused_window_covers_the_page() {
        let tree = json!({
            "type": "root",
            "nodes": [
                {
                    "type": "con",
                    "pid": 42,
                    "app_id": "runebook",
                    "visible": true,
                    "focused": false,
                    "rect": {"x": 10, "y": 10, "width": 400, "height": 300}
                },
                {
                    "type": "con",
                    "pid": 99,
                    "app_id": "cursor",
                    "focused": true,
                    "visible": true,
                    "rect": {"x": 500, "y": 10, "width": 400, "height": 300}
                }
            ]
        });
        let origin = layout_origin(&tree, 42, "runebook", 400, 300).expect("origin");
        assert!(origin.visible);
        assert!(!origin.focused);
        assert!(origin.other_focused);
        assert!(origin.covered());
    }

    #[test]
    fn a_click_with_no_other_window_keeps_the_page() {
        let tree = json!({
            "type": "con",
            "pid": 42,
            "app_id": "runebook",
            "visible": true,
            "focused": false,
            "rect": {"x": 10, "y": 10, "width": 400, "height": 300}
        });
        let origin = layout_origin(&tree, 42, "runebook", 400, 300).expect("origin");
        assert!(!origin.focused);
        assert!(!origin.other_focused);
        assert!(!origin.covered());
    }

    #[test]
    fn a_window_on_another_workspace_is_absent() {
        let tree = json!({
            "type": "con",
            "pid": 42,
            "app_id": "runebook",
            "visible": false,
            "focused": false,
            "rect": {"x": 10, "y": 10, "width": 400, "height": 300}
        });
        let origin = layout_origin(&tree, 42, "runebook", 400, 300).expect("origin");
        assert_eq!(super::cover_of(Some(origin)), super::Cover::Absent);
    }

    #[test]
    fn another_focused_window_is_only_a_focus_cover() {
        let tree = json!({
            "type": "root",
            "nodes": [
                {
                    "type": "con",
                    "pid": 42,
                    "app_id": "runebook",
                    "visible": true,
                    "focused": false,
                    "rect": {"x": 10, "y": 10, "width": 400, "height": 300}
                },
                {
                    "type": "con",
                    "pid": 99,
                    "app_id": "cursor",
                    "focused": true,
                    "visible": true,
                    "rect": {"x": 500, "y": 10, "width": 400, "height": 300}
                }
            ]
        });
        let origin = layout_origin(&tree, 42, "runebook", 400, 300).expect("origin");
        assert_eq!(super::cover_of(Some(origin)), super::Cover::Focus);
    }

    #[test]
    fn a_missing_window_is_absent() {
        assert_eq!(super::cover_of(None), super::Cover::Absent);
    }

    #[test]
    fn another_process_is_ignored() {
        let tree = json!({
            "type": "con",
            "app_id": "runebook",
            "pid": 7,
            "focused": true,
            "visible": true,
            "rect": {"x": 1, "y": 2, "width": 100, "height": 100}
        });
        assert!(layout_origin(&tree, 42, "runebook", 100, 100).is_none());
    }

    #[test]
    fn a_window_without_a_pid_matches_the_app_id() {
        let tree = json!({
            "type": "output",
            "name": "DP-4",
            "rect": {"x": 0, "y": 0, "width": 800, "height": 600},
            "nodes": [{
                "type": "con",
                "app_id": "runebook",
                "visible": true,
                "rect": {"x": 12, "y": 8, "width": 400, "height": 300}
            }]
        });
        let origin = layout_origin(&tree, 42, "runebook", 400, 300).expect("origin");
        assert_eq!((origin.x, origin.y), (12, 8));
    }

    #[test]
    fn an_internal_output_is_ignored() {
        let tree = json!({
            "nodes": [
                {
                    "type": "output",
                    "name": "__i3",
                    "rect": {"x": 0, "y": 0, "width": 10000, "height": 10000}
                },
                {
                    "type": "output",
                    "name": "DP-4",
                    "rect": {"x": 500, "y": 0, "width": 1920, "height": 1080},
                    "nodes": [{
                        "type": "con",
                        "pid": 42,
                        "visible": true,
                        "focused": true,
                        "rect": {"x": 510, "y": 10, "width": 100, "height": 100}
                    }]
                }
            ]
        });
        let origin = layout_origin(&tree, 42, "runebook", 100, 100).expect("origin");
        assert_eq!((origin.x, origin.y), (10, 10));
        assert_eq!(origin.output_x, 500);
    }

    #[test]
    fn a_deep_tree_stops() {
        let mut node = json!({
            "type": "con",
            "pid": 42,
            "app_id": "runebook",
            "visible": true,
            "focused": true,
            "rect": {"x": 1, "y": 2, "width": 10, "height": 10}
        });
        for _ in 0..80 {
            node = json!({"type": "con", "nodes": [node]});
        }
        assert!(layout_origin(&node, 42, "runebook", 10, 10).is_none());
    }
}
