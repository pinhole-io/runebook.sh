//! A VS Code color theme, mapped onto a GPUI Kit theme.
//!
//! The file uses `name`, `type`, `colors`, and `tokenColors`.
//! One `include` may name a `.json` file in the same folder.
//! `semanticTokenColors` is ignored.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use gpui_kit::SharedString;
use gpui_kit::component::highlighter::HighlightThemeStyle;
use gpui_kit::component::{Colorize, ThemeConfig, ThemeMode, try_parse_color};
use serde_json::{Map, Value};

/// Largest theme file the app will read.
pub(crate) const MAX_THEME_BYTES: u64 = 1024 * 1024;

/// Heading scale used when a theme omits `runebook.spacing`.
pub(crate) const DEFAULT_SPACING: NoteSpacing = NoteSpacing {
    paragraph_gap: 1.0,
    heading_before: 1.0,
    heading_1: 1.4,
    heading_2: 1.2,
};

const DEFAULT_FONT: &str = "Noto Sans";
const DEFAULT_MONO: &str = "Noto Sans Mono";
const DEFAULT_FONT_SIZE: f32 = 14.;
const DEFAULT_MONO_SIZE: f32 = 13.;
const DEFAULT_RADIUS: usize = 6;
const DEFAULT_RADIUS_LG: usize = 8;

/// Heading scale and paragraph gap from a theme file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NoteSpacing {
    pub paragraph_gap: f32,
    /// Extra space before a heading that follows text, a table, or another block.
    pub heading_before: f32,
    pub heading_1: f32,
    pub heading_2: f32,
}

/// One theme the window can apply.
#[derive(Clone, Debug)]
pub(crate) struct LoadedTheme {
    pub(crate) name: String,
    pub(crate) config: ThemeConfig,
    pub(crate) spacing: NoteSpacing,
}

/// Why a theme file was not used.
///
/// The text is the file name and a short reason. It does not include the file body.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ThemeError {
    file: String,
    reason: &'static str,
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.reason)
    }
}

impl ThemeError {
    fn new(file: &str, reason: &'static str) -> Self {
        Self {
            file: file.to_string(),
            reason,
        }
    }
}

/// Read one theme file. `include` is resolved in the parent folder.
pub(crate) fn read_theme_path(path: &Path) -> Result<LoadedTheme, ThemeError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("theme");
    let text = read_regular(path, file_name, "not a theme")?;
    parse_named(file_name, &text, path.parent())
}

/// Read every regular `.json` file in `dir`. A bad file is skipped.
///
/// A symlink is skipped. Two files with one name keep the later file name.
pub(crate) fn load_dir(dir: &Path) -> Vec<LoadedTheme> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        paths.push(path);
    }
    paths.sort();
    let mut themes: Vec<LoadedTheme> = Vec::new();
    for path in paths {
        match read_theme_path(&path) {
            Ok(theme) => {
                if let Some(slot) = themes.iter_mut().find(|item| item.name == theme.name) {
                    *slot = theme;
                } else {
                    themes.push(theme);
                }
            }
            Err(err) => eprintln!("runebook: skipped theme {err}"),
        }
    }
    themes
}

/// Parse theme JSON. `dir` is the folder used for `include`.
pub(crate) fn parse_named(
    file_name: &str,
    text: &str,
    dir: Option<&Path>,
) -> Result<LoadedTheme, ThemeError> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| ThemeError::new(file_name, "not a color theme"))?;
    let value = resolve_include(file_name, value, dir)?;
    map_theme(file_name, &value)
}

fn resolve_include(file_name: &str, value: Value, dir: Option<&Path>) -> Result<Value, ThemeError> {
    let Some(include) = value.get("include") else {
        return Ok(value);
    };
    let Some(include) = include.as_str() else {
        return Err(ThemeError::new(file_name, "bad include"));
    };
    let Some(dir) = dir else {
        return Err(ThemeError::new(file_name, "bad include"));
    };
    let base = read_include(dir, include, file_name)?;
    Ok(merge_theme(base, value))
}

