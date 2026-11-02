//! The file tree for the open vault.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Enter, Escape, Input, InputState};
#[cfg(target_os = "macos")]
use gpui_kit::component::menu::ContextMenuExt;
#[cfg(target_os = "macos")]
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, App, AppContext, ClickEvent, Context, Hsla, MouseButton, Render,
    SharedString, Transformation, WeakEntity, Window, black, div, img, px, size, svg,
};

use super::{EditorPane, NameAction, Prompt, TreeSelection};
use crate::chrome::{FOOTER_PAD_Y, FOOTER_ROW_HEIGHT, composer_field_height, panel_header};
use runebook::NoteKind;

use crate::locale;
use crate::{NewNote, OpenVault, Root, on_root};

/// Which capture is live on one note in the file tree.
#[derive(Clone, Copy)]
enum RowCapture {
    Voice,
    Screen,
}

/// The note that owns the live capture. Transcribing and saving stay unmarked.
fn row_capture(pane: &EditorPane) -> Option<(runebook::NoteId, RowCapture)> {
    let id = pane.recording_note.clone()?;
    if pane.voice_recording() {
        Some((id, RowCapture::Voice))
    } else if pane.screen_recording() {
        Some((id, RowCapture::Screen))
    } else {
        None
    }
}

/// A live capture uses the same icon slot as the other notes. A click stops it.
/// The glyph pulses larger than that slot. The slot itself does not move.
fn record_mark(
    capture: RowCapture,
    key: &str,
    view: WeakEntity<Root>,
    cx: &App,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("record-{key}")))
        .flex_shrink_0()
        .cursor_pointer()
        .child(RecordDot {
            color: cx.theme().danger,
        })
        .aria_label(locale::text("toolbar.stop_recording"))
        .tooltip(|window, cx| {
            Tooltip::new(locale::text("toolbar.stop_recording")).build(window, cx)
        })
        .on_click(on_root(&view, move |this, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            if let Some(pane) = this.pane.as_mut() {
                match capture {
                    RowCapture::Voice => pane.toggle_voice(cx),
                    RowCapture::Screen => pane.toggle_screen(cx),
                }
            }
            cx.notify();
        }))
}

/// Red record mark at the same size as a note icon.
/// The scale starts at 1 so reduced motion still shows a normal icon.
#[derive(gpui_kit::IntoElement)]
struct RecordDot {
    color: Hsla,
}

impl RenderOnce for RecordDot {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let text = window.text_style();
        svg()
            .path(IconName::CircleDot.path())
            .flex_shrink_0()
            .size(text.font_size.to_pixels(window.rem_size()))
            .text_color(self.color)
            .with_animation(
                "record-pulse",
                Animation::new(Duration::from_millis(1400))
                    .repeat()
                    .with_max_fps(30.0),
                |icon, delta| {
                    let wave = (delta * std::f32::consts::PI).sin();
                    let scale = 1.0 + 0.16 * wave;
                    icon.with_transformation(Transformation::scale(size(scale, scale)))
                },
            )
    }
}

fn row_actions(
    key: &str,
    selected: bool,
    selection: TreeSelection,
    view: WeakEntity<Root>,
    cx: &App,
) -> impl IntoElement {
    let group = SharedString::from(format!("row-{key}"));
    let rename_selection = selection.clone();
    let delete_selection = selection;
    let danger = cx.theme().danger;
    let hover = cx.theme().danger_hover;
    let delete_group = SharedString::from(format!("delete-{key}"));
    div()
        .ml_auto()
        .flex()
        .flex_shrink_0()
        .items_center()
        .when(!selected, |this| {
            this.opacity(0.)
                .group_hover(group, |style| style.opacity(1.))
        })
        .child(
            Button::new(SharedString::from(format!("rename-{key}")))
                .ghost()
                .cursor_pointer()
                .small()
                .compact()
                .icon(IconName::PenLine)
                .tooltip(locale::text("toolbar.rename"))
                .accessibility_label(locale::text("toolbar.rename"))
                .on_click(on_root(&view, move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    if let Some(pane) = this.pane.as_mut() {
                        pane.tree_selection = rename_selection.clone();
                        pane.begin_rename(window, cx);
                    }
                    cx.notify();
                })),
        )
        .child(
            div()
                .id(SharedString::from(format!("delete-{key}")))
                .group(delete_group.clone())
                .size_6()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .rounded_sm()
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .active(move |style| style.bg(hover))
                .child(
                    svg()
                        .path(IconName::Trash.path())
                        .size_3p5()
                        .flex_shrink_0()
                        .text_color(danger)
                        .group_hover(delete_group, |style| style.text_color(black())),
                )
                .aria_label(locale::text("toolbar.delete"))
                .tooltip(|window, cx| {
                    Tooltip::new(locale::text("toolbar.delete")).build(window, cx)
                })
                .on_click(on_root(&view, move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    if let Some(pane) = this.pane.as_mut() {
                        pane.tree_selection = delete_selection.clone();
                        pane.begin_delete(window, cx);
                    }
                    cx.notify();
                })),
        )
}

