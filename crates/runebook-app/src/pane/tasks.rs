//! Tasks from every note, and the dialog that writes one task line.
//!
//! The list is rebuilt from the vault index. The open note uses the editor body.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::rc::Rc;

use chrono::NaiveDate;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants};
use gpui_kit::component::date_picker::{DatePicker, DatePickerState};
use gpui_kit::component::input::{Escape, Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, Sizable, WindowExt};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, ClickEvent, Context, Focusable, Hsla, MouseButton, MouseDownEvent, MouseUpEvent,
    SharedString, WeakEntity, Window, div, px, svg,
};

use super::names::dialog_footer;
use super::{EditorPane, NoteView};
use crate::chrome::panel_header;
use crate::locale;
use crate::{Root, on_root};
use runebook::{
    NoteId, TaskDate, TaskRef, body_line, collect_tasks, insert_task_at, new_task_line,
    parse_task_line, replace_line, rewrite_task, toggle_body_line,
};

const TASK_CAP: usize = 200;
const TEXT_LIMIT: usize = 512;

/// The local calendar day. Tests pass a fixed date into the vault helpers.
pub(super) fn local_today() -> TaskDate {
    let date = jiff::Zoned::now().date();
    let year = u16::try_from(date.year()).unwrap_or(1);
    let month = u8::try_from(date.month()).unwrap_or(1);
    let day = u8::try_from(date.day()).unwrap_or(1);
    TaskDate::from_ymd(year, month, day)
        .unwrap_or_else(|| TaskDate::from_ymd(1970, 1, 1).expect("1970-01-01 is a calendar day"))
}

impl EditorPane {
    /// Open or close the task list. Opening it closes Settings.
    pub(super) fn toggle_tasks(&mut self, _window: &mut Window, _cx: &mut Context<Root>) {
        self.tasks_open = !self.tasks_open;
        if self.tasks_open && self.settings_open {
            self.settings_open = false;
            self.close_keys();
        }
    }

    /// Ask for a task, then write it into the open note.
    pub(crate) fn begin_task(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if self.open_note.is_none() {
            self.notice = Some(locale::text("media.open_note").to_string());
            return;
        }
        if let Err(err) = self.require_idle() {
            self.notice = Some(err);
            return;
        }
        let target = task_target(self, cx);
        open_task_dialog(target, window, cx);
    }