fn read_include(dir: &Path, include: &str, theme_file: &str) -> Result<Value, ThemeError> {
    let Some(name) = include_file_name(include) else {
        return Err(ThemeError::new(theme_file, "bad include"));
    };
    if name == theme_file {
        return Err(ThemeError::new(theme_file, "bad include"));
    }
    let path = dir.join(name);
    if path.parent() != Some(dir) {
        return Err(ThemeError::new(theme_file, "bad include"));
    }
    let text = read_regular(&path, theme_file, "bad include")?;
    serde_json::from_str(&text).map_err(|_| ThemeError::new(theme_file, "bad include"))
}

/// A single `.json` file name. No slash, no `..`, and no second path segment.
fn include_file_name(value: &str) -> Option<&str> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value.contains("..")
        || !value.ends_with(".json")
    {
        return None;
    }
    if Path::new(value).components().count() != 1 {
        return None;
    }
    Some(value)
}

fn read_regular(path: &Path, file_name: &str, reason: &'static str) -> Result<String, ThemeError> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| ThemeError::new(file_name, reason))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(ThemeError::new(file_name, reason));
    }
    if meta.len() > MAX_THEME_BYTES {
        return Err(ThemeError::new(file_name, "too large"));
    }
    let mut file = File::open(path).map_err(|_| ThemeError::new(file_name, reason))?;
    let mut buf = Vec::new();
    file.by_ref()
        .take(MAX_THEME_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|_| ThemeError::new(file_name, reason))?;
    if buf.len() as u64 > MAX_THEME_BYTES {
        return Err(ThemeError::new(file_name, "too large"));
    }
    String::from_utf8(buf).map_err(|_| ThemeError::new(file_name, "not a color theme"))
}

fn merge_theme(mut base: Value, outer: Value) -> Value {
    let Some(outer_obj) = outer.as_object().cloned() else {
        return base;
    };
    if !base.is_object() {
        base = Value::Object(Map::new());
    }
    let Some(base_obj) = base.as_object_mut() else {
        return Value::Object(outer_obj);
    };
    merge_object(base_obj, &outer_obj, "colors");
    merge_object(base_obj, &outer_obj, "runebook");
    let mut tokens = Vec::new();
    if let Some(Value::Array(items)) = base_obj.get("tokenColors") {
        tokens.extend(items.clone());
    }
    if let Some(Value::Array(items)) = outer_obj.get("tokenColors") {
        tokens.extend(items.clone());
    }
    if !tokens.is_empty() {
        base_obj.insert("tokenColors".to_string(), Value::Array(tokens));
    }
    for key in ["name", "type"] {
        if let Some(value) = outer_obj.get(key) {
            base_obj.insert(key.to_string(), value.clone());
        }
    }
    base_obj.remove("include");
    base
}

fn merge_object(base: &mut Map<String, Value>, outer: &Map<String, Value>, key: &str) {
    let Some(outer_value) = outer.get(key) else {
        return;
    };
    let Some(outer_map) = outer_value.as_object() else {
        base.insert(key.to_string(), outer_value.clone());
        return;
    };
    match base.get_mut(key) {
        Some(Value::Object(base_map)) => {
            for (name, value) in outer_map {
                base_map.insert(name.clone(), value.clone());
            }
        }
        _ => {
            base.insert(key.to_string(), outer_value.clone());
        }
    }
}

fn map_theme(file_name: &str, value: &Value) -> Result<LoadedTheme, ThemeError> {
    let Some(root) = value.as_object() else {
        return Err(ThemeError::new(file_name, "not a color theme"));
    };
    let name = root
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ThemeError::new(file_name, "not a color theme"))?;
    let mode = match root.get("type").and_then(Value::as_str) {
        Some("dark") => ThemeMode::Dark,
        Some("light") => ThemeMode::Light,
        _ => return Err(ThemeError::new(file_name, "not a color theme")),
    };
    let colors = root.get("colors").and_then(Value::as_object);
    let empty = Map::new();
    let colors = colors.unwrap_or(&empty);
    let rules = token_rules(root.get("tokenColors"));
    let (font, spacing) = runebook_extras(root.get("runebook"));

    let mut config = ThemeConfig {
        name: SharedString::from(name.to_string()),
        mode,
        font_family: Some(SharedString::from(font.family)),
        font_size: Some(font.size),
        mono_font_family: Some(SharedString::from(font.mono)),
        mono_font_size: Some(font.mono_size),
        radius: Some(font.radius),
        radius_lg: Some(font.radius_lg),
        highlight: Some(highlight_style(colors, &rules)),
        ..ThemeConfig::default()
    };
    apply_colors(&mut config, colors);
    Ok(LoadedTheme {
        name: name.to_string(),
        config,
        spacing,
    })
}