pub(crate) fn files(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let selected = pane.tree_selection.clone();
    let entries = pane.tree.children.clone();
    let query_live = pane
        .search
        .as_ref()
        .is_some_and(|input| !input.read(cx).value().trim().is_empty());
    let sidebar = cx.theme().sidebar;
    let foreground = cx.theme().sidebar_foreground;
    div()
        .id("file-tree")
        .size_full()
        .min_h(px(0.))
        .min_w(crate::chrome::SIDEBAR_MIN_WIDTH)
        .overflow_hidden()
        .flex()
        .flex_col()
        .bg(sidebar)
        .text_color(foreground)
        .child(vault_heading(pane, cx))
        .when_some(pane.search.clone(), |column, input| {
            column.child(search_field(&input, view.clone(), cx))
        })
        .child(create_actions(view.clone(), cx))
        .child(file_list(
            pane,
            &entries,
            &selected,
            query_live,
            view.clone(),
            cx,
        ))
        .child(super::research::cards(pane, view.clone(), cx))
        .child(sidebar_footer(pane, view, cx))
}

fn sidebar_footer(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let border = cx.theme().sidebar_border;
    div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .border_t_1()
        .border_color(border)
        .py(FOOTER_PAD_Y)
        .child(
            footer_row(
                "tasks",
                IconName::ListTodo,
                locale::text("toolbar.tasks"),
                pane.tasks_open,
                cx,
            )
            .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.toggle_tasks(window, cx);
                }
                cx.notify();
            })),
        )
        .child(
            footer_row(
                "open-another",
                IconName::FolderOpen,
                locale::text("toolbar.open_vault"),
                false,
                cx,
            )
            .tooltip(|window, cx| {
                Tooltip::new(locale::text("toolbar.open_vault"))
                    .action(&OpenVault, None)
                    .build(window, cx)
            })
            .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
                this.pick_vault(false, cx);
            })),
        )
        .child(
            footer_row(
                "settings",
                IconName::Settings,
                locale::text("toolbar.settings"),
                pane.settings_open,
                cx,
            )
            .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.toggle_settings(window, cx);
                }
                cx.notify();
            })),
        )
}

/// One sidebar action, aligned with a note row.
fn footer_row(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    active: bool,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let hover = cx.theme().sidebar_accent;
    let sidebar = cx.theme().sidebar;
    let foreground = cx.theme().sidebar_foreground;
    let accent = cx.theme().sidebar_accent;
    let accent_foreground = cx.theme().sidebar_accent_foreground;
    div()
        .id(id)
        .pl(px(8.))
        .pr_2()
        .min_h(FOOTER_ROW_HEIGHT)
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .bg(if active { accent } else { sidebar })
        .text_color(if active {
            accent_foreground
        } else {
            foreground
        })
        .hover(move |style| style.bg(hover))
        .child(icon)
        .child(label)
}

fn file_list(
    pane: &EditorPane,
    entries: &[runebook::TreeEntry],
    selected: &TreeSelection,
    query_live: bool,
    view: WeakEntity<Root>,
    cx: &App,
) -> impl IntoElement {
    let collapsed = &pane.collapsed;
    let prompt = pane.prompt.as_ref();
    let capture = row_capture(pane);
    div()
        .id("file-list")
        .flex_1()
        .min_h(px(0.))
        .py_1()
        .when(!query_live, |list| {
            list.can_drop(|value, _, _| {
                value
                    .downcast_ref::<NoteMove>()
                    .is_some_and(|drag| !drag.id.parent().is_root())
            })
            .on_drop(on_root(&view, |this, drag: &NoteMove, _, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.move_note(&drag.id, &runebook::FolderId::root());
                }
                cx.notify();
            }))
            .children(entries.iter().cloned().map(|entry| {
                render_entry(
                    entry,
                    0,
                    &TreeFrame {
                        selected,
                        collapsed,
                        prompt,
                        capture: capture.as_ref(),
                        view: view.clone(),
                    },
                    cx,
                )
            }))
        })
        .when(query_live, |list| {
            let empty = pane.search_hits.is_empty();
            let truncated = pane.search_truncated;
            list.children(
                pane.search_hits
                    .iter()
                    .map(|hit| search_hit(hit, view.clone(), cx)),
            )
            .when(empty, |list| {
                list.child(search_status(locale::text("search.none"), cx))
            })
            .when(truncated, |list| {
                list.child(search_status(locale::text("search.more"), cx))
            })
        })
        .overflow_y_scrollbar()
}