    /// Write the dialog into the open note.
    ///
    /// Returns whether a save should wait.
    pub(super) fn apply_task(
        &mut self,
        target: &TaskTarget,
        description: &str,
        due: &str,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> Result<bool, String> {
        let (text, due) = draft(description, due)?;
        let next = match target.edit {
            TaskEdit::Insert { at } => {
                let line = new_task_line(&text, due);
                insert_task_at(&self.latest, at, &line)
            }
            TaskEdit::Replace { line } => {
                let current = body_line(&self.latest, line)
                    .ok_or_else(|| locale::text("tasks.missing").to_string())?;
                let rewritten = rewrite_task(current, &text, due)
                    .ok_or_else(|| locale::text("tasks.missing").to_string())?;
                replace_line(&self.latest, line, &rewritten)
                    .ok_or_else(|| locale::text("tasks.missing").to_string())?
            }
        };
        Ok(self.replace_body(next, window, cx))
    }

    /// Flip one task. The open note changes in the editor. A closed note is written.
    ///
    /// Returns whether a save of the open note should wait.
    pub(super) fn toggle_listed_task(
        &mut self,
        id: &NoteId,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        let today = local_today();
        if self.open_note.as_ref() == Some(id) {
            let Some(next) = toggle_body_line(&self.latest, line, today) else {
                return false;
            };
            return self.replace_body(next, window, cx);
        }
        if let Err(err) = self.require_idle() {
            self.notice = Some(err);
            return false;
        }
        let Ok(file) = self.vault.read(id) else {
            self.notice = Some(locale::text("archivist.read_failed").to_string());
            return false;
        };
        let (kind, body) = runebook::decode_note(&file);
        let Some(next_body) = toggle_body_line(body, line, today) else {
            return false;
        };
        let next = runebook::encode_note(kind, &next_body);
        if let Err(err) = self.vault.write(id, &next) {
            self.notice = Some(err.to_string());
            return false;
        }
        if let Err(err) = self.reload() {
            self.notice = Some(err.to_string());
        }
        false
    }
}

/// Where the dialog writes, plus the fields it shows.
#[derive(Clone)]
pub(super) struct TaskTarget {
    edit: TaskEdit,
    description: String,
    due: String,
}

#[derive(Clone, Copy)]
enum TaskEdit {
    Insert { at: usize },
    Replace { line: u32 },
}

pub(super) fn panel(pane: &EditorPane, view: WeakEntity<Root>, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    let border = cx.theme().border;
    let background = cx.theme().background;
    let foreground = cx.theme().foreground;
    let (rows, more) = arrange(gather(pane), pane.tasks_show_done);
    let empty = rows.is_empty();
    div()
        .id("tasks-panel")
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(background)
        .text_color(foreground)
        .child(header(pane, view.clone(), border, cx))
        .child(
            div()
                .id("tasks-body")
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .children(
                    rows.into_iter()
                        .map(|row| task_row(row, muted, view.clone(), cx)),
                )
                .when(empty, |column| {
                    column.child(
                        div()
                            .p_2()
                            .text_color(muted)
                            .child(locale::text("tasks.empty")),
                    )
                })
                .when(more, |column| {
                    column.child(
                        div()
                            .p_2()
                            .text_color(muted)
                            .child(locale::text("tasks.more")),
                    )
                }),
        )
}

fn header(
    pane: &EditorPane,
    view: WeakEntity<Root>,
    border: impl Into<gpui_kit::Hsla>,
    cx: &App,
) -> impl IntoElement {
    let show = view.clone();
    panel_header(div().id("tasks-header").px_2().gap_2(), border.into())
        .child(div().px_2().child(locale::text("tasks.title")))
        .child(
            Switch::new("tasks-show-done")
                .checked(pane.tasks_show_done)
                .cursor_pointer()
                .accessibility_label(locale::text("tasks.show_done"))
                .tooltip(locale::text("tasks.show_done.desc"))
                .on_change(move |on, _, cx| {
                    show.update(cx, |root, cx| {
                        if let Some(pane) = root.pane.as_mut() {
                            pane.tasks_show_done = *on;
                        }
                        cx.notify();
                    })
                    .ok();
                }),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(locale::text("tasks.show_done")),
        )
        .child(div().flex_1())
        .child(
            Button::new("tasks-done")
                .ghost()
                .small()
                .cursor_pointer()
                .label(locale::text("settings.done"))
                .on_click(on_root(&view, |this, _: &ClickEvent, window, cx| {
                    if let Some(pane) = this.pane.as_mut() {
                        pane.toggle_tasks(window, cx);
                    }
                    cx.notify();
                })),
        )
}

fn task_row(
    row: Listed,
    muted: impl Into<gpui_kit::Hsla> + Copy,
    view: WeakEntity<Root>,
    cx: &App,
) -> impl IntoElement {
    let muted = muted.into();
    let hover = cx.theme().muted;
    let border = cx.theme().foreground;
    let empty = cx.theme().background;
    let id = row.id.clone();
    let line = row.line;
    let open_id = id.clone();
    div()
        .id(SharedString::from(format!(
            "task-row-{}-{line}",
            id.as_str()
        )))
        .w_full()
        .min_h(px(32.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded_md()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(on_root(&view, move |this, _: &ClickEvent, window, cx| {
            this.open_note(open_id.clone(), window, cx);
        }))
        .child(task_box(row.checked, id, line, border, empty, view))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(row_label(&row)),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_sm()
                .text_color(muted)
                .child(row.id.as_str().to_string()),
        )
}

fn row_label(row: &Listed) -> String {
    match row.due {
        Some(due) if row.text.is_empty() => due.to_string(),
        Some(due) => format!("{} · {due}", row.text),
        None => row.text.clone(),
    }
}

fn task_box(
    checked: bool,
    id: NoteId,
    line: u32,
    border: Hsla,
    empty: Hsla,
    view: WeakEntity<Root>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!(
            "task-box-{}-{line}",
            id.as_str()
        )))
        .size(px(16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(border)
        .bg(if checked { border } else { empty })
        .cursor_pointer()
        .when(checked, |mark| {
            mark.child(
                svg()
                    .path(IconName::Check.path())
                    .size(px(12.))
                    .text_color(empty),
            )
        })
        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
            cx.stop_propagation();
        })
        .on_click(on_root(&view, move |this, _: &ClickEvent, window, cx| {
            cx.stop_propagation();
            let dirty = this
                .pane
                .as_mut()
                .is_some_and(|pane| pane.toggle_listed_task(&id, line, window, cx));
            if dirty {
                this.schedule_autosave(cx);
            }
            cx.notify();
        }))
}