struct FontExtras {
    family: String,
    size: f32,
    mono: String,
    mono_size: f32,
    radius: usize,
    radius_lg: usize,
}

fn runebook_extras(value: Option<&Value>) -> (FontExtras, NoteSpacing) {
    let obj = value.and_then(Value::as_object);
    let font = FontExtras {
        family: obj
            .and_then(|item| item.get("font.family"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_FONT.to_string()),
        size: obj
            .and_then(|item| item.get("font.size"))
            .and_then(Value::as_f64)
            .map(|size| size as f32)
            .unwrap_or(DEFAULT_FONT_SIZE),
        mono: obj
            .and_then(|item| item.get("mono_font.family"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_MONO.to_string()),
        mono_size: obj
            .and_then(|item| item.get("mono_font.size"))
            .and_then(Value::as_f64)
            .map(|size| size as f32)
            .unwrap_or(DEFAULT_MONO_SIZE),
        radius: obj
            .and_then(|item| item.get("radius"))
            .and_then(Value::as_u64)
            .and_then(|size| usize::try_from(size).ok())
            .unwrap_or(DEFAULT_RADIUS),
        radius_lg: obj
            .and_then(|item| item.get("radius.lg"))
            .and_then(Value::as_u64)
            .and_then(|size| usize::try_from(size).ok())
            .unwrap_or(DEFAULT_RADIUS_LG),
    };
    let spacing_value = obj.and_then(|item| item.get("spacing"));
    let spacing = NoteSpacing {
        paragraph_gap: spacing_number(
            spacing_value,
            "paragraph_gap",
            DEFAULT_SPACING.paragraph_gap,
        ),
        heading_before: spacing_number(
            spacing_value,
            "heading_before",
            DEFAULT_SPACING.heading_before,
        ),
        heading_1: spacing_number(spacing_value, "heading.1", DEFAULT_SPACING.heading_1),
        heading_2: spacing_number(spacing_value, "heading.2", DEFAULT_SPACING.heading_2),
    };
    (font, spacing)
}

fn spacing_number(spacing: Option<&Value>, key: &str, fallback: f32) -> f32 {
    spacing
        .and_then(|item| item.get(key))
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .unwrap_or(fallback)
}

fn apply_colors(config: &mut ThemeConfig, colors: &Map<String, Value>) {
    let slots = &mut config.colors;
    // The dock paints `tab_bar` behind every split. That surface is the note.
    copy(
        colors,
        "editor.background",
        &mut [&mut slots.background, &mut slots.tab_bar],
    );
    copy(colors, "editor.foreground", &mut [&mut slots.foreground]);
    copy(colors, "editorCursor.foreground", &mut [&mut slots.caret]);
    copy(
        colors,
        "editor.selectionBackground",
        &mut [&mut slots.selection],
    );
    copy(colors, "sideBar.background", &mut [&mut slots.sidebar]);
    copy(
        colors,
        "sideBar.foreground",
        &mut [&mut slots.sidebar_foreground],
    );
    copy(colors, "sideBar.border", &mut [&mut slots.sidebar_border]);
    copy(
        colors,
        "list.activeSelectionBackground",
        &mut [
            &mut slots.list_active,
            &mut slots.table_active,
            &mut slots.sidebar_accent,
        ],
    );
    copy(
        colors,
        "list.activeSelectionForeground",
        &mut [&mut slots.sidebar_accent_foreground],
    );
    copy(
        colors,
        "list.hoverBackground",
        &mut [&mut slots.list_hover, &mut slots.table_hover],
    );
    copy(
        colors,
        "list.inactiveSelectionBackground",
        &mut [&mut slots.list_even, &mut slots.table_even],
    );
    copy(
        colors,
        "titleBar.activeBackground",
        &mut [&mut slots.title_bar],
    );
    copy(
        colors,
        "titleBar.border",
        &mut [&mut slots.title_bar_border],
    );
    copy(colors, "statusBar.background", &mut [&mut slots.status_bar]);
    copy(
        colors,
        "statusBar.border",
        &mut [&mut slots.status_bar_border],
    );
    copy(colors, "tab.activeBackground", &mut [&mut slots.tab_active]);
    copy(colors, "tab.inactiveBackground", &mut [&mut slots.tab]);
    copy(
        colors,
        "tab.activeForeground",
        &mut [&mut slots.tab_active_foreground],
    );
    copy(
        colors,
        "tab.inactiveForeground",
        &mut [&mut slots.tab_foreground],
    );
    copy(
        colors,
        "editorGroupHeader.tabsBackground",
        &mut [&mut slots.tab_bar_segmented],
    );
    copy(
        colors,
        "button.background",
        &mut [&mut slots.primary, &mut slots.button_primary],
    );
    copy(
        colors,
        "button.foreground",
        &mut [
            &mut slots.primary_foreground,
            &mut slots.button_primary_foreground,
        ],
    );
    copy(
        colors,
        "button.hoverBackground",
        &mut [&mut slots.primary_hover, &mut slots.button_primary_hover],
    );
    copy(
        colors,
        "button.secondaryBackground",
        &mut [&mut slots.secondary, &mut slots.button_secondary],
    );
    copy(
        colors,
        "button.secondaryForeground",
        &mut [
            &mut slots.secondary_foreground,
            &mut slots.button_secondary_foreground,
        ],
    );
    copy(
        colors,
        "button.secondaryHoverBackground",
        &mut [
            &mut slots.secondary_hover,
            &mut slots.button_secondary_hover,
        ],
    );
    copy_first(
        colors,
        &["focusBorder", "contrastBorder", "list.focusOutline"],
        &mut [&mut slots.ring],
    );
    copy_first(
        colors,
        &["panel.border", "widget.border", "editorGroup.border"],
        &mut [&mut slots.border, &mut slots.window_border],
    );
    copy_first(
        colors,
        &["input.border", "checkbox.border", "dropdown.border"],
        &mut [&mut slots.input],
    );
    copy_first(
        colors,
        &[
            "editorWidget.background",
            "dropdown.background",
            "menu.background",
        ],
        &mut [&mut slots.popover],
    );
    copy_first(
        colors,
        &[
            "editorWidget.foreground",
            "dropdown.foreground",
            "menu.foreground",
        ],
        &mut [&mut slots.popover_foreground],
    );
    copy(colors, "textLink.foreground", &mut [&mut slots.link]);
    copy(
        colors,
        "textLink.activeForeground",
        &mut [&mut slots.link_active],
    );
    copy(
        colors,
        "descriptionForeground",
        &mut [&mut slots.muted_foreground],
    );
    // The window paints `danger` as text and icons. The error fill is too dark for that.
    copy_first(
        colors,
        &["editorError.foreground", "errorForeground"],
        &mut [&mut slots.danger],
    );
    copy_first(
        colors,
        &["editor.background", "button.foreground"],
        &mut [&mut slots.danger_foreground],
    );
    copy_first(
        colors,
        &["editorWarning.foreground"],
        &mut [&mut slots.warning],
    );
    copy_first(
        colors,
        &["editorInfo.foreground", "textLink.foreground"],
        &mut [&mut slots.info],
    );
    copy_first(
        colors,
        &[
            "gitDecoration.addedResourceForeground",
            "terminal.ansiGreen",
        ],
        &mut [&mut slots.success],
    );
    copy_first(
        colors,
        &["menu.selectionBackground", "list.activeSelectionBackground"],
        &mut [&mut slots.accent],
    );
    copy_first(
        colors,
        &[
            "menu.selectionForeground",
            "list.activeSelectionForeground",
            "editor.foreground",
        ],
        &mut [&mut slots.accent_foreground],
    );
    copy_first(
        colors,
        &["input.background", "list.hoverBackground"],
        &mut [&mut slots.muted],
    );
    copy_first(
        colors,
        &[
            "input.background",
            "checkbox.background",
            "list.hoverBackground",
        ],
        &mut [&mut slots.switch],
    );
    copy(colors, "editor.foreground", &mut [&mut slots.switch_thumb]);
    copy_first(
        colors,
        &["textLink.activeForeground", "textLink.foreground"],
        &mut [&mut slots.link_hover],
    );
    copy(
        colors,
        "scrollbarSlider.background",
        &mut [&mut slots.scrollbar_thumb],
    );
    copy(
        colors,
        "scrollbarSlider.hoverBackground",
        &mut [&mut slots.scrollbar_thumb_hover],
    );
}

fn copy(colors: &Map<String, Value>, key: &str, slots: &mut [&mut Option<SharedString>]) {
    copy_first(colors, &[key], slots);
}

/// Use the first color id that parses. Skip a slot that already has a color.
fn copy_first(colors: &Map<String, Value>, keys: &[&str], slots: &mut [&mut Option<SharedString>]) {
    let Some(raw) = keys.iter().find_map(|key| {
        colors
            .get(*key)
            .and_then(Value::as_str)
            .filter(|raw| try_parse_color(raw).is_ok())
    }) else {
        return;
    };
    let value = SharedString::from(raw.to_string());
    for slot in slots {
        if slot.is_none() {
            **slot = Some(value.clone());
        }
    }
}

/// Probe scope, then the kit syntax slots that use that probe.
const PROBES: &[(&str, &[&str])] = &[
    ("comment.line", &["comment"]),
    ("comment.block.documentation", &["comment.doc"]),
    ("keyword.control", &["keyword"]),
    ("string.quoted", &["string"]),
    ("constant.character.escape", &["string.escape"]),
    ("string.regexp", &["string.regex"]),
    ("constant.numeric", &["number"]),
    ("constant.language", &["boolean", "constant"]),
    ("entity.name.function", &["function"]),
    ("entity.name.type", &["type"]),
    ("variable", &["variable"]),
    ("variable.language", &["variable.special"]),
    ("variable.other.property", &["property"]),
    ("entity.name.tag", &["tag"]),
    ("entity.other.attribute-name", &["attribute"]),
    ("keyword.operator", &["operator"]),
    ("punctuation", &["punctuation"]),
    ("markup.heading", &["title"]),
    ("markup.italic", &["emphasis"]),
    ("markup.bold", &["emphasis.strong"]),
    ("markup.inline.raw", &["text.literal", "text.code.span"]),
    ("markup.underline.link", &["link_text", "link_uri"]),
];

struct TokenRule {
    scopes: Vec<String>,
    foreground: Option<String>,
    font_style: Option<String>,
}

fn token_rules(value: Option<&Value>) -> Vec<TokenRule> {
    let Some(items) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    items.iter().filter_map(token_rule).collect()
}

fn token_rule(value: &Value) -> Option<TokenRule> {
    let settings = value.get("settings")?;
    let foreground = settings
        .get("foreground")
        .and_then(Value::as_str)
        .map(str::to_string);
    let font_style = settings
        .get("fontStyle")
        .and_then(Value::as_str)
        .map(str::to_string);
    if foreground.is_none() && font_style.is_none() {
        return None;
    }
    let scopes = match value.get("scope")? {
        Value::String(text) => split_scopes(text),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .flat_map(split_scopes)
            .collect(),
        _ => return None,
    };
    if scopes.is_empty() {
        return None;
    }
    Some(TokenRule {
        scopes,
        foreground,
        font_style,
    })
}

fn split_scopes(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(str::to_string)
        .collect()
}

/// Longest matching scope wins. A later rule wins a tie.
fn best_rule<'a>(rules: &'a [TokenRule], probe: &str) -> Option<&'a TokenRule> {
    let mut best: Option<(usize, usize, &TokenRule)> = None;
    for (index, rule) in rules.iter().enumerate() {
        for scope in &rule.scopes {
            let Some(score) = scope_score(scope, probe) else {
                continue;
            };
            let replace = match best {
                None => true,
                Some((best_score, best_index, _)) => {
                    score > best_score || (score == best_score && index >= best_index)
                }
            };
            if replace {
                best = Some((score, index, rule));
            }
        }
    }
    best.map(|(_, _, rule)| rule)
}

