//! Preview rows.
//!
//! A click selects one row. A second plain click on that selection clears it.
//! A drag or a shift-click extends the range.
//! The actions float over the note when the pointer goes up.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Enter, Escape, Input};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable, Selectable, Sizable};
use gpui_kit::prelude::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::{
    Anchor, AnyElement, Bounds, ClickEvent, Context, Image, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, SharedString, StyleRefinement, canvas,
    div, img, px, svg,
};
use runebook::{NoteKind, parse_task_line};

use super::EditorPane;
use super::rows::split_rows;
use crate::Root;
use crate::playback::{ClipKind, MediaLink};

pub(super) fn surface(
    pane: &EditorPane,
    view: gpui_kit::WeakEntity<Root>,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let slices: Vec<String> = split_rows(&pane.latest)
        .into_iter()
        .map(|row| pane.latest[row.start..row.end].to_string())
        .collect();
    let bounds = pane
        .row_gesture
        .as_ref()
        .and_then(|gesture| gesture.bounds(slices.len()));
    let menu = bounds.is_some()
        && pane
            .row_gesture
            .as_ref()
            .is_some_and(|gesture| gesture.menu_open());
    let menu_at = bounds.map(|(_, hi)| hi);
    let rewrite = pane.row_rewrite.clone();
    let selection = cx.theme().selection;
    let hover = cx.theme().list_hover;

    let kind = pane.note_kind();
    let images = pane
        .open_note
        .as_ref()
        .map(|note| crate::markdown_view::ImageRoots {
            vault: pane.vault.clone(),
            note: note.clone(),
        });
    let mut body: Vec<AnyElement> = Vec::with_capacity(slices.len() + 1);
    for (index, slice) in slices.into_iter().enumerate() {
        let selected = bounds.is_some_and(|(lo, hi)| index >= lo && index <= hi);
        let (checked, source) = row_source(kind, &slice);
        let clip = crate::playback::media_link(&slice).map(|link| {
            let playing = pane.playing_href() == Some(link.href.as_str());
            let frame = pane.picture(&link.href);
            let live = pane.live_frame(&link.href);
            let position = pane.clip_position(&link.href);
            let duration = pane.clip_duration(&link.href);
            let muted = pane.media_muted();
            if link.kind == ClipKind::Video && frame.is_none() && !pane.poster_started(&link.href) {
                let href = link.href.clone();
                let view = view.clone();
                cx.defer(move |cx| {
                    view.update(cx, |root, cx| {
                        if let Some(pane) = root.pane.as_mut() {
                            pane.request_poster(&href, cx);
                        }
                    })
                    .ok();
                });
            }
            if !pane.has_timing(&link.href) {
                let href = link.href.clone();
                let view = view.clone();
                cx.defer(move |cx| {
                    view.update(cx, |root, cx| {
                        if let Some(pane) = root.pane.as_mut() {
                            pane.ensure_timing(&href, cx);
                        }
                    })
                    .ok();
                });
            }
            ClipPaint {
                link,
                playing,
                frame,
                live,
                position,
                duration,
                muted,
            }
        });
        let prepared = crate::anchors::prepare(&pane.latest, &source);
        let pad = super::rows::list_pad(&source);
        let row = note_row(
            index,
            RowText {
                source: prepared.source,
                hide_anchors: prepared.hide_anchors,
                pad,
                images: images.clone(),
            },
            RowPaint {
                selected,
                selection,
                hover,
                checked,
            },
            clip,
            view.clone(),
            cx,
        )
        .into_any_element();
        if menu && menu_at == Some(index) {
            body.push(row_actions(row, rewrite.clone(), view.clone(), cx).into_any_element());
        } else {
            body.push(row);
        }
    }

    div()
        .id("preview")
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p_4()
        .flex()
        .flex_col()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.clear_note_rows();
                }
                cx.notify();
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.release_note_row();
                }
                cx.notify();
            }),
        )
        .children(body)
}

fn row_source(kind: NoteKind, slice: &str) -> (Option<bool>, String) {
    if kind != NoteKind::Deed {
        return (None, slice.to_string());
    }
    let line_end = slice.find('\n').unwrap_or(slice.len());
    let raw = &slice[..line_end];
    let logical = raw.strip_suffix('\r').unwrap_or(raw);
    let Some(task) = parse_task_line(logical) else {
        return (None, slice.to_string());
    };
    let mut shown = task.plain;
    shown.push_str(&slice[line_end..]);
    (Some(task.checked), shown)
}

struct RowPaint {
    selected: bool,
    selection: gpui_kit::Hsla,
    hover: gpui_kit::Hsla,
    checked: Option<bool>,
}

struct ClipPaint {
    link: MediaLink,
    playing: bool,
    frame: Option<Arc<Image>>,
    live: Option<Arc<gpui_kit::RenderImage>>,
    position: Duration,
    duration: Duration,
    muted: bool,
}