fn search_field(input: &gpui_kit::Entity<InputState>, view: WeakEntity<Root>) -> impl IntoElement {
    div()
        .id("vault-search")
        .px_2()
        .py_1()
        .flex_shrink_0()
        .on_action(on_root(&view, |this, _: &Escape, window, cx| {
            cx.stop_propagation();
            if let Some(pane) = this.pane.as_mut() {
                pane.clear_search(window, cx);
            }
            cx.notify();
        }))
        .child(
            Input::new(input)
                .small()
                .cleanable(true)
                .prefix(IconName::Search)
                .w_full(),
        )
}

fn search_hit(hit: &runebook::SearchHit, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let id = hit.id.clone();
    let name = hit.id.name().to_string();
    let excerpt = hit.excerpt.clone();
    let muted = cx.theme().muted_foreground;
    let hover = cx.theme().sidebar_accent;
    let key = SharedString::from(format!("search-hit-{}", id.as_str()));
    div()
        .id(key)
        .px_2()
        .py_1()
        .min_h(px(28.))
        .w_full()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(
            div()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis_middle()
                .child(name),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis_middle()
                .child(excerpt),
        )
        .on_click(on_root(&view, move |this, _: &ClickEvent, window, cx| {
            this.open_note(id.clone(), window, cx);
        }))
}

fn search_status(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn vault_heading(pane: &EditorPane, cx: &App) -> impl IntoElement {
    let border = cx.theme().sidebar_border;
    panel_header(div().id("vault-heading").px_2().gap_2(), border)
        .on_mouse_down(MouseButton::Left, |_, window, _| {
            window.start_window_move();
        })
        .child(
            img(super::view::vault_mark())
                .h(px(18.))
                .w(px(16.))
                .flex_shrink_0(),
        )
        .child(
            div()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis_middle()
                .child(pane.vault.name().to_string()),
        )
}

fn create_actions(view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let border = cx.theme().sidebar_border;
    div()
        .id("tree-header")
        .w_full()
        .min_h(crate::chrome::FOOTER_ROW_HEIGHT)
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .pl(px(8.))
        .pr_2()
        .overflow_hidden()
        .border_b_1()
        .border_color(border)
        .child(
            Button::new("new-note")
                .ghost()
                .small()
                .flex_shrink_0()
                .cursor_pointer()
                .icon(IconName::FilePlus)
                .accessibility_label(locale::text("toolbar.new_note"))
                .tooltip_with_action(locale::text("toolbar.new_note"), &NewNote, None)
                .on_click(|_: &ClickEvent, window, cx| {
                    window.dispatch_action(Box::new(NewNote), cx);
                }),
        )
        .child(header_rule(border))
        .child(
            Button::new("new-folder")
                .ghost()
                .small()
                .flex_shrink_0()
                .cursor_pointer()
                .icon(IconName::FolderPlus)
                .accessibility_label(locale::text("toolbar.new_folder"))
                .tooltip(locale::text("toolbar.new_folder"))
                .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
                    if let Some(pane) = this.pane.as_mut() {
                        pane.begin_create_folder(window, cx);
                    }
                    cx.notify();
                })),
        )
        .child(header_rule(border))
        .child(
            Button::new("refresh-files")
                .ghost()
                .small()
                .flex_shrink_0()
                .cursor_pointer()
                .icon(IconName::RefreshCw)
                .accessibility_label(locale::text("toolbar.refresh"))
                .tooltip(locale::text("toolbar.refresh"))
                .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
                    if let Some(pane) = this.pane.as_mut() {
                        pane.refresh_from_disk(window, cx);
                    }
                    cx.notify();
                })),
        )
}

fn header_rule(border: gpui_kit::Hsla) -> impl IntoElement {
    div().h(px(16.)).w(px(1.)).flex_shrink_0().bg(border)
}

pub(super) fn note_icon(kind: NoteKind) -> IconName {
    match kind {
        NoteKind::Book => IconName::FileText,
        NoteKind::Speech => IconName::Mic,
        NoteKind::Sight => IconName::Video,
        NoteKind::Research => IconName::Search,
    }
}

