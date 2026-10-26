//! Layout for the open vault: toolbar, tree, editor, and preview.

use std::sync::{Arc, OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::input::{Editor, Enter, Escape, Input, InputContentType, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::Select;
use gpui_kit::component::toolbar::Toolbar;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Selectable, Sizable, Theme};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AppContext, ClickEvent, Context, Entity, Image, ImageFormat, Render, SharedString,
    WeakEntity, Window, black, div, img, px, relative, svg,
};

use super::{EditorPane, NameAction, NoteView, Prompt, TreeSelection};
use crate::chrome::panel_header;
use runebook::{NoteKind, ThemeChoice};

use crate::capture::{ScreenTarget, list_record_sources};
use crate::{NewNote, OpenVault, Root, ToggleNoteView, on_root, theme_mode};

pub(crate) fn toolbar(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    Toolbar::new("vault-toolbar")
        .w_full()
        .pl_0()
        .content(brand(pane))
        .content(div().px_1().text_color(muted).child("-"))
        .child(
            Button::new("open-another")
                .ghost()
                .cursor_pointer()
                .icon(IconName::FolderOpen)
                .label("Open vault")
                .tooltip_with_action("Open vault", &OpenVault, None)
                .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
                    this.pick_vault(false, cx);
                })),
        )
        .content(div().flex_1())
        .content(capture_buttons(pane, view, cx))
}

fn archivist_button(pane: &EditorPane, view: WeakEntity<Root>) -> impl IntoElement {
    let label = if pane.archivist.is_open() {
        "Hide the Archivist"
    } else {
        "Show the Archivist"
    };
    Button::new("toggle-archivist")
        .ghost()
        .small()
        .cursor_pointer()
        .selected(pane.archivist.is_open())
        .icon(IconName::Bot)
        .label("Archivist")
        .tooltip(label)
        .accessibility_label(label)
        .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
            if let Some(pane) = this.pane.as_mut() {
                pane.archivist.toggle();
            }
            cx.notify();
        }))
}

fn capture_buttons(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let voice_on = pane.voice_recording();
    let screen_on = pane.screen_recording();
    let busy = pane.capture_status().is_some();
    let voice_label = if voice_on {
        "Stop voice"
    } else if pane.voice_allowed() {
        "Record voice"
    } else {
        "Open a Speech note to record voice"
    };
    let screen_label = if screen_on {
        "Stop screen"
    } else if pane.sight_allowed() {
        "Record screen"
    } else {
        "Open a Sight note to record the screen"
    };
    let voice = Button::new("record-voice")
        .cursor_pointer()
        .disabled(!voice_on && (busy || !pane.voice_allowed()))
        .icon(if voice_on {
            IconName::CircleDot
        } else {
            IconName::Mic
        })
        .tooltip(voice_label)
        .accessibility_label(voice_label)
        .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
            if let Some(pane) = this.pane.as_mut() {
                pane.toggle_voice(cx);
            }
            cx.notify();
        }));
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(recording_style(voice, voice_on, cx))
        .child(screen_button(
            screen_on,
            busy || !pane.sight_allowed(),
            screen_label,
            view,
            cx,
        ))
}

/// A live capture stays the size of the other toolbar icons and uses a red record mark.
fn recording_style(button: Button, live: bool, cx: &App) -> Button {
    if !live {
        return button.ghost();
    }
    let danger = cx.theme().danger;
    let hover = cx.theme().secondary_hover;
    button.custom(
        ButtonCustomVariant::new(cx)
            .foreground(danger)
            .hover(hover)
            .active(hover),
    )
}

fn screen_button(
    screen_on: bool,
    busy: bool,
    screen_label: &str,
    view: WeakEntity<Root>,
    cx: &App,
) -> gpui_kit::AnyElement {
    let button = Button::new("record-screen")
        .cursor_pointer()
        .icon(if screen_on {
            IconName::CircleDot
        } else {
            IconName::Video
        })
        .tooltip(screen_label.to_string())
        .accessibility_label(screen_label.to_string());
    if screen_on {
        return recording_style(button, true, cx)
            .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.toggle_screen(cx);
                }
                cx.notify();
            }))
            .into_any_element();
    }
    recording_style(button, false, cx)
        .disabled(busy)
        .dropdown_menu(move |menu, _, menu_cx| {
            let windows = list_record_sources();
            let failed = windows.is_err();
            view.update(menu_cx, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.note_window_list(failed);
                }
                cx.notify();
            })
            .ok();
            let mut menu = menu.item(screen_target_item(
                view.clone(),
                "Full display",
                ScreenTarget::Display,
            ));
            if let Ok(windows) = windows {
                for window in windows {
                    menu = menu.item(screen_target_item(
                        view.clone(),
                        window.title,
                        window.target,
                    ));
                }
            }
            menu
        })
        .into_any_element()
}

