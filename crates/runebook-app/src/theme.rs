//! The window theme.
//!
//! Built-in files are VS Code color themes. User files live in
//! [`runebook::AppConfig::themes_dir`]. A missing font family is left unset,
//! so text layout does not panic on a missing name.

use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use gpui_kit::component::{Theme, ThemeConfig};
use gpui_kit::{App, px};
use runebook::{AppConfig, TextSize};

use crate::vscode_theme::{self, DEFAULT_SPACING, LoadedTheme, NoteSpacing};

const DARK_RAW: &str = include_str!("../../../themes/runebook-dark.json");
const LIGHT_RAW: &str = include_str!("../../../themes/runebook-light.json");
const OBSIDIAN_DARK_RAW: &str = include_str!("../../../themes/obsidian-dark.json");
const OBSIDIAN_LIGHT_RAW: &str = include_str!("../../../themes/obsidian-light.json");
const BLUE_TOPAZ_DARK_RAW: &str = include_str!("../../../themes/blue-topaz-dark.json");
const BLUE_TOPAZ_LIGHT_RAW: &str = include_str!("../../../themes/blue-topaz-light.json");

/// Marketplace themes, in install order. `themes/NOTICE` names each source.
const VENDORED: &[(&str, &str)] = &[
    (
        "github-dark-default.json",
        include_str!("../../../themes/github-dark-default.json"),
    ),
    (
        "one-dark-pro.json",
        include_str!("../../../themes/one-dark-pro.json"),
    ),
    ("dracula.json", include_str!("../../../themes/dracula.json")),
    (
        "atom-one-dark.json",
        include_str!("../../../themes/atom-one-dark.json"),
    ),
    (
        "ayu-dark.json",
        include_str!("../../../themes/ayu-dark.json"),
    ),
    (
        "winter-is-coming.json",
        include_str!("../../../themes/winter-is-coming.json"),
    ),
    (
        "night-owl.json",
        include_str!("../../../themes/night-owl.json"),
    ),
    (
        "tokyo-night.json",
        include_str!("../../../themes/tokyo-night.json"),
    ),
    (
        "one-monokai.json",
        include_str!("../../../themes/one-monokai.json"),
    ),
    (
        "palenight.json",
        include_str!("../../../themes/palenight.json"),
    ),
    (
        "synthwave-84.json",
        include_str!("../../../themes/synthwave-84.json"),
    ),
    (
        "shades-of-purple.json",
        include_str!("../../../themes/shades-of-purple.json"),
    ),
    ("cobalt2.json", include_str!("../../../themes/cobalt2.json")),
    (
        "andromeda.json",
        include_str!("../../../themes/andromeda.json"),
    ),
    ("theme.json", include_str!("../../../themes/theme.json")),
];

/// Name of the built-in dark theme.
pub(crate) const DARK_NAME: &str = "Runebook Dark";
/// Name of the built-in light theme.
pub(crate) const LIGHT_NAME: &str = "Runebook Light";

const DEFAULT_BODY: f32 = 14.;
const DEFAULT_MONO: f32 = 13.;

struct ActiveLook {
    spacing: NoteSpacing,
    body: f32,
    mono: f32,
}

const DEFAULT_LOOK: ActiveLook = ActiveLook {
    spacing: DEFAULT_SPACING,
    body: DEFAULT_BODY,
    mono: DEFAULT_MONO,
};

static ACTIVE: Mutex<ActiveLook> = Mutex::new(DEFAULT_LOOK);

/// Body size chosen in Settings. `0` means the theme file has not been scaled.
static NOTE_BODY: AtomicU32 = AtomicU32::new(0);

/// Built-in themes, then user themes. A user file replaces a built-in name.
pub(crate) struct Catalog {
    themes: Vec<LoadedTheme>,
}

impl Catalog {
    /// Load the built-in themes and every theme in `dir`.
    pub(crate) fn load(dir: &Path) -> Self {
        Self::assemble(builtin_themes(), vscode_theme::load_dir(dir))
    }