struct RowText {
    source: String,
    hide_anchors: bool,
    pad: f32,
    images: Option<crate::markdown_view::ImageRoots>,
}

fn note_row(
    index: usize,
    text: RowText,
    paint: RowPaint,
    clip: Option<ClipPaint>,
    view: gpui_kit::WeakEntity<Root>,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let RowText {
        source,
        hide_anchors,
        pad,
        images,
    } = text;
    let RowPaint {
        selected,
        selection,
        hover,
        checked,
    } = paint;
    div()
        .id(SharedString::from(format!("note-row-{index}")))
        .w_full()
        .min_w(px(0.))
        .px_2()
        .py(px(2.))
        .flex()
        .items_start()
        .rounded_md()
        .cursor_pointer()
        .when(selected, |row| row.bg(selection))
        .when(!selected, |row| row.hover(move |style| style.bg(hover)))
        .when_some(checked, |row, checked| {
            row.child(task_box(index, checked, cx))
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if let Some(pane) = this.pane.as_mut() {
                    pane.press_note_row(index, event.modifiers.shift);
                }
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
            let changed = this
                .pane
                .as_mut()
                .is_some_and(|pane| pane.drag_note_row(index));
            if changed {
                cx.notify();
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if let Some(pane) = this.pane.as_mut() {
                    pane.release_note_row();
                }
                cx.notify();
            }),
        )
        .child(match clip {
            Some(clip) => player(index, clip, cx).into_any_element(),
            None => div()
                .flex_1()
                .min_w(px(0.))
                .pl(px(pad))
                .child(
                    crate::markdown_view::view(
                        SharedString::from(format!("note-row-text-{index}")),
                        source,
                        images,
                        hide_anchors,
                    )
                    .w_full()
                    .selectable(false)
                    .on_link_click(move |url, _, window, cx| {
                        let url = url.to_string();
                        view.update(cx, |this, cx| {
                            this.follow_link(&url, window, cx);
                        })
                        .ok();
                    }),
                )
                .into_any_element(),
        })
}

fn fitted(source: impl Into<gpui_kit::ImageSource> + 'static) -> impl IntoElement {
    div().absolute().inset_0().overflow_hidden().child(
        img(source)
            .w_full()
            .h_full()
            .object_fit(gpui_kit::ObjectFit::Fill),
    )
}

fn player(index: usize, clip: ClipPaint, cx: &mut Context<Root>) -> impl IntoElement + 'static {
    let ClipPaint {
        link,
        playing,
        frame,
        live,
        position,
        duration,
        muted,
    } = clip;
    let href = link.href;
    let label = link.label;
    let kind = link.kind;
    let seek_bounds = Arc::new(Mutex::new(Bounds::default()));
    let finish_href = href.clone();
    let bar = transport(
        Transport {
            index,
            href: href.clone(),
            name: label,
            kind,
            playing,
            position,
            duration,
            muted,
            overlay: kind == ClipKind::Video,
        },
        &seek_bounds,
        cx,
    );
    if kind != ClipKind::Video {
        return div()
            .id(SharedString::from(format!("player-{index}")))
            .w_full()
            .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
                cx.stop_propagation();
            })
            .child(bar)
            .into_any_element();
    }
    let well = cx.theme().secondary;
    let ratio = frame
        .as_ref()
        .and_then(|image| crate::playback::jpeg_ratio(&image.bytes))
        .unwrap_or(16.0 / 9.0);
    let play_href = href.clone();
    let seek_href = href.clone();
    let center_href = href.clone();
    let move_bounds = Arc::clone(&seek_bounds);
    div()
        .id(SharedString::from(format!("player-{index}")))
        .relative()
        .w_full()
        .min_w(px(0.))
        .aspect_ratio(ratio)
        .overflow_hidden()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                if let Some(pane) = this.pane.as_mut() {
                    pane.toggle_playback(&play_href, kind, window, cx);
                }
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
            let Some(pane) = this.pane.as_mut() else {
                return;
            };
            let bounds = match move_bounds.lock() {
                Ok(bounds) => *bounds,
                Err(err) => *err.into_inner(),
            };
            pane.move_seek(&seek_href, seek_ratio(bounds, event.position.x));
            cx.notify();
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                cx.stop_propagation();
                if let Some(pane) = this.pane.as_mut() {
                    pane.finish_seek(&finish_href, kind, window, cx);
                }
                cx.notify();
            }),
        )
        .when(frame.is_none() && live.is_none(), |stage| stage.bg(well))
        .when(live.is_none(), |stage| {
            stage.when_some(frame, |stage, image| stage.child(fitted(image)))
        })
        .when_some(live, |stage, image| stage.child(fitted(image)))
        .when(!playing, |stage| {
            let href = center_href;
            stage.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon_hit(
                        format!("center-play-{index}"),
                        IconName::Play,
                        px(28.),
                        "Play",
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            if let Some(pane) = this.pane.as_mut() {
                                pane.toggle_playback(&href, kind, window, cx);
                            }
                            cx.notify();
                        }),
                    )),
            )
        })
        .child(bar)
        .into_any_element()
}