fn screen_target_item(
    view: gpui_kit::WeakEntity<Root>,
    label: impl Into<SharedString>,
    target: ScreenTarget,
) -> PopupMenuItem {
    PopupMenuItem::new(label).on_click(move |_, _, app| {
        let target = target.clone();
        view.update(app, |root, cx| {
            if let Some(pane) = root.pane.as_mut() {
                pane.start_screen(target);
            }
            cx.notify();
        })
        .ok();
    })
}

fn toggle_note_view(pane: &EditorPane, view: WeakEntity<Root>) -> impl IntoElement {
    let (icon, label) = match pane.note_view {
        NoteView::Preview => (IconName::SquarePen, "Edit source"),
        NoteView::Source => (IconName::Eye, "Show preview"),
    };
    Button::new("toggle-note-view")
        .ghost()
        .cursor_pointer()
        .icon(icon)
        .tooltip_with_action(label, &ToggleNoteView, None)
        .accessibility_label(label)
        .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
            if let Some(pane) = this.pane.as_mut() {
                pane.toggle_note_view(window, cx);
            }
            cx.notify();
        }))
}

pub(crate) fn note(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> gpui_kit::AnyElement {
    if pane.settings_open {
        return settings_panel(pane, view, cx).into_any_element();
    }
    if pane.research.wizard_open() {
        return super::research::wizard(pane, view, cx).into_any_element();
    }
    note_surface(pane, view, cx)
}

fn note_surface(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> gpui_kit::AnyElement {
    let body = if pane.research_note_open() {
        super::research::report(pane, view.clone(), cx).into_any_element()
    } else {
        match pane.note_view {
            NoteView::Preview => super::preview::surface(pane, view.clone(), cx).into_any_element(),
            NoteView::Source => source_surface(pane).into_any_element(),
        }
    };
    let border = cx.theme().border;
    div()
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(note_header(pane, border, view, cx))
        .child(body)
        .into_any_element()
}

fn note_header(
    pane: &EditorPane,
    border: gpui_kit::Hsla,
    view: WeakEntity<Root>,
    _cx: &App,
) -> impl IntoElement {
    panel_header(div().id("note-header").px_2(), border)
        .child(archivist_button(pane, view.clone()))
        .child(div().flex_1())
        .when(!pane.research_note_open(), |header| {
            header.child(toggle_note_view(pane, view))
        })
}

fn source_surface(pane: &EditorPane) -> impl IntoElement {
    div()
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            Editor::new(&pane.editor)
                .bordered(false)
                .readonly(pane.open_note.is_none())
                .h(relative(1.))
                .size_full()
                .flex_1(),
        )
}

fn brand(pane: &EditorPane) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .pr_2()
        .child(img(mark()).h(px(20.)).w(px(14.)).flex_shrink_0())
        .child(pane.vault.name().to_string())
}