    fn assemble(mut themes: Vec<LoadedTheme>, user: Vec<LoadedTheme>) -> Self {
        let mut extras = Vec::new();
        for theme in user {
            if let Some(slot) = themes.iter_mut().find(|item| item.name == theme.name) {
                *slot = theme;
            } else {
                extras.push(theme);
            }
        }
        extras.sort_by(|left, right| left.name.cmp(&right.name));
        themes.extend(extras);
        Self { themes }
    }

    /// Theme names in Settings order.
    pub(crate) fn names(&self) -> Vec<String> {
        self.themes.iter().map(|theme| theme.name.clone()).collect()
    }

    /// The theme for `saved`. `dark` and `light` select the built-in pair.
    /// An unknown name uses Runebook Dark.
    pub(crate) fn resolve(&self, saved: &str) -> &LoadedTheme {
        let name = match saved {
            "dark" => DARK_NAME,
            "light" => LIGHT_NAME,
            other => other,
        };
        self.themes
            .iter()
            .find(|theme| theme.name == name)
            .or_else(|| self.themes.iter().find(|theme| theme.name == DARK_NAME))
            .unwrap_or(&self.themes[0])
    }

    pub(crate) fn by_name(&self, name: &str) -> Option<&LoadedTheme> {
        self.themes.iter().find(|theme| theme.name == name)
    }
}

/// Themes loaded for this process.
pub(crate) fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| Catalog::load(&AppConfig::themes_dir()))
}

fn builtin_themes() -> Vec<LoadedTheme> {
    let mut themes = vec![
        vscode_theme::parse_named("runebook-dark.json", DARK_RAW, None)
            .expect("themes/runebook-dark.json is a color theme"),
        vscode_theme::parse_named("runebook-light.json", LIGHT_RAW, None)
            .expect("themes/runebook-light.json is a color theme"),
        vscode_theme::parse_named("obsidian-dark.json", OBSIDIAN_DARK_RAW, None)
            .expect("themes/obsidian-dark.json is a color theme"),
        vscode_theme::parse_named("obsidian-light.json", OBSIDIAN_LIGHT_RAW, None)
            .expect("themes/obsidian-light.json is a color theme"),
        vscode_theme::parse_named("blue-topaz-dark.json", BLUE_TOPAZ_DARK_RAW, None)
            .expect("themes/blue-topaz-dark.json is a color theme"),
        vscode_theme::parse_named("blue-topaz-light.json", BLUE_TOPAZ_LIGHT_RAW, None)
            .expect("themes/blue-topaz-light.json is a color theme"),
    ];
    for (file_name, raw) in VENDORED {
        themes.push(
            vscode_theme::parse_named(file_name, raw, None).unwrap_or_else(|err| panic!("{err}")),
        );
    }
    themes
}

/// Body and mono sizes for `size`, in pixels.
///
/// Medium is the active theme. Small is 2 px smaller. Large is 4 px and 3 px larger.
pub(crate) fn text_size_px(size: TextSize) -> (f32, f32) {
    let (body, mono) = theme_font_px();
    match size {
        TextSize::Small => ((body - 2.).max(10.), (mono - 2.).max(10.)),
        TextSize::Medium => (body, mono),
        TextSize::Large => (body + 4., mono + 3.),
    }
}

/// Apply `size` to the window theme and the note preview.
pub(crate) fn apply_text_size(size: TextSize, cx: &mut App) {
    let (body, mono) = text_size_px(size);
    NOTE_BODY.store(body as u32, Ordering::Relaxed);
    Theme::update(cx, |theme| {
        theme.font_size = px(body);
        theme.mono_font_size = px(mono);
    });
}

/// Body size from Settings, or from the active theme before Settings applies.
pub(crate) fn body_font_size() -> f32 {
    match NOTE_BODY.load(Ordering::Relaxed) {
        0 => theme_font_px().0,
        size => size as f32,
    }
}

fn theme_font_px() -> (f32, f32) {
    let look = ACTIVE.lock().unwrap_or_else(|err| err.into_inner());
    (look.body, look.mono)
}

