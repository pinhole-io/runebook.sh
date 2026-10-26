//! The compiled theme file.
//!
//! `themes/runebook.json` sets the face, the size, the radius, the colors,
//! and the note spacing. A family that is not installed is left unset, so
//! text layout does not panic on a missing name.

use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use gpui_kit::App;
use gpui_kit::component::{Theme, ThemeConfig, ThemeSet};
use gpui_kit::px;
use runebook::TextSize;

const RAW: &str = include_str!("../../../themes/runebook.json");

/// Heading scale and paragraph gap from the theme file.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NoteSpacing {
    pub paragraph_gap: f32,
    /// Extra space before a heading that follows text, a table, or another block.
    pub heading_before: f32,
    pub heading_1: f32,
    pub heading_2: f32,
}

pub(crate) fn theme_set() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(|| serde_json::from_str(RAW).expect("themes/runebook.json is a theme set"))
}

/// Body size chosen in Settings. `0` means the theme file has not been scaled.
static NOTE_BODY: AtomicU32 = AtomicU32::new(0);

/// Body and mono sizes for `size`, in pixels.
///
/// Medium is the theme file. Small is 2 px smaller. Large is 4 px and 3 px larger.
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

/// Body size from Settings, or from the theme file before Settings applies.
pub(crate) fn body_font_size() -> f32 {
    match NOTE_BODY.load(Ordering::Relaxed) {
        0 => theme_font_px().0,
        size => size as f32,
    }
}

fn theme_font_px() -> (f32, f32) {
    let theme = theme_set().themes.first();
    let body = theme.and_then(|item| item.font_size).unwrap_or(14.);
    let mono = theme.and_then(|item| item.mono_font_size).unwrap_or(13.);
    (body, mono)
}

/// Note spacing from the first theme. Both modes use the same numbers.
pub(crate) fn note_spacing() -> NoteSpacing {
    theme_spacings()[0]
}

fn theme_spacings() -> &'static [NoteSpacing] {
    static SPACING: OnceLock<Vec<NoteSpacing>> = OnceLock::new();
    SPACING.get_or_init(|| {
        let value: serde_json::Value =
            serde_json::from_str(RAW).expect("themes/runebook.json is JSON");
        let themes = value["themes"]
            .as_array()
            .expect("themes/runebook.json has themes");
        themes
            .iter()
            .map(|theme| {
                let spacing = &theme["spacing"];
                NoteSpacing {
                    paragraph_gap: spacing_number(spacing, "paragraph_gap"),
                    heading_before: spacing_number(spacing, "heading_before"),
                    heading_1: spacing_number(spacing, "heading.1"),
                    heading_2: spacing_number(spacing, "heading.2"),
                }
            })
            .collect()
    })
}

fn spacing_number(spacing: &serde_json::Value, key: &str) -> f32 {
    spacing[key]
        .as_f64()
        .unwrap_or_else(|| panic!("themes/runebook.json spacing.{key}")) as f32
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

/// Install both modes. Call this before [`Theme::change`].
pub(crate) fn apply(cx: &mut App) {
    let installed = cx.text_system().all_font_names();
    let themes = theme_set().themes.clone();
    Theme::update(cx, |theme| {
        for config in themes {
            let mut config = config;
            drop_missing_fonts(&mut config, &installed);
            theme.apply_config(&Rc::new(config));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{drop_missing_fonts, text_size_px, theme_set, theme_spacings};
    use runebook::TextSize;

    #[test]
    fn the_theme_file_parses() {
        let set = theme_set();
        assert_eq!(set.themes.len(), 2);
        assert!(set.themes.iter().any(|theme| theme.mode.is_dark()));
        assert!(set.themes.iter().any(|theme| !theme.mode.is_dark()));
        for theme in &set.themes {
            assert_eq!(theme.font_family.as_deref(), Some("Noto Sans"));
            assert_eq!(theme.mono_font_family.as_deref(), Some("Noto Sans Mono"));
            assert_eq!(theme.font_size, Some(14.));
            assert_eq!(theme.mono_font_size, Some(13.));
            assert_eq!(theme.radius, Some(6));
            assert_eq!(theme.radius_lg, Some(8));
            assert!(theme.colors.primary.is_some());
            assert!(theme.colors.primary_foreground.is_some());
            assert!(theme.colors.primary_hover.is_some());
            assert!(theme.colors.primary_active.is_some());
            assert_ne!(
                theme.colors.primary.as_deref(),
                theme.colors.primary_foreground.as_deref()
            );
            assert!(theme.highlight.is_some());
        }
    }

    #[test]
    fn the_note_spacing_comes_from_the_theme_file() {
        let spacings = theme_spacings();
        assert_eq!(spacings.len(), 2);
        for spacing in spacings {
            assert_eq!(spacing.paragraph_gap, 1.0);
            assert_eq!(spacing.heading_before, 1.0);
            assert_eq!(spacing.heading_1, 1.4);
            assert_eq!(spacing.heading_2, 1.2);
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
        let mut config = theme_set().themes[0].clone();
        drop_missing_fonts(&mut config, &["Other".to_string()]);
        assert!(config.font_family.is_none());
        assert!(config.mono_font_family.is_none());
    }
}