fn mark() -> Arc<Image> {
    static MARK: OnceLock<Arc<Image>> = OnceLock::new();
    MARK.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Svg,
            include_bytes!("../../../../assets/icon.svg").to_vec(),
        ))
    })
    .clone()
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
                .tooltip("Rename")
                .accessibility_label("Rename")
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
                .aria_label("Delete")
                .tooltip(|window, cx| Tooltip::new("Delete").build(window, cx))
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
    let sidebar = cx.theme().sidebar;
    let foreground = cx.theme().sidebar_foreground;
    div()
        .id("file-tree")
        .size_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .overflow_hidden()
        .flex()
        .flex_col()
        .bg(sidebar)
        .text_color(foreground)
        .child(create_actions(view.clone(), cx))
        .child(
            div()
                .id("file-list")
                .flex_1()
                .min_h(px(0.))
                .py_1()
                .can_drop(|value, _, _| {
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
                .children(entries.into_iter().map(|entry| {
                    render_entry(
                        entry,
                        0,
                        &selected,
                        &pane.collapsed,
                        pane.prompt.as_ref(),
                        view.clone(),
                        cx,
                    )
                }))
                .overflow_y_scrollbar(),
        )
        .child(super::research::cards(pane, view.clone(), cx))
        .child(settings_row(view, cx))
}

fn settings_row(view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let border = cx.theme().sidebar_border;
    div()
        .w_full()
        .flex_shrink_0()
        .border_t_1()
        .border_color(border)
        .px_1()
        .py_1()
        .child(
            Button::new("settings")
                .ghost()
                .small()
                .w_full()
                .cursor_pointer()
                .icon(IconName::Settings)
                .label("Settings")
                .on_click(on_root(&view, |this, _: &ClickEvent, _, cx| {
                    if let Some(pane) = this.pane.as_mut() {
                        pane.toggle_settings(cx);
                    }
                    cx.notify();
                })),
        )
}

fn settings_panel(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    div()
        .id("settings-panel")
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p_4()
        .child(
            div()
                .w_full()
                .max_w(px(420.))
                .flex()
                .flex_col()
                .gap_4()
                .child(setting_label("Microphone", muted))
                .child(microphone_field(pane, cx))
                .child(setting_label("Theme", muted))
                .child(theme_group(pane, view.clone()))
                .child(setting_label("Note view", muted))
                .child(note_view_group(pane, view))
                .child(div().text_sm().child("Models"))
                .child(setting_label("Tool rounds", muted))
                .child(text_setting(&pane.model_inputs.tool_rounds))
                .child(setting_label("Groq API key", muted))
                .child(secret_setting(&pane.model_inputs.groq_api_key))
                .child(setting_label("OpenAI-compatible API key", muted))
                .child(secret_setting(&pane.model_inputs.openai_api_key))
                .child(setting_label("Hostname", muted))
                .child(text_setting(&pane.model_inputs.openai_base_url))
                .child(setting_label("Research model", muted))
                .child(text_setting(&pane.model_inputs.openai_model))
                .child(setting_label("Archivist model", muted))
                .child(text_setting(&pane.model_inputs.archivist_model)),
        )
}

fn text_setting(state: &Entity<InputState>) -> impl IntoElement {
    Input::new(state).w_full()
}

fn secret_setting(state: &Entity<InputState>) -> impl IntoElement {
    Input::new(state)
        .w_full()
        .content_type(InputContentType::Password)
        .mask_toggle()
}

fn setting_label(text: &'static str, color: impl Into<gpui_kit::Hsla>) -> impl IntoElement {
    div().text_xs().text_color(color).child(text)
}

fn microphone_field(pane: &EditorPane, cx: &App) -> gpui_kit::AnyElement {
    let Some(select) = pane.microphone_select.as_ref() else {
        return microphone_loading(cx).into_any_element();
    };
    Select::new(select)
        .id("microphone")
        .w_full()
        .cursor_pointer()
        .into_any_element()
}

fn microphone_loading(cx: &App) -> impl IntoElement {
    let border = cx.theme().border;
    let muted = cx.theme().muted_foreground;
    div()
        .id("microphone")
        .w_full()
        .h(px(32.))
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(border)
        .text_sm()
        .text_color(muted)
        .child("loading...")
}

fn theme_group(pane: &EditorPane, view: WeakEntity<Root>) -> RadioGroup {
    let selected = match pane.theme {
        ThemeChoice::Dark => 0,
        ThemeChoice::Light => 1,
    };
    RadioGroup::vertical("theme")
        .selected_index(Some(selected))
        .child(Radio::new("theme-dark").label("Dark").cursor_pointer())
        .child(Radio::new("theme-light").label("Light").cursor_pointer())
        .on_change(move |index, window, app| {
            let choice = match *index {
                0 => ThemeChoice::Dark,
                1 => ThemeChoice::Light,
                _ => return,
            };
            Theme::change(theme_mode(choice), Some(window), app);
            view.update(app, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.choose_theme(choice);
                }
                cx.notify();
            })
            .ok();
        })
}

