//! Desktop window for a Runebook vault.
//!
//! The core crate owns files and wiki links. This binary only draws them.

mod anchors;
mod archivist;
mod capture;
mod chrome;
mod dock;
mod html_style;
mod http;
mod locale;
mod markdown_view;
mod mermaid;
mod models;
mod pane;
mod playback;
mod theme;

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::DockArea;
use gpui_kit::component::{ActiveTheme, Theme, ThemeMode, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Bounds, ClickEvent, Context, Entity, FocusHandle, KeyBinding, PathPromptOptions, Render,
    SharedString, WeakEntity, Window, WindowBounds, WindowDecorations, WindowOptions, div, px,
    size,
};
use runebook::{AppConfig, Error, NoteId, ThemeChoice, Vault};

use crate::pane::EditorPane;

gpui_kit::actions!(runebook, [SaveNote, NewNote, OpenVault, ToggleNoteView]);

const WELCOME: &str = "\
# Welcome

This is your Runebook vault.

The window shows the rendered note. Use the edit icon to change the source.

A wiki link looks like [[inbox]]. Click a missing link to create that note.
";

/// How often the file tree reads the vault for changes made outside the app.
const TREE_POLL: Duration = Duration::from_millis(500);

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .with_http_client(http::client())
        .run(|cx| {
            gpui_kit::init(cx);
            bind_keys(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
            let config = AppConfig::load().unwrap_or_default();
            locale::activate(&config.locale);
            let last = config.last_vault.clone();
            let theme = config.theme;
            let text_size = config.text_size;
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_decorations: Some(WindowDecorations::Client),
                    ..TitleBar::window_options()
                },
                cx,
                move |window, cx| {
                    theme::apply(cx);
                    Theme::change(theme_mode(theme), Some(window), cx);
                    theme::apply_text_size(text_size, cx);
                    cx.new(|cx| Root::new(last, window, cx))
                },
            )
            .expect("open the Runebook window");
        });
}

pub(crate) fn on_root<E: 'static>(
    view: &WeakEntity<Root>,
    f: impl Fn(&mut Root, &E, &mut Window, &mut Context<Root>) + 'static,
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    let view = view.clone();
    move |event, window, cx| {
        view.update(cx, |root, cx| f(root, event, window, cx)).ok();
    }
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-s", SaveNote, None),
        KeyBinding::new("cmd-n", NewNote, None),
        KeyBinding::new("cmd-o", OpenVault, None),
        KeyBinding::new("cmd-e", ToggleNoteView, None),
    ]);
}

fn welcome_id() -> NoteId {
    NoteId::parse("welcome").expect("welcome is one safe path segment")
}

struct Root {
    focus: FocusHandle,
    banner: Option<String>,
    pane: Option<EditorPane>,
    dock: Entity<DockArea>,
    autosave_gen: u64,
    dock_save_gen: u64,
    tree_watch: u64,
    pending_open: Option<(PathBuf, bool)>,
}

impl Root {
    fn new(last: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, {
            let weak = weak.clone();
            move |_, app| {
                weak.update(app, |this, cx| this.flush(cx)).ok();
                true
            }
        });