/// Right-click menu that shows the folder in Finder.
///
/// The shell `open` command returns at once. A spawn error uses the notice line.
#[cfg(target_os = "macos")]
fn folder_finder_menu(
    row: impl gpui_kit::InteractiveElement + gpui_kit::ParentElement + gpui_kit::Styled,
    folder: runebook::FolderId,
    view: WeakEntity<Root>,
) -> impl IntoElement {
    row.context_menu(move |menu, _, _| {
        let folder = folder.clone();
        let view = view.clone();
        menu.item(
            PopupMenuItem::new(locale::text("tree.open_in_finder")).on_click(on_root(
                &view,
                move |this, _: &ClickEvent, _, cx| {
                    let Some(pane) = this.pane.as_ref() else {
                        return;
                    };
                    let path = super::folder_on_disk(pane.root(), &folder);
                    if let Err(err) = crate::markdown_view::spawn_desktop_open(&path) {
                        this.set_notice(locale::format(
                            "tree.open_finder_failed",
                            &[("err", &err.to_string())],
                        ));
                        cx.notify();
                    }
                },
            )),
        )
    })
}

struct TreeFrame<'a> {
    selected: &'a TreeSelection,
    collapsed: &'a std::collections::HashSet<runebook::FolderId>,
    prompt: Option<&'a Prompt>,
    capture: Option<&'a (runebook::NoteId, RowCapture)>,
    view: WeakEntity<Root>,
}

fn render_entry(
    entry: runebook::TreeEntry,
    depth: usize,
    frame: &TreeFrame<'_>,
    cx: &App,
) -> gpui_kit::AnyElement {
    let selected = frame.selected;
    let collapsed = frame.collapsed;
    let prompt = frame.prompt;
    let capture = frame.capture;
    let view = frame.view.clone();
    let pad = px(8.0 + depth as f32 * 12.0);
    let sidebar = cx.theme().sidebar;
    let accent = cx.theme().sidebar_accent;
    let foreground = cx.theme().sidebar_foreground;
    let accent_foreground = cx.theme().sidebar_accent_foreground;
    match entry {
        runebook::TreeEntry::Folder { id, name, children } => {
            let open = matches!(selected, TreeSelection::Folder(current) if current == &id);
            let folded = collapsed.contains(&id);
            let key = format!("folder-{}", id.as_str());
            let folder_id = id.clone();
            let renaming = rename_input(prompt, &TreeSelection::Folder(id.clone()));
            let select_id = folder_id.clone();
            let row = div()
                .id(SharedString::from(key.clone()))
                .group(SharedString::from(format!("row-{key}")))
                .pl(pad)
                .pr_2()
                .min_h(px(28.))
                .w_full()
                .min_w(px(0.))
                .flex()
                .items_center()
                .gap_2()
                .bg(if open { accent } else { sidebar })
                .text_color(if open { accent_foreground } else { foreground })
                .cursor_pointer()
                .child(folder_toggle(&key, folded, folder_id.clone(), view.clone()))
                .child(if folded {
                    IconName::Folder
                } else {
                    IconName::FolderOpen
                })
                .child(row_label(&key, name, renaming))
                .when(renaming.is_none(), |row| {
                    let select_id = select_id.clone();
                    row.child(row_actions(
                        &key,
                        open,
                        TreeSelection::Folder(id),
                        view.clone(),
                        cx,
                    ))
                    .on_click(on_root(
                        &view,
                        move |this, _: &ClickEvent, _, cx| {
                            if let Some(pane) = this.pane.as_mut() {
                                pane.select_tree_folder(select_id.clone());
                            }
                            cx.notify();
                        },
                    ))
                })
                .when(renaming.is_some(), |row| rename_keys(row, view.clone()))
                .can_drop({
                    let folder = folder_id.clone();
                    move |value, _, _| {
                        value
                            .downcast_ref::<NoteMove>()
                            .is_some_and(|drag| drag.id.parent() != folder)
                    }
                })
                .drag_over::<NoteMove>(|style, _, _, cx| style.bg(cx.theme().sidebar_accent))
                .on_drop({
                    let folder = folder_id.clone();
                    on_root(&view, move |this, drag: &NoteMove, _, cx| {
                        if let Some(pane) = this.pane.as_mut() {
                            pane.move_note(&drag.id, &folder);
                        }
                        cx.notify();
                    })
                });
            #[cfg(target_os = "macos")]
            let row = folder_finder_menu(row, folder_id, view.clone());
            div()
                .child(row)
                .when(!folded, |row| {
                    row.children(
                        children
                            .into_iter()
                            .map(|child| render_entry(child, depth + 1, frame, cx)),
                    )
                })
                .into_any_element()
        }
        runebook::TreeEntry::Note { id, name, kind } => {
            let open = matches!(selected, TreeSelection::Note(current) if current == &id);
            let key = format!("note-{}", id.as_str());
            let note_id = id.clone();
            let renaming = rename_input(prompt, &TreeSelection::Note(id.clone()));
            let live =
                capture.and_then(|(note, kind_capture)| (note == &id).then_some(*kind_capture));
            div()
                .id(SharedString::from(key.clone()))
                .group(SharedString::from(format!("row-{key}")))
                .pl(pad)
                .pr_2()
                .min_h(px(28.))
                .w_full()
                .min_w(px(0.))
                .flex()
                .items_center()
                .gap_2()
                .bg(if open { accent } else { sidebar })
                .text_color(if open { accent_foreground } else { foreground })
                .cursor_pointer()
                .child(match live {
                    Some(kind_capture) => {
                        record_mark(kind_capture, &key, view.clone(), cx).into_any_element()
                    }
                    None => note_icon(kind).into_any_element(),
                })
                .child(row_label(&key, name, renaming))
                .when(renaming.is_none(), |row| {
                    row.child(row_actions(
                        &key,
                        open,
                        TreeSelection::Note(id),
                        view.clone(),
                        cx,
                    ))
                    .on_drag(
                        NoteMove {
                            id: note_id.clone(),
                        },
                        |drag, _, _, cx| {
                            let text = SharedString::from(drag.id.name());
                            cx.new(|_| DragChip { text })
                        },
                    )
                    .on_click(on_root(
                        &view,
                        move |this, _: &ClickEvent, window, cx| {
                            this.open_note(note_id.clone(), window, cx);
                        },
                    ))
                })
                .when(renaming.is_some(), |row| rename_keys(row, view.clone()))
                .into_any_element()
        }
    }
}