fn note_view_group(pane: &EditorPane, view: WeakEntity<Root>) -> RadioGroup {
    let selected = match pane.default_note_view {
        NoteView::Preview => 0,
        NoteView::Source => 1,
    };
    RadioGroup::vertical("note-view")
        .selected_index(Some(selected))
        .child(Radio::new("view-preview").label("Preview").cursor_pointer())
        .child(Radio::new("view-source").label("Source").cursor_pointer())
        .on_change(move |index, window, app| {
            let next = match *index {
                0 => NoteView::Preview,
                1 => NoteView::Source,
                _ => return,
            };
            // The click already holds this window. Do not look the window up again.
            view.update(app, |root, cx| {
                if let Some(pane) = root.pane.as_mut() {
                    pane.choose_note_view(next, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
}

fn create_actions(view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let border = cx.theme().sidebar_border;
    panel_header(div().id("tree-header").justify_center().px_1(), border)
        .child(
            Button::new("new-note")
                .ghost()
                .small()
                .cursor_pointer()
                .icon(IconName::FilePlus)
                .label("New note")
                .tooltip_with_action("New note", &NewNote, None)
                .dropdown_menu({
                    let view = view.clone();
                    move |menu, _, _| {
                        menu.item(new_kind_item(view.clone(), NoteKind::Book))
                            .item(new_kind_item(view.clone(), NoteKind::Speech))
                            .item(new_kind_item(view.clone(), NoteKind::Sight))
                            .item(new_kind_item(view.clone(), NoteKind::Deed))
                            .item(new_kind_item(view.clone(), NoteKind::Research))
                    }
                }),
        )
        .child(div().mx_2().h(px(16.)).w(px(1.)).flex_shrink_0().bg(border))
        .child(
            Button::new("new-folder")
                .ghost()
                .small()
                .cursor_pointer()
                .icon(IconName::FolderPlus)
                .label("New folder")
                .tooltip("New folder")
                .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
                    if let Some(pane) = this.pane.as_mut() {
                        pane.begin_create_folder(window, cx);
                    }
                    cx.notify();
                })),
        )
}

fn new_kind_item(view: gpui_kit::WeakEntity<Root>, kind: NoteKind) -> PopupMenuItem {
    PopupMenuItem::new(super::kind_title(kind)).on_click(move |_, window, app| {
        view.update(app, |root, cx| {
            if let Some(pane) = root.pane.as_mut() {
                pane.prompt_new_note(kind, window, cx);
            }
            cx.notify();
        })
        .ok();
    })
}

fn note_icon(kind: NoteKind) -> IconName {
    match kind {
        NoteKind::Book => IconName::FileText,
        NoteKind::Speech => IconName::Mic,
        NoteKind::Sight => IconName::Video,
        NoteKind::Deed => IconName::ListTodo,
        NoteKind::Research => IconName::Search,
    }
}

fn render_entry(
    entry: runebook::TreeEntry,
    depth: usize,
    selected: &TreeSelection,
    collapsed: &std::collections::HashSet<runebook::FolderId>,
    prompt: Option<&Prompt>,
    view: WeakEntity<Root>,
    cx: &App,
) -> gpui_kit::AnyElement {
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
            div()
                .child(
                    div()
                        .id(SharedString::from(key.clone()))
                        .group(SharedString::from(format!("row-{key}")))
                        .pl(pad)
                        .pr_2()
                        .py(px(4.))
                        .w_full()
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
                        .drag_over::<NoteMove>(|style, _, _, cx| {
                            style.bg(cx.theme().sidebar_accent)
                        })
                        .on_drop({
                            let folder = folder_id.clone();
                            on_root(&view, move |this, drag: &NoteMove, _, cx| {
                                if let Some(pane) = this.pane.as_mut() {
                                    pane.move_note(&drag.id, &folder);
                                }
                                cx.notify();
                            })
                        }),
                )
                .when(!folded, |row| {
                    row.children(children.into_iter().map(|child| {
                        render_entry(
                            child,
                            depth + 1,
                            selected,
                            collapsed,
                            prompt,
                            view.clone(),
                            cx,
                        )
                    }))
                })
                .into_any_element()
        }
        runebook::TreeEntry::Note { id, name, kind } => {
            let open = matches!(selected, TreeSelection::Note(current) if current == &id);
            let key = format!("note-{}", id.as_str());
            let note_id = id.clone();
            let renaming = rename_input(prompt, &TreeSelection::Note(id.clone()));
            div()
                .id(SharedString::from(key.clone()))
                .group(SharedString::from(format!("row-{key}")))
                .pl(pad)
                .pr_2()
                .py(px(4.))
                .w_full()
                .flex()
                .items_center()
                .gap_2()
                .bg(if open { accent } else { sidebar })
                .text_color(if open { accent_foreground } else { foreground })
                .cursor_pointer()
                .child(note_icon(kind))
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

pub(crate) fn status_bar(pane: &EditorPane, cx: &App) -> impl IntoElement {
    let note = pane
        .open_note
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "No note".to_string());
    let state = if pane.dirty { "Unsaved" } else { "Saved" };
    let mut text = format!("{} · {note} · {state}", pane.vault.root().display());
    if let Some(status) = pane.capture_status() {
        text.push_str(" · ");
        text.push_str(status);
    }
    if let Some(notice) = &pane.notice {
        text.push_str(" · ");
        text.push_str(notice);
    }
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    div()
        .px_2()
        .py_1()
        .text_xs()
        .text_color(muted)
        .border_t_1()
        .border_color(border)
        .child(text)
}