fn scope_score(selector: &str, probe: &str) -> Option<usize> {
    let scope = selector.split_whitespace().next_back()?.trim();
    if scope.is_empty() {
        return None;
    }
    let matches = probe == scope
        || probe
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('.'));
    if !matches {
        return None;
    }
    Some(scope.split('.').filter(|part| !part.is_empty()).count())
}

fn highlight_style(colors: &Map<String, Value>, rules: &[TokenRule]) -> HighlightThemeStyle {
    let mut obj = Map::new();
    set_highlight(&mut obj, colors, "editor.background", "editor.background");
    set_highlight(&mut obj, colors, "editor.foreground", "editor.foreground");
    set_highlight(
        &mut obj,
        colors,
        "editor.active_line.background",
        "editor.lineHighlightBackground",
    );
    set_highlight(
        &mut obj,
        colors,
        "editor.line_number",
        "editorLineNumber.foreground",
    );
    set_highlight(
        &mut obj,
        colors,
        "editor.active_line_number",
        "editorLineNumber.activeForeground",
    );
    set_highlight(
        &mut obj,
        colors,
        "editor.invisible",
        "editorWhitespace.foreground",
    );
    set_highlight(
        &mut obj,
        colors,
        "editor.gutter.background",
        "editorGutter.background",
    );
    set_highlight(&mut obj, colors, "error", "editorError.foreground");
    set_highlight(&mut obj, colors, "warning", "editorWarning.foreground");
    set_highlight(&mut obj, colors, "info", "editorInfo.foreground");
    set_highlight(&mut obj, colors, "hint", "editorHint.foreground");

    let mut syntax = Map::new();
    for (probe, slots) in PROBES {
        let Some(rule) = best_rule(rules, probe) else {
            continue;
        };
        let Some(style) = style_json(rule) else {
            continue;
        };
        for slot in *slots {
            syntax.insert((*slot).to_string(), style.clone());
        }
    }
    if !syntax.is_empty() {
        obj.insert("syntax".to_string(), Value::Object(syntax));
    }
    serde_json::from_value(Value::Object(obj)).unwrap_or_default()
}