fn open_task_dialog(target: TaskTarget, window: &mut Window, cx: &mut Context<Root>) {
    let view = cx.weak_entity();
    let description = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(locale::text("tasks.description"))
            .default_value(target.description.clone())
    });
    let due = cx.new(|cx| {
        let mut state = DatePickerState::new(window, cx).date_format("%Y-%m-%d");
        if let Some(date) = naive_due(&target.due) {
            state.set_date(date, window, cx);
        }
        state
    });
    let error = Rc::new(RefCell::new(None::<String>));
    let initial_focus = Cell::new(true);
    let title = if matches!(target.edit, TaskEdit::Replace { .. }) {
        locale::text("tasks.edit")
    } else {
        locale::text("tasks.insert")
    };
    let confirm = if matches!(target.edit, TaskEdit::Replace { .. }) {
        locale::text("tasks.save")
    } else {
        locale::text("dialog.create")
    };
    window.open_alert_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            description.focus_handle(cx).focus(window, cx);
        }
        let message = error.borrow().clone();
        let danger = cx.theme().danger;
        let description_ok = description.clone();
        let due_ok = due.clone();
        let error_ok = error.clone();
        let target = target.clone();
        let view = view.clone();
        dialog
            .title(title)
            .confirm()
            .width(px(420.))
            .footer(dialog_footer(
                locale::text("dialog.cancel"),
                confirm,
                ButtonVariant::Primary,
            ))
            .child(
                div()
                    .id("task-dialog")
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_action(|_: &Escape, window, cx| {
                        cx.stop_propagation();
                        window.close_dialog(cx);
                    })
                    .child(Input::new(&description).w_full())
                    .child(
                        DatePicker::new(&due)
                            .placeholder(locale::text("tasks.due"))
                            .cleanable(true)
                            .w_full(),
                    )
                    .when_some(message, |column, message| {
                        column.child(div().text_sm().text_color(danger).child(message))
                    }),
            )
            .on_ok(move |_event, window, cx| {
                let description = description_ok.read(cx).value().to_string();
                let due = due_ok
                    .read(cx)
                    .date()
                    .format("%Y-%m-%d")
                    .map(|text| text.to_string())
                    .unwrap_or_default();
                view.update(cx, |root, cx| {
                    let Some(pane) = root.pane.as_mut() else {
                        return true;
                    };
                    match pane.apply_task(&target, &description, &due, window, cx) {
                        Ok(dirty) => {
                            if dirty {
                                root.schedule_autosave(cx);
                            }
                            cx.notify();
                            true
                        }
                        Err(err) => {
                            *error_ok.borrow_mut() = Some(err);
                            cx.notify();
                            false
                        }
                    }
                })
                .unwrap_or(true)
            })
    });
}

fn task_target(pane: &EditorPane, cx: &App) -> TaskTarget {
    let preview = pane.note_view == NoteView::Preview && !pane.research_note_open();
    if preview {
        if let Some(target) = preview_target(pane) {
            return target;
        }
        return blank(TaskEdit::Insert {
            at: pane.latest.len(),
        });
    }
    cursor_target(pane, cx)
}

fn preview_target(pane: &EditorPane) -> Option<TaskTarget> {
    let gesture = pane.row_gesture.as_ref()?;
    let rows = super::split_rows(&pane.latest);
    let (_, hi) = gesture.bounds(rows.len())?;
    let row = rows[hi];
    let slice = pane.latest.get(row.start..row.end)?;
    let line_end = slice.find('\n').unwrap_or(slice.len());
    let raw = &slice[..line_end];
    let logical = raw.strip_suffix('\r').unwrap_or(raw);
    let line = line_index(&pane.latest, row.start);
    if parse_task_line(logical).is_some() {
        Some(filled(TaskEdit::Replace { line }, Some(logical)))
    } else {
        Some(blank(TaskEdit::Insert { at: row.end }))
    }
}

fn cursor_target(pane: &EditorPane, cx: &App) -> TaskTarget {
    let editor = pane.editor.read(cx);
    let body = editor.value().to_string();
    let at = editor.selected_range().start.min(body.len());
    let line = line_index(&body, at);
    let text = body_line(&body, line);
    if text.is_some_and(|line| parse_task_line(line).is_some()) {
        filled(TaskEdit::Replace { line }, text)
    } else {
        blank(TaskEdit::Insert { at })
    }
}