        let saved = AppConfig::load().ok().and_then(|config| config.dock_layout);
        let dock = crate::dock::install(weak, saved.as_deref(), window, cx);
        let mut root = Self {
            focus: cx.focus_handle(),
            banner: None,
            pane: None,
            dock,
            autosave_gen: 0,
            dock_save_gen: 0,
            tree_watch: 0,
            pending_open: None,
        };
        if let Some(path) = last {
            root.open_path(path, false, window, cx);
        }
        root.focus.focus(window, cx);
        root
    }

    fn flush(&mut self, cx: &mut Context<Self>) -> bool {
        self.autosave_gen = self.autosave_gen.wrapping_add(1);
        match self.pane.as_mut() {
            Some(pane) => pane.flush_edits(cx),
            None => true,
        }
    }

    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        self.autosave_gen = self.autosave_gen.wrapping_add(1);
        let Some(delay) = self.pane.as_ref().and_then(|pane| pane.autosave_delay()) else {
            return;
        };
        let generation = self.autosave_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| {
                if this.autosave_gen == generation {
                    this.flush(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn schedule_dock_save(&mut self, area: &Entity<DockArea>, cx: &mut Context<Self>) {
        self.dock_save_gen = self.dock_save_gen.wrapping_add(1);
        let generation = self.dock_save_gen;
        let area = area.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            this.update(cx, |this, cx| {
                if this.dock_save_gen != generation {
                    return;
                }
                let state = area.read(cx).dump(cx);
                let Ok(json) = serde_json::to_string(&state) else {
                    return;
                };
                if json.len() > crate::dock::LAYOUT_LIMIT {
                    return;
                }
                save_setting(|config| config.dock_layout = Some(json)).ok();
            })
            .ok();
        })
        .detach();
    }

    fn on_edit(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self.pane.as_mut() else {
            return;
        };
        if pane.capture_edit(cx) {
            self.schedule_autosave(cx);
        }
        cx.notify();
    }

    fn open_note(&mut self, id: NoteId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.flush(cx) {
            cx.notify();
            return;
        }
        if let Some(pane) = self.pane.as_mut()
            && let Err(err) = pane.load_note(id, window, cx)
        {
            pane.set_notice(err);
        }
        cx.notify();
    }

    fn follow_link(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !url.starts_with("runebook://") {
            if runebook::is_external_url(url) {
                cx.open_url(url);
            } else if let Err(err) = self.open_media(url) {
                self.set_notice(err);
            }
            cx.notify();
            return;
        }
        let Some(target) = runebook::note_target_from_url(url) else {
            self.set_notice(locale::text("link.not_note"));
            cx.notify();
            return;
        };
        let Some(resolved) = self.pane.as_ref().map(|pane| pane.index().resolve(&target)) else {
            return;
        };
        match resolved {
            runebook::ResolveResult::Exact(id) | runebook::ResolveResult::UniqueStem(id) => {
                self.open_note(id, window, cx);
            }
            runebook::ResolveResult::Missing => self.create_missing(&target, window, cx),
            runebook::ResolveResult::Ambiguous(ids) => {
                let names = ids
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                self.set_notice(locale::format(
                    "link.ambiguous",
                    &[("target", target.as_str()), ("names", &names)],
                ));
                cx.notify();
            }
        }
    }

    fn open_media(&self, href: &str) -> Result<(), String> {
        let Some(pane) = self.pane.as_ref() else {
            return Err(locale::text("media.open_note").to_string());
        };
        let path = pane.media_path(href)?;
        let program = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        match crate::markdown_view::spawn_desktop_open(&path) {
            Ok(mut child) => match child.wait() {
                Ok(status) if status.success() => Ok(()),
                Ok(_) => Err(locale::text("media.open_failed").to_string()),
                Err(err) => Err(locale::format(
                    "media.open_failed_detail",
                    &[("err", &err.to_string())],
                )),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(locale::format(
                "media.program_missing",
                &[("program", program)],
            )),
            Err(err) => Err(locale::format(
                "media.open_failed_detail",
                &[("err", &err.to_string())],
            )),
        }
    }

    fn create_missing(
        &mut self,
        target: &runebook::WikiTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(id) = NoteId::parse(target.as_str()) else {
            self.set_notice(locale::format(
                "link.bad_name",
                &[("target", target.as_str())],
            ));
            cx.notify();
            return;
        };
        if !self.flush(cx) {
            cx.notify();
            return;
        }
        let body = format!("# {}\n", id.name());
        let Some(pane) = self.pane.as_mut() else {
            return;
        };
        if let Err(err) = pane.create_and_open(id, &body, window, cx) {
            pane.set_notice(err);
        }
        cx.notify();
    }

    fn set_notice(&mut self, notice: impl Into<String>) {
        if let Some(pane) = self.pane.as_mut() {
            pane.set_notice(notice);
        } else {
            self.banner = Some(notice.into());
        }
    }

    fn pick_vault(&mut self, must_be_new: bool, cx: &mut Context<Self>) {
        let prompt = if must_be_new {
            locale::text("empty.create")
        } else {
            locale::text("empty.open")
        };
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from(prompt)),
        });
        cx.spawn(async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next().map(Ok),
                Ok(Err(err)) => Some(Err(err.to_string())),
                _ => None,
            };
            this.update(cx, |this, cx| {
                match picked {
                    Some(Ok(path)) => this.pending_open = Some((path, must_be_new)),
                    Some(Err(err)) => this.banner = Some(err),
                    None => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_path(
        &mut self,
        path: PathBuf,
        must_be_new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.flush(cx);
        let marker = path.join("runebook.toml");
        let vault = if marker.is_file() {
            if must_be_new {
                self.banner = Some(locale::format(
                    "vault.already",
                    &[("path", &path.display().to_string())],
                ));
                cx.notify();
                return;
            }
            match Vault::open(&path) {
                Ok(vault) => vault,
                Err(err) => {
                    self.banner = Some(err.to_string());
                    self.pane = None;
                    self.arm_tree_watch(window, cx);
                    cx.notify();
                    return;
                }
            }
        } else {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(locale::text("vault.fallback_name"));
            match Vault::create(&path, name) {
                Ok(vault) => {
                    if let Err(err) = vault.create_note(&welcome_id(), WELCOME)
                        && !matches!(err, Error::NoteExists { .. })
                    {
                        self.banner = Some(err.to_string());
                        cx.notify();
                        return;
                    }
                    vault
                }
                Err(err) => {
                    self.banner = Some(err.to_string());
                    cx.notify();
                    return;
                }
            }
        };

        match EditorPane::open(vault, window, cx) {
            Ok(pane) => {
                let saved = remember_vault(pane.root());
                self.banner = None;
                self.pane = Some(pane);
                self.arm_tree_watch(window, cx);
                if let Err(err) = saved {
                    self.set_notice(locale::format(
                        "vault.save_failed",
                        &[("err", &err.to_string())],
                    ));
                }
            }
            Err(err) => {
                self.banner = Some(err.to_string());
                self.pane = None;
                self.arm_tree_watch(window, cx);
            }
        }
        cx.notify();
    }

    /// Read the vault on a timer so a change made outside the app shows up.
    ///
    /// A scan that started before the latest local edit is dropped.
    fn arm_tree_watch(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.tree_watch = self.tree_watch.wrapping_add(1);
        let generation = self.tree_watch;
        let Some(root) = self.pane.as_ref().map(|pane| pane.root().to_path_buf()) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(TREE_POLL).await;
                let Some(base) = this
                    .update(cx, |this, _| {
                        if this.tree_watch != generation {
                            return None;
                        }
                        this.pane.as_ref().map(|pane| pane.file_stamp().clone())
                    })
                    .ok()
                    .flatten()
                else {
                    break;
                };
                let root = root.clone();
                let base_for_scan = base.clone();
                let scanned = cx
                    .background_executor()
                    .spawn(async move { pane::scan_vault(&root, &base_for_scan) })
                    .await;
                let keep = this
                    .update_in(cx, |this, window, cx| {
                        if this.tree_watch != generation {
                            return false;
                        }
                        let Some(pane) = this.pane.as_mut() else {
                            return false;
                        };
                        let changed = match scanned {
                            Ok(Some(refresh)) => {
                                pane.apply_disk_refresh(&base, refresh, window, cx)
                            }
                            Ok(None) => false,
                            Err(err) => pane.fail_disk_scan(err.to_string()),
                        };
                        if changed {
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let banner = self.banner.clone();
        let background = cx.theme().background;
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        div()
            .flex_1()
            .h_full()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .bg(background)
            .text_color(foreground)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_2xl().child(locale::text("brand.name")))
                            .child(div().text_color(muted).child(locale::text("brand.alpha"))),
                    )
                    .child(div().text_color(muted).child(locale::text("empty.body")))
                    .when_some(banner, |this, banner| {
                        this.child(div().text_color(danger).child(banner))
                    })
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("create-vault")
                                    .primary()
                                    .cursor_pointer()
                                    .icon(IconName::FolderPlus)
                                    .label(locale::text("empty.create"))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.pick_vault(true, cx);
                                    })),
                            )
                            .child(
                                Button::new("open-vault")
                                    .outline()
                                    .cursor_pointer()
                                    .icon(IconName::FolderOpen)
                                    .label(locale::text("empty.open"))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.pick_vault(false, cx);
                                    })),
                            ),
                    ),
            )
    }
}