fn rename_input<'a>(
    prompt: Option<&'a Prompt>,
    target: &TreeSelection,
) -> Option<&'a gpui_kit::Entity<InputState>> {
    let Prompt::Rename { input, action } = prompt?;
    let hit = match (action, target) {
        (NameAction::RenameNote(id), TreeSelection::Note(current)) => id == current,
        (NameAction::RenameFolder(id), TreeSelection::Folder(current)) => id == current,
        _ => false,
    };
    hit.then_some(input)
}

fn row_label(
    key: &str,
    name: String,
    renaming: Option<&gpui_kit::Entity<InputState>>,
) -> impl IntoElement {
    if let Some(input) = renaming {
        div()
            .id(SharedString::from(format!("rename-field-{key}")))
            .flex_1()
            .min_w(px(0.))
            .on_click(|_: &ClickEvent, _, cx| {
                cx.stop_propagation();
            })
            .child(Input::new(input).small().w_full())
            .into_any_element()
    } else {
        div()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis_middle()
            .child(name)
            .into_any_element()
    }
}

fn rename_keys<E>(row: E, view: WeakEntity<Root>) -> E
where
    E: gpui_kit::StatefulInteractiveElement,
{
    row.on_action(on_root(&view, |this, _: &Enter, window, cx| {
        cx.stop_propagation();
        if let Some(pane) = this.pane.as_mut() {
            pane.submit_prompt(window, cx);
        }
        cx.notify();
    }))
    .on_action(on_root(&view, |this, _: &Escape, _, cx| {
        cx.stop_propagation();
        if let Some(pane) = this.pane.as_mut() {
            pane.cancel_prompt();
        }
        cx.notify();
    }))
}

fn folder_toggle(
    key: &str,
    folded: bool,
    folder: runebook::FolderId,
    view: WeakEntity<Root>,
) -> impl IntoElement {
    let icon = if folded {
        IconName::ChevronRight
    } else {
        IconName::ChevronDown
    };
    div()
        .id(SharedString::from(format!("collapse-{key}")))
        .flex_shrink_0()
        .cursor_pointer()
        .child(icon)
        .on_click(on_root(&view, move |this, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            if let Some(pane) = this.pane.as_mut() {
                pane.toggle_folder(folder.clone());
            }
            cx.notify();
        }))
}

#[derive(Clone)]
struct NoteMove {
    id: runebook::NoteId,
}

struct DragChip {
    text: SharedString,
}

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(cx.theme().sidebar_accent)
            .text_color(cx.theme().sidebar_accent_foreground)
            .child(self.text.clone())
    }
}
