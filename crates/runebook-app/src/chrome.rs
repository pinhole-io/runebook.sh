//! Minimize, maximize, and exit controls for the window.

use gpui_kit::assets::IconName;
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::*;
use gpui_kit::{ClickEvent, Context, SharedString, Window, div, px, svg};

use crate::Root;

struct WinButton {
    id: &'static str,
    label: &'static str,
    icon: IconName,
    color: gpui_kit::Hsla,
    hover_bg: gpui_kit::Hsla,
    hover_fg: gpui_kit::Hsla,
    close: bool,
}

pub(crate) fn window_controls(window: &Window, cx: &mut Context<Root>) -> impl IntoElement {
    let maximized = window.is_maximized();
    let foreground = cx.theme().foreground;
    let hover = cx.theme().secondary_hover;
    let danger = cx.theme().danger;
    let danger_fg = cx.theme().danger_foreground;
    div()
        .flex()
        .items_center()
        .flex_shrink_0()
        .child(win_button(
            WinButton {
                id: "win-min",
                label: "Minimize",
                icon: IconName::WindowMinimize,
                color: foreground,
                hover_bg: hover,
                hover_fg: foreground,
                close: false,
            },
            cx,
            |window| window.minimize_window(),
        ))
        .child(win_button(
            WinButton {
                id: "win-max",
                label: if maximized { "Restore" } else { "Maximize" },
                icon: if maximized {
                    IconName::WindowRestore
                } else {
                    IconName::WindowMaximize
                },
                color: foreground,
                hover_bg: hover,
                hover_fg: foreground,
                close: false,
            },
            cx,
            |window| window.zoom_window(),
        ))
        .child(win_button(
            WinButton {
                id: "win-close",
                label: "Exit",
                icon: IconName::WindowClose,
                color: foreground,
                hover_bg: danger,
                hover_fg: danger_fg,
                close: true,
            },
            cx,
            |_| {},
        ))
}

fn win_button(
    button: WinButton,
    cx: &mut Context<Root>,
    action: impl Fn(&mut Window) + 'static,
) -> impl IntoElement {
    let group = SharedString::from(button.id);
    let hover_group = group.clone();
    let WinButton {
        id,
        label,
        icon,
        color,
        hover_bg,
        hover_fg,
        close,
    } = button;
    div()
        .id(id)
        .group(group)
        .w(px(36.))
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |style| style.bg(hover_bg))
        .aria_label(label)
        .child(
            svg()
                .path(icon.path())
                .size_3p5()
                .flex_shrink_0()
                .text_color(color)
                .group_hover(hover_group, move |style| style.text_color(hover_fg)),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            if close {
                this.flush(cx);
                window.remove_window();
            } else {
                action(window);
            }
            cx.notify();
        }))
}