fn filled(edit: TaskEdit, line: Option<&str>) -> TaskTarget {
    let task = line.and_then(parse_task_line);
    TaskTarget {
        edit,
        description: task
            .as_ref()
            .map(|task| task.text.clone())
            .unwrap_or_default(),
        due: task
            .and_then(|task| task.due)
            .map(|date| date.to_string())
            .unwrap_or_default(),
    }
}

fn blank(edit: TaskEdit) -> TaskTarget {
    TaskTarget {
        edit,
        description: String::new(),
        due: String::new(),
    }
}

fn naive_due(text: &str) -> Option<NaiveDate> {
    let date = TaskDate::parse(text.trim())?;
    let text = date.to_string();
    let mut parts = text.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

fn draft(description: &str, due: &str) -> Result<(String, Option<TaskDate>), String> {
    let description = description.trim();
    if description.is_empty() || description.contains(['\n', '\r']) {
        return Err(locale::text("tasks.need_text").to_string());
    }
    if description.chars().count() > TEXT_LIMIT {
        return Err(locale::text("notice.value_long").to_string());
    }
    let due = due.trim();
    if due.is_empty() {
        return Ok((description.to_string(), None));
    }
    let due = TaskDate::parse(due).ok_or_else(|| locale::text("tasks.due_bad").to_string())?;
    Ok((description.to_string(), Some(due)))
}

fn line_index(body: &str, at: usize) -> u32 {
    let at = at.min(body.len());
    let at = floor_char(body, at);
    body[..at].bytes().filter(|byte| *byte == b'\n').count() as u32
}

fn floor_char(text: &str, mut offset: usize) -> usize {
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

struct Listed {
    id: NoteId,
    line: u32,
    checked: bool,
    text: String,
    due: Option<TaskDate>,
}

fn gather(pane: &EditorPane) -> Vec<Listed> {
    let mut rows = Vec::new();
    for id in pane.index.ids() {
        let owned: Vec<TaskRef> = if pane.open_note.as_ref() == Some(id) {
            collect_tasks(&pane.latest)
        } else {
            pane.index.tasks(id).to_vec()
        };
        for task in owned {
            rows.push(Listed {
                id: id.clone(),
                line: task.line,
                checked: task.checked,
                text: task.text,
                due: task.due,
            });
        }
    }
    rows
}

fn arrange(mut rows: Vec<Listed>, show_done: bool) -> (Vec<Listed>, bool) {
    if !show_done {
        rows.retain(|row| !row.checked);
    }
    rows.sort_by(|left, right| {
        due_order(left.due, right.due)
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.line.cmp(&right.line))
    });
    let more = rows.len() > TASK_CAP;
    rows.truncate(TASK_CAP);
    (rows, more)
}

fn due_order(left: Option<TaskDate>, right: Option<TaskDate>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::{Listed, arrange, draft};
    use runebook::{NoteId, TaskDate};

    fn row(id: &str, line: u32, checked: bool, due: Option<&str>) -> Listed {
        Listed {
            id: NoteId::parse(id).expect("id"),
            line,
            checked,
            text: id.to_string(),
            due: due.and_then(TaskDate::parse),
        }
    }

    #[test]
    fn open_tasks_sort_by_due_date_then_note() {
        let (rows, more) = arrange(
            vec![
                row("b", 0, false, None),
                row("a", 1, false, Some("2026-12-01")),
                row("a", 0, true, Some("2026-11-01")),
                row("a", 2, false, Some("2026-11-02")),
            ],
            false,
        );
        assert!(!more);
        let order: Vec<_> = rows.iter().map(|row| (row.id.as_str(), row.line)).collect();
        assert_eq!(order, vec![("a", 2), ("a", 1), ("b", 0)]);
    }

    #[test]
    fn a_due_date_becomes_a_calendar_day() {
        let date = super::naive_due("2026-11-02").expect("date");
        assert_eq!(date.to_string(), "2026-11-02");
        assert!(super::naive_due("").is_none());
        assert!(super::naive_due("2026-13-01").is_none());
    }

    #[test]
    fn show_done_keeps_a_checked_task() {
        let (rows, _) = arrange(vec![row("a", 0, true, Some("2026-11-01"))], true);
        assert_eq!(rows.len(), 1);
        assert!(draft("wash", "").is_ok());
        assert!(draft("wash", "2026-11-02").is_ok());
        assert!(draft("  ", "").is_err());
        assert!(draft("wash", "2026-13-01").is_err());
    }
}