fn remember_vault(path: &std::path::Path) -> runebook::Result<()> {
    let mut config = AppConfig::load().unwrap_or_default();
    config.last_vault = Some(path.to_path_buf());
    config.save()
}

pub(crate) fn theme_mode(choice: ThemeChoice) -> ThemeMode {
    match choice {
        ThemeChoice::Dark => ThemeMode::Dark,
        ThemeChoice::Light => ThemeMode::Light,
    }
}

/// Load the user config, change one field, and write it back.
pub(crate) fn save_setting(change: impl FnOnce(&mut AppConfig)) -> Result<(), String> {
    let mut config = AppConfig::load().map_err(|err| err.to_string())?;
    change(&mut config);
    config.save().map_err(|err| err.to_string())
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((path, must_be_new)) = self.pending_open.take() {
            let view = cx.weak_entity();
            window.defer(cx, move |window, app| {
                view.update(app, |this, cx| {
                    this.open_path(path, must_be_new, window, cx);
                })
                .ok();
            });
        }

        if let Some(pane) = self.pane.as_mut() {
            pane.sync_picture_width(window);
        }
        let view = cx.weak_entity();
        let background = cx.theme().background;
        let foreground = cx.theme().foreground;
        let mut title = TitleBar::new().h(px(40.)).bg(background).on_close_window({
            let view = view.clone();
            move |_, window, cx| {
                view.update(cx, |this, cx| {
                    this.flush(cx);
                })
                .ok();
                window.remove_window();
            }
        });
        if let Some(pane) = self.pane.as_ref() {
            title = title.child(pane::toolbar(pane, view, cx));
        }
        let body = if self.pane.is_some() {
            div()
                .flex_1()
                .h_full()
                .min_h(px(0.))
                .min_w(px(0.))
                .child(self.dock.clone())
                .into_any_element()
        } else {
            self.welcome(cx).into_any_element()
        };
        let status = self
            .pane
            .as_ref()
            .map(|pane| pane::status_bar(pane, cx).into_any_element());

        div()
            .id("runebook")
            .size_full()
            .flex()
            .flex_col()
            .bg(background)
            .text_color(foreground)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SaveNote, _, cx| {
                this.flush(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &NewNote, window, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.begin_create_note(window, cx);
                } else {
                    this.banner = Some(locale::text("vault.open_first").to_string());
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenVault, _, cx| {
                this.pick_vault(false, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleNoteView, window, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.toggle_note_view(window, cx);
                }
                cx.notify();
            }))
            .child(title)
            .child(body)
            .children(status)
    }
}