struct Transport {
    index: usize,
    href: String,
    name: String,
    kind: ClipKind,
    playing: bool,
    position: Duration,
    duration: Duration,
    muted: bool,
    overlay: bool,
}

fn transport(
    bar: Transport,
    seek_bounds: &Arc<Mutex<Bounds<Pixels>>>,
    cx: &mut Context<Root>,
) -> impl IntoElement + 'static {
    let Transport {
        index,
        href,
        name,
        kind,
        playing,
        position,
        duration,
        muted,
        overlay,
    } = bar;
    let ink = cx.theme().foreground;
    let track = cx.theme().border;
    let fill = cx.theme().primary;
    let wash = cx.theme().background.opacity(0.82);
    let fraction = if duration.is_zero() {
        0.0
    } else {
        (position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
    };
    let clock_label = format!("{} / {}", clock(position), clock(duration));
    let play_href = href.clone();
    let bounds = Arc::clone(seek_bounds);
    div()
        .id(SharedString::from(format!("transport-{index}")))
        .when(overlay, |row| row.absolute().bottom_0().left_0().right_0())
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .bg(wash)
        .text_color(ink)
        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
            cx.stop_propagation();
        })
        .child(icon_hit(
            format!("play-{index}"),
            if playing {
                IconName::Pause
            } else {
                IconName::Play
            },
            px(16.),
            if playing { "Pause" } else { "Play" },
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                if let Some(pane) = this.pane.as_mut() {
                    pane.toggle_playback(&play_href, kind, window, cx);
                }
                cx.notify();
            }),
        ))
        .when(!overlay, |row| row.child(div().text_sm().child(name)))
        .child(div().text_xs().w(px(92.)).child(clock_label))
        .child(seek_track(
            Seek {
                index,
                href,
                kind,
                fraction,
                track,
                fill,
            },
            bounds,
            cx,
        ))
        .child(icon_hit(
            format!("mute-{index}"),
            if muted {
                IconName::VolumeX
            } else {
                IconName::Volume2
            },
            px(16.),
            if muted { "Unmute" } else { "Mute" },
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if let Some(pane) = this.pane.as_mut() {
                    pane.toggle_mute();
                }
                cx.notify();
            }),
        ))
}

struct Seek {
    index: usize,
    href: String,
    kind: ClipKind,
    fraction: f32,
    track: gpui_kit::Hsla,
    fill: gpui_kit::Hsla,
}

fn seek_track(
    bar: Seek,
    bounds: Arc<Mutex<Bounds<Pixels>>>,
    cx: &mut Context<Root>,
) -> impl IntoElement + 'static {
    let Seek {
        index,
        href,
        kind,
        fraction,
        track,
        fill,
    } = bar;
    let paint_bounds = Arc::clone(&bounds);
    let down_bounds = Arc::clone(&bounds);
    let move_bounds = Arc::clone(&bounds);
    let move_href = href.clone();
    let up_href = href.clone();
    div()
        .id(SharedString::from(format!("seek-{index}")))
        .flex_1()
        .h(px(18.))
        .relative()
        .cursor_pointer()
        .child(
            canvas(
                move |area, _, _| {
                    let mut slot = match paint_bounds.lock() {
                        Ok(slot) => slot,
                        Err(err) => err.into_inner(),
                    };
                    *slot = area;
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(7.))
                .h(px(4.))
                .rounded_full()
                .bg(track)
                .child(
                    div()
                        .h_full()
                        .w(gpui_kit::relative(fraction))
                        .rounded_full()
                        .bg(fill),
                ),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                let Some(pane) = this.pane.as_mut() else {
                    return;
                };
                let area = match down_bounds.lock() {
                    Ok(area) => *area,
                    Err(err) => *err.into_inner(),
                };
                pane.begin_seek(&href, seek_ratio(area, event.position.x));
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
            let Some(pane) = this.pane.as_mut() else {
                return;
            };
            let area = match move_bounds.lock() {
                Ok(area) => *area,
                Err(err) => *err.into_inner(),
            };
            pane.move_seek(&move_href, seek_ratio(area, event.position.x));
            cx.notify();
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                cx.stop_propagation();
                if let Some(pane) = this.pane.as_mut() {
                    pane.finish_seek(&up_href, kind, window, cx);
                }
                cx.notify();
            }),
        )
}