fn set_highlight(
    obj: &mut Map<String, Value>,
    colors: &Map<String, Value>,
    kit: &str,
    vscode: &str,
) {
    let Some(raw) = colors.get(vscode).and_then(Value::as_str) else {
        return;
    };
    let Some(hex) = highlight_hex(raw) else {
        return;
    };
    obj.insert(kit.to_string(), Value::String(hex));
}

fn style_json(rule: &TokenRule) -> Option<Value> {
    let mut style = Map::new();
    if let Some(raw) = rule.foreground.as_deref()
        && let Some(hex) = highlight_hex(raw)
    {
        style.insert("color".to_string(), Value::String(hex));
    }
    if let Some(font_style) = rule.font_style.as_deref() {
        let words: Vec<&str> = font_style.split_whitespace().collect();
        if words.iter().any(|word| word.eq_ignore_ascii_case("italic")) {
            style.insert(
                "font_style".to_string(),
                Value::String("italic".to_string()),
            );
        }
        if words.iter().any(|word| word.eq_ignore_ascii_case("bold")) {
            style.insert("font_weight".to_string(), serde_json::json!(700));
        }
    }
    if style.is_empty() {
        None
    } else {
        Some(Value::Object(style))
    }
}

fn highlight_hex(raw: &str) -> Option<String> {
    let parsed = try_parse_color(raw).ok()?;
    if raw.starts_with('#') {
        Some(raw.to_string())
    } else {
        Some(parsed.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{MAX_THEME_BYTES, load_dir, parse_named, read_theme_path, scope_score};
    use gpui_kit::component::try_parse_color;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "runebook-theme-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_vscode_fixture_sets_sidebar_line_and_keyword() {
        let theme = parse_named(
            "fixture.json",
            r##"{
                "name": "Fixture",
                "type": "dark",
                "colors": {
                    "sideBar.background": "#112233",
                    "editor.lineHighlightBackground": "#F5F5F5",
                    "not.a.slot": "#ffffff",
                    "editor.background": "not-a-color"
                },
                "tokenColors": [
                    {
                        "scope": "keyword",
                        "settings": { "foreground": "#0433ff" }
                    }
                ],
                "semanticTokenColors": { "keyword": "#ffffff" }
            }"##,
            None,
        )
        .unwrap();
        assert_eq!(theme.config.colors.sidebar.as_deref(), Some("#112233"));
        assert!(theme.config.colors.primary.is_none());
        assert!(theme.config.colors.background.is_none());
        assert!(theme.config.colors.tab_bar.is_none());
        let highlight = theme.config.highlight.as_ref().unwrap();
        assert_eq!(
            highlight.editor_active_line,
            Some(try_parse_color("#F5F5F5").unwrap())
        );
        let keyword = highlight.syntax.style("keyword").unwrap();
        assert_eq!(keyword.color, Some(try_parse_color("#0433ff").unwrap()));
    }

    #[test]
    fn the_note_surface_uses_the_editor_and_danger_uses_the_error_ink() {
        let theme = parse_named(
            "surfaces.json",
            r##"{
                "name": "Surfaces",
                "type": "dark",
                "colors": {
                    "editor.background": "#111111",
                    "editor.foreground": "#eeeeee",
                    "editorGroupHeader.tabsBackground": "#222222",
                    "editorError.foreground": "#cc0000",
                    "errorForeground": "#ff0000",
                    "inputValidation.errorBackground": "#330000",
                    "list.activeSelectionBackground": "#303030",
                    "list.activeSelectionForeground": "#eeeeee",
                    "editorWarning.foreground": "#cc8800",
                    "editorInfo.foreground": "#0088cc"
                }
            }"##,
            None,
        )
        .unwrap();
        assert_eq!(theme.config.colors.background.as_deref(), Some("#111111"));
        assert_eq!(theme.config.colors.tab_bar.as_deref(), Some("#111111"));
        assert_eq!(
            theme.config.colors.tab_bar_segmented.as_deref(),
            Some("#222222")
        );
        assert_eq!(theme.config.colors.danger.as_deref(), Some("#cc0000"));
        assert_eq!(
            theme.config.colors.danger_foreground.as_deref(),
            Some("#111111")
        );
        assert_eq!(theme.config.colors.accent.as_deref(), Some("#303030"));
        assert_eq!(theme.config.colors.warning.as_deref(), Some("#cc8800"));
        assert_eq!(theme.config.colors.info.as_deref(), Some("#0088cc"));
        assert_eq!(theme.config.colors.switch_thumb.as_deref(), Some("#eeeeee"));
    }

    #[test]
    fn a_longer_scope_beats_a_shorter_one_and_a_later_rule_wins_a_tie() {
        let theme = parse_named(
            "scopes.json",
            r##"{
                "name": "Scopes",
                "type": "light",
                "colors": {},
                "tokenColors": [
                    { "scope": "keyword.control", "settings": { "foreground": "#222222" } },
                    { "scope": "keyword", "settings": { "foreground": "#111111" } },
                    { "scope": "string.quoted", "settings": { "foreground": "#aaaaaa" } },
                    { "scope": ["string.quoted", "comment"], "settings": { "foreground": "#bbbbbb" } }
                ]
            }"##,
            None,
        )
        .unwrap();
        let syntax = &theme.config.highlight.as_ref().unwrap().syntax;
        assert_eq!(
            syntax.style("keyword").unwrap().color,
            Some(try_parse_color("#222222").unwrap())
        );
        assert_eq!(
            syntax.style("string").unwrap().color,
            Some(try_parse_color("#bbbbbb").unwrap())
        );
        assert_eq!(scope_score("keyword.control", "keyword"), None);
    }

    #[test]
    fn italic_and_bold_map_onto_the_kit_style() {
        let theme = parse_named(
            "style.json",
            r#"{
                "name": "Style",
                "type": "dark",
                "tokenColors": [
                    { "scope": "markup.italic", "settings": { "fontStyle": "italic" } },
                    { "scope": "markup.bold", "settings": { "fontStyle": "bold" } }
                ]
            }"#,
            None,
        )
        .unwrap();
        let syntax = &theme.config.highlight.as_ref().unwrap().syntax;
        assert_eq!(
            syntax.style("emphasis").unwrap().font_style,
            Some(gpui_kit::FontStyle::Italic)
        );
        assert_eq!(
            syntax.style("emphasis.strong").unwrap().font_weight,
            Some(gpui_kit::FontWeight::BOLD)
        );
    }

    #[test]
    fn an_error_names_the_file_and_not_the_body() {
        let err = parse_named("bad.json", r#"{"secret":"hunter2"}"#, None).unwrap_err();
        let text = err.to_string();
        assert_eq!(text, "bad.json: not a color theme");
        assert!(!text.contains("hunter2"));
    }

    #[test]
    fn a_bad_include_a_symlink_and_a_large_file_are_skipped() {
        let root = TempDir::new();
        let dir = root.path().join("themes");
        std::fs::create_dir(&dir).unwrap();
        let secret = root.path().join("secret.json");
        std::fs::write(&secret, r#"{"name":"Secret","type":"dark","colors":{}}"#).unwrap();
        std::fs::write(
            dir.join("bad-include.json"),
            r#"{"name":"Nope","type":"dark","include":"../secret.json","colors":{}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("ok.json"),
            r##"{"name":"Ok","type":"light","colors":{"sideBar.background":"#010101"}}"##,
        )
        .unwrap();
        let outside = root.path().join("linked.json");
        std::fs::write(
            &outside,
            r##"{"name":"Linked","type":"dark","colors":{"sideBar.background":"#abcdef"}}"##,
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link.json")).unwrap();
        let mut large = vec![b'x'; MAX_THEME_BYTES as usize + 1];
        large.splice(0..2, *b"{ ");
        std::fs::write(dir.join("large.json"), large).unwrap();

        let themes = load_dir(&dir);
        let names: Vec<_> = themes.iter().map(|theme| theme.name.as_str()).collect();
        assert_eq!(names, ["Ok"]);
        assert!(read_theme_path(&dir.join("bad-include.json")).is_err());
        assert!(read_theme_path(&dir.join("large.json")).is_err());
        let err = read_theme_path(&dir.join("bad-include.json")).unwrap_err();
        assert_eq!(err.to_string(), "bad-include.json: bad include");
        assert!(!err.to_string().contains("Secret"));
    }

    #[test]
    fn include_merges_colors_and_the_outer_file_wins() {
        let dir = TempDir::new();
        std::fs::write(
            dir.path().join("base.json"),
            r##"{
                "name": "Base",
                "type": "dark",
                "colors": { "sideBar.background": "#111111", "editor.background": "#222222" },
                "tokenColors": [
                    { "scope": "keyword", "settings": { "foreground": "#111111" } }
                ]
            }"##,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("outer.json"),
            r##"{
                "include": "base.json",
                "name": "Outer",
                "type": "light",
                "colors": { "editor.background": "#333333" },
                "tokenColors": [
                    { "scope": "keyword.control", "settings": { "foreground": "#444444" } }
                ]
            }"##,
        )
        .unwrap();
        let theme = read_theme_path(&dir.path().join("outer.json")).unwrap();
        assert_eq!(theme.name, "Outer");
        assert!(!theme.config.mode.is_dark());
        assert_eq!(theme.config.colors.sidebar.as_deref(), Some("#111111"));
        assert_eq!(theme.config.colors.background.as_deref(), Some("#333333"));
        let keyword = theme
            .config
            .highlight
            .as_ref()
            .unwrap()
            .syntax
            .style("keyword")
            .unwrap();
        assert_eq!(keyword.color, Some(try_parse_color("#444444").unwrap()));
    }

    #[test]
    fn the_later_file_name_wins_the_same_theme_name() {
        let dir = TempDir::new();
        std::fs::write(
            dir.path().join("a.json"),
            r##"{"name":"Zed","type":"dark","colors":{"sideBar.background":"#111111"}}"##,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b.json"),
            r##"{"name":"Zed","type":"dark","colors":{"sideBar.background":"#222222"}}"##,
        )
        .unwrap();
        let themes = load_dir(dir.path());
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].config.colors.sidebar.as_deref(), Some("#222222"));
    }
}