/// Note spacing from the active theme.
pub(crate) fn note_spacing() -> NoteSpacing {
    ACTIVE.lock().unwrap_or_else(|err| err.into_inner()).spacing
}

/// Clear a font family the machine does not have.
pub(crate) fn drop_missing_fonts(config: &mut ThemeConfig, installed: &[String]) {
    if config
        .font_family
        .as_ref()
        .is_some_and(|family| !installed.iter().any(|name| name == family.as_ref()))
    {
        config.font_family = None;
    }
    if config
        .mono_font_family
        .as_ref()
        .is_some_and(|family| !installed.iter().any(|name| name == family.as_ref()))
    {
        config.mono_font_family = None;
    }
}

/// Install `theme` and switch the window to its mode.
pub(crate) fn apply(theme: &LoadedTheme, cx: &mut App) {
    {
        let mut look = ACTIVE.lock().unwrap_or_else(|err| err.into_inner());
        *look = ActiveLook {
            spacing: theme.spacing,
            body: theme.config.font_size.unwrap_or(DEFAULT_BODY),
            mono: theme.config.mono_font_size.unwrap_or(DEFAULT_MONO),
        };
    }
    let installed = cx.text_system().all_font_names();
    let mut config = theme.config.clone();
    drop_missing_fonts(&mut config, &installed);
    Theme::update(cx, |current| {
        current.apply_config(&Rc::new(config));
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{Catalog, DARK_NAME, LIGHT_NAME, builtin_themes, drop_missing_fonts, text_size_px};
    use crate::vscode_theme::DEFAULT_SPACING;
    use runebook::TextSize;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "runebook-catalog-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_builtin_themes_keep_primary_sidebar_and_editor_colors() {
        let themes = builtin_themes();
        assert_eq!(
            themes
                .iter()
                .map(|theme| theme.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                DARK_NAME,
                LIGHT_NAME,
                "Obsidian Dark",
                "Obsidian Light",
                "Blue Topaz Dark",
                "Blue Topaz Light",
                "GitHub Dark Default",
                "One Dark Pro",
                "Dracula Theme",
                "Atom One Dark",
                "Ayu Dark",
                "Winter is Coming (Dark Blue)",
                "Night Owl",
                "Tokyo Night",
                "One Monokai",
                "Palenight Theme",
                "SynthWave '84",
                "Shades of Purple",
                "Cobalt2",
                "Andromeda",
                "Theme",
            ]
        );
        let dark = &themes[0];
        let light = &themes[1];
        assert_eq!(dark.name, DARK_NAME);
        assert_eq!(light.name, LIGHT_NAME);
        assert!(dark.config.mode.is_dark());
        assert!(!light.config.mode.is_dark());
        assert_eq!(dark.config.colors.primary.as_deref(), Some("neutral-50"));
        assert_eq!(dark.config.colors.sidebar.as_deref(), Some("#0a0a0a"));
        assert_eq!(dark.config.colors.background.as_deref(), Some("#0a0a0a"));
        assert_eq!(light.config.colors.primary.as_deref(), Some("neutral-900"));
        assert_eq!(light.config.colors.sidebar.as_deref(), Some("#fafafa"));
        assert_eq!(light.config.colors.background.as_deref(), Some("#ffffff"));
        assert!(dark.config.highlight.is_some());
        assert!(light.config.highlight.is_some());
        let obsidian_dark = &themes[2];
        let obsidian_light = &themes[3];
        assert!(obsidian_dark.config.mode.is_dark());
        assert!(!obsidian_light.config.mode.is_dark());
        assert_eq!(
            obsidian_dark.config.colors.background.as_deref(),
            Some("#202020")
        );
        assert_eq!(
            obsidian_dark.config.colors.sidebar.as_deref(),
            Some("#161616")
        );
        assert_eq!(
            obsidian_dark.config.colors.primary.as_deref(),
            Some("#483699")
        );
        assert_eq!(
            obsidian_light.config.colors.background.as_deref(),
            Some("#ffffff")
        );
        assert_eq!(
            obsidian_light.config.colors.sidebar.as_deref(),
            Some("#f2f3f5")
        );
        assert_eq!(
            obsidian_light.config.colors.primary.as_deref(),
            Some("#7b6cd9")
        );
        let topaz_dark = &themes[4];
        let topaz_light = &themes[5];
        assert!(topaz_dark.config.mode.is_dark());
        assert!(!topaz_light.config.mode.is_dark());
        assert_eq!(
            topaz_dark.config.colors.background.as_deref(),
            Some("#202020")
        );
        assert_eq!(topaz_dark.config.colors.sidebar.as_deref(), Some("#151515"));
        assert_eq!(topaz_dark.config.colors.primary.as_deref(), Some("#2d82cc"));
        assert_eq!(
            topaz_light.config.colors.background.as_deref(),
            Some("#ffffff")
        );
        assert_eq!(
            topaz_light.config.colors.sidebar.as_deref(),
            Some("#fcfcfc")
        );
        assert_eq!(
            topaz_light.config.colors.primary.as_deref(),
            Some("#468eeb")
        );
        for theme in &themes[..2] {
            assert_eq!(theme.config.font_family.as_deref(), Some("Noto Sans"));
            assert_eq!(
                theme.config.mono_font_family.as_deref(),
                Some("Noto Sans Mono")
            );
            assert_eq!(theme.config.font_size, Some(14.));
            assert_eq!(theme.config.mono_font_size, Some(13.));
            assert_eq!(theme.config.radius, Some(6));
            assert_eq!(theme.config.radius_lg, Some(8));
            assert_eq!(theme.spacing, DEFAULT_SPACING);
            assert_ne!(
                theme.config.colors.primary.as_deref(),
                theme.config.colors.primary_foreground.as_deref()
            );
        }
        for theme in &themes[6..] {
            assert!(theme.config.mode.is_dark());
            assert!(theme.config.colors.background.is_some());
            assert!(theme.config.highlight.is_some());
        }
    }

    #[test]
    fn text_size_scales_around_the_theme_file() {
        let (body, mono) = text_size_px(TextSize::Medium);
        assert_eq!((body, mono), (14., 13.));
        assert_eq!(text_size_px(TextSize::Small), (12., 11.));
        assert_eq!(text_size_px(TextSize::Large), (18., 16.));
    }

    #[test]
    fn a_missing_family_is_left_unset() {
        let mut config = builtin_themes()[0].config.clone();
        drop_missing_fonts(&mut config, &["Other".to_string()]);
        assert!(config.font_family.is_none());
        assert!(config.mono_font_family.is_none());
    }

    #[test]
    fn settings_order_is_builtin_then_alphabetical_and_aliases_resolve() {
        let dir = TempDir::new();
        std::fs::write(
            dir.0.join("z.json"),
            r##"{"name":"Zulu","type":"dark","colors":{"sideBar.background":"#111111"}}"##,
        )
        .unwrap();
        std::fs::write(
            dir.0.join("a.json"),
            r##"{"name":"Alpha","type":"light","colors":{"sideBar.background":"#222222"}}"##,
        )
        .unwrap();
        std::fs::write(
            dir.0.join("replace.json"),
            r##"{"name":"Runebook Dark","type":"dark","colors":{"sideBar.background":"#333333","button.background":"neutral-50"}}"##,
        )
        .unwrap();
        let catalog = Catalog::load(&dir.0);
        let names = catalog.names();
        assert_eq!(names[0], "Runebook Dark");
        assert_eq!(names[1], "Runebook Light");
        assert_eq!(names[names.len() - 2], "Alpha");
        assert_eq!(names[names.len() - 1], "Zulu");
        assert_eq!(names.len(), builtin_themes().len() + 2);
        assert_eq!(
            catalog.resolve("dark").config.colors.sidebar.as_deref(),
            Some("#333333")
        );
        assert_eq!(catalog.resolve("light").name, LIGHT_NAME);
        assert_eq!(catalog.resolve("missing").name, DARK_NAME);
        assert_eq!(catalog.resolve("Alpha").name, "Alpha");
    }
}