fn icon_hit(
    id: impl Into<SharedString>,
    icon: IconName,
    size: Pixels,
    tip: &'static str,
    on_down: impl Fn(&MouseDownEvent, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement + 'static {
    div()
        .id(id.into())
        .p_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        .child(svg().path(icon.path()).size(size))
        .on_mouse_down(MouseButton::Left, on_down)
        .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
            cx.stop_propagation();
        })
}

fn seek_ratio(bounds: Bounds<Pixels>, x: Pixels) -> f32 {
    let width = f32::from(bounds.size.width);
    if width <= 0.0 {
        return 0.0;
    }
    let delta = f32::from(x) - f32::from(bounds.origin.x);
    (delta / width).clamp(0.0, 1.0)
}

fn clock(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

fn task_box(index: usize, checked: bool, cx: &mut Context<Root>) -> impl IntoElement {
    let border = cx.theme().foreground;
    let empty = cx.theme().background;
    div()
        .id(SharedString::from(format!("task-box-{index}")))
        .mt(px(4.))
        .mr_2()
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
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            cx.stop_propagation();
            let dirty = this
                .pane
                .as_mut()
                .is_some_and(|pane| pane.toggle_deed_task(index, window, cx));
            if dirty {
                this.schedule_autosave(cx);
            }
            cx.notify();
        }))
}

/// The selected row, used only to anchor the action popover.
struct RowAnchor {
    row: AnyElement,
}

impl Selectable for RowAnchor {
    fn selected(self, _: bool) -> Self {
        self
    }

    fn is_selected(&self) -> bool {
        false
    }

    fn open(self, _: bool) -> Self {
        self
    }

    fn is_open(&self) -> bool {
        false
    }
}

impl IntoElement for RowAnchor {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        self.row
    }
}

/// Actions for the selection. They float under the last selected row.
///
/// The row owns the second click. Outside dismiss would clear the selection
/// in the capture phase, and that same click would select the row again.
fn row_actions(
    row: AnyElement,
    rewrite: Option<gpui_kit::Entity<gpui_kit::component::input::InputState>>,
    view: gpui_kit::WeakEntity<Root>,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    Popover::new("note-row-actions")
        .anchor(Anchor::TopLeft)
        .arrow(true)
        .open(true)
        .overlay_closable(false)
        .trigger_style(StyleRefinement::default().w_full())
        .on_open_change(move |open, window, cx| {
            if *open {
                return;
            }
            // The popover can close during the row click. Clear after that update ends.
            let view = view.clone();
            window.defer(cx, move |_, cx| {
                view.update(cx, |root, cx| {
                    if let Some(pane) = root.pane.as_mut() {
                        pane.clear_note_rows();
                    }
                    cx.notify();
                })
                .ok();
            });
        })
        .trigger(RowAnchor { row })
        .child(row_menu(rewrite, cx))
}

fn row_menu(
    rewrite: Option<gpui_kit::Entity<gpui_kit::component::input::InputState>>,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let prompting = rewrite.is_some();
    let empty = rewrite
        .as_ref()
        .is_some_and(|input| input.read(cx).value().trim().is_empty());
    div()
        .id("note-row-menu")
        .flex()
        .flex_col()
        .gap_2()
        .when(prompting, |menu| menu.w(px(280.)))
        .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
        })
        .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
            cx.stop_propagation();
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .child(
                    Button::new("row-add-chat")
                        .small()
                        .cursor_pointer()
                        .label("Add to chat")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(pane) = this.pane.as_mut() {
                                pane.add_selected_rows_to_chat();
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("row-rewrite")
                        .small()
                        .primary()
                        .cursor_pointer()
                        .label("Rewrite")
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            if let Some(pane) = this.pane.as_mut() {
                                pane.begin_row_rewrite(window, cx);
                            }
                            cx.notify();
                        })),
                ),
        )
        .when_some(rewrite, |menu, input| {
            menu.child(
                div()
                    .id("row-rewrite-prompt")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_action(cx.listener(|this, _: &Enter, window, cx| {
                        cx.stop_propagation();
                        if let Some(pane) = this.pane.as_mut() {
                            pane.apply_row_rewrite(window, cx);
                        }
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &Escape, _, cx| {
                        cx.stop_propagation();
                        if let Some(pane) = this.pane.as_mut() {
                            pane.clear_note_rows();
                        }
                        cx.notify();
                    }))
                    .child(Input::new(&input).w_full())
                    .child(
                        Button::new("row-rewrite-apply")
                            .small()
                            .primary()
                            .cursor_pointer()
                            .disabled(empty)
                            .label("Apply")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                if let Some(pane) = this.pane.as_mut() {
                                    pane.apply_row_rewrite(window, cx);
                                }
                                cx.notify();
                            })),
                    ),
            )
        })
}
