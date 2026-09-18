//! Client-side window decorations: the title bar's window buttons and the
//! resize borders. Drawn by us because the window asks for no server-side
//! decorations, so nothing else would draw them.

use eframe::egui;
use egui::{Color32, Rect, Sense, pos2, vec2};

use crate::ui::App;
use crate::ui::fonts;
use crate::ui::icon;

const TITLEBAR_BG: Color32 = Color32::from_rgb(30, 30, 36);
/// Height of the title bar, and the width its three window buttons take at
/// the right end of it. The resize borders need both: they keep off the
/// buttons rather than sitting over them, since a press that was meant for
/// Close should not turn into a resize.
const TITLEBAR_H: f32 = 34.0;
const BUTTON_W: f32 = 34.0;
const BUTTONS_W: f32 = 3.0 * BUTTON_W;
const CLOSE_HOVER: Color32 = Color32::from_rgb(224, 27, 36);
enum WindowIcon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// A GNOME-style window button: a grey circle on hover (red for close),
/// with a painted vector icon.
fn window_button(
    ui: &egui::Ui,
    rect: Rect,
    id: &str,
    icon: WindowIcon,
    tooltip: &str,
) -> egui::Response {
    let resp = ui.interact(rect, egui::Id::new(id), Sense::click());
    let p = ui.painter_at(rect);
    let c = rect.center();
    let hover = resp.hovered();
    let fg = if hover {
        if matches!(icon, WindowIcon::Close) {
            p.circle_filled(c, 13.0, CLOSE_HOVER);
        } else {
            p.circle_filled(c, 13.0, Color32::from_gray(65));
        }
        Color32::WHITE
    } else {
        Color32::from_gray(185)
    };
    match icon {
        WindowIcon::Minimize => {
            p.rect_filled(Rect::from_center_size(c, vec2(10.0, 1.5)), 0.5, fg);
        }
        WindowIcon::Maximize => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(10.0, 10.0)),
                2.0,
                egui::Stroke::new(1.5, fg),
                egui::StrokeKind::Middle,
            );
        }
        WindowIcon::Restore => {
            let back = Rect::from_center_size(c + vec2(2.5, 2.5), vec2(9.0, 9.0));
            let front = Rect::from_center_size(c - vec2(2.5, 2.5), vec2(9.0, 9.0));
            p.rect(
                back,
                2.0,
                Color32::TRANSPARENT,
                egui::Stroke::new(1.5, fg),
                egui::StrokeKind::Middle,
            );
            p.rect(
                front,
                2.0,
                TITLEBAR_BG,
                egui::Stroke::new(1.5, fg),
                egui::StrokeKind::Middle,
            );
        }
        WindowIcon::Close => {
            let d = 4.5;
            p.line_segment([c - vec2(d, d), c + vec2(d, d)], egui::Stroke::new(1.6, fg));
            p.line_segment(
                [c - vec2(d, -d), c + vec2(d, -d)],
                egui::Stroke::new(1.6, fg),
            );
        }
    }
    resp.on_hover_text(tooltip)
}

/// Client-side title bar: window buttons, drag-to-move and
/// double-click-to-maximize, plus a north resize grip.
pub fn title_bar(app: &mut App, root: &mut egui::Ui) {
    egui::Panel::top("titlebar")
        .exact_size(TITLEBAR_H)
        .frame(egui::Frame::NONE.fill(TITLEBAR_BG))
        .show(root, |ui| {
            let ctx = ui.ctx().clone();
            let full = ui.available_rect_before_wrap();
            let bw = BUTTON_W;
            let r_close =
                Rect::from_min_max(pos2(full.right() - bw, full.top()), full.right_bottom());
            let r_max = r_close.translate(vec2(-bw, 0.0));
            let r_min = r_max.translate(vec2(-bw, 0.0));

            let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            let close = window_button(ui, r_close, "win-close", WindowIcon::Close, "Close");
            if close.clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            let (max_icon, max_tip) = if maximized {
                (WindowIcon::Restore, "Restore")
            } else {
                (WindowIcon::Maximize, "Maximize")
            };
            let max = window_button(ui, r_max, "win-max", max_icon, max_tip);
            if max.clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            let min = window_button(ui, r_min, "win-min", WindowIcon::Minimize, "Minimize");
            if min.clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }

            // The rest of the bar moves the window; double-click maximizes.
            let drag_rect = Rect::from_min_max(full.min, pos2(r_min.left(), full.bottom()));
            let drag = ui.interact(
                drag_rect,
                egui::Id::new("titlebar-drag"),
                Sense::click_and_drag(),
            );
            if drag.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                forget_pointer(&ctx);
            }
            if drag.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            let icon_rect = Rect::from_center_size(
                pos2(full.left() + 8.0 + icon::TITLE_SIZE / 2.0, full.center().y),
                vec2(icon::TITLE_SIZE, icon::TITLE_SIZE),
            );
            ui.painter().image(
                app.icon.id(),
                icon_rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            ui.painter().text(
                pos2(icon_rect.right() + 8.0, full.center().y),
                egui::Align2::LEFT_CENTER,
                &app.title,
                egui::FontId::proportional(fonts::BODY),
                Color32::from_gray(175),
            );
        });
}

/// Forget that a pointer button is down, having just handed the pointer to
/// the compositor.
///
/// A window move or resize belongs to the compositor for the rest of the
/// gesture, and on Wayland the button release is never sent on to us. egui
/// would go on believing the button is held and swallow the next press as
/// part of a drag that never ended — which is why dragging the window worked
/// only every other time. Upstream bug, still open:
/// <https://github.com/emilk/egui/issues/7959>.
///
/// Only ever called where we ask for that grab. A standing check for a held
/// button — say, whenever the window is not focused — looks like the same
/// thing and is not: clicking an unfocused window delivers the press before
/// egui has registered the focus, and clearing it there kills the first drag
/// of every slider in the window.
fn forget_pointer(ctx: &egui::Context) {
    ctx.input_mut(|i| i.pointer = Default::default());
}

/// Client-side window resize borders.
///
/// One `Area` per strip, each claiming exactly its strip. egui finds the
/// layer under the pointer by the rect an area's content claims, and that
/// rect has caught this code out twice in opposite directions:
///
/// * All eight strips in one area, interacting without allocating, left that
///   area zero-size. Nothing was ever hit-tested, so the edges were inert and
///   the window could not be resized at all.
/// * Allocating each strip with `allocate_rect` and absolute coordinates does
///   not tell the area where it is either: the area grew from its layout
///   origin to swallow the rect, so the east strip claimed 600 points of the
///   window and ate the transport bar's sliders.
///
/// Hence `allocate_exact_size` at the area's own `fixed_pos`: the only
/// arrangement where the area's rect and the strip are the same rectangle.
///
/// They keep off the window buttons in the top-right corner: the north edge
/// stops short of them, the east edge starts below the title bar, and there
/// is no north-east corner. A press meant for Close is not a resize.
pub fn resize_borders(ctx: &egui::Context) {
    use egui::CursorIcon as Cur;
    use egui::viewport::ResizeDirection as Dir;

    // A maximized window is not resizable by its edges.
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }

    const EDGE: f32 = 6.0;
    const CORNER: f32 = 16.0;
    let r = ctx.viewport_rect();
    if r.width() < 4.0 * CORNER || r.height() < 4.0 * CORNER {
        return;
    }
    let (l, rt, t, b) = (r.left(), r.right(), r.top(), r.bottom());

    // Edges first, corners last: a later area sits above an earlier one, and
    // a corner has to beat the two edges it overlaps.
    let regions: [(&str, Rect, Dir, Cur); 7] = [
        (
            "rz-n",
            Rect::from_min_max(pos2(l, t), pos2(rt - BUTTONS_W, t + EDGE)),
            Dir::North,
            Cur::ResizeNorth,
        ),
        (
            "rz-s",
            Rect::from_min_max(pos2(l, b - EDGE), pos2(rt, b)),
            Dir::South,
            Cur::ResizeSouth,
        ),
        (
            "rz-w",
            Rect::from_min_max(pos2(l, t), pos2(l + EDGE, b)),
            Dir::West,
            Cur::ResizeWest,
        ),
        (
            "rz-e",
            Rect::from_min_max(pos2(rt - EDGE, t + TITLEBAR_H), pos2(rt, b)),
            Dir::East,
            Cur::ResizeEast,
        ),
        (
            "rz-nw",
            Rect::from_min_max(pos2(l, t), pos2(l + CORNER, t + CORNER)),
            Dir::NorthWest,
            Cur::ResizeNorthWest,
        ),
        (
            "rz-sw",
            Rect::from_min_max(pos2(l, b - CORNER), pos2(l + CORNER, b)),
            Dir::SouthWest,
            Cur::ResizeSouthWest,
        ),
        (
            "rz-se",
            Rect::from_min_max(pos2(rt - CORNER, b - CORNER), pos2(rt, b)),
            Dir::SouthEast,
            Cur::ResizeSouthEast,
        ),
    ];

    for (id, rect, dir, cursor) in regions {
        egui::Area::new(egui::Id::new(id))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (_, resp) = ui.allocate_exact_size(rect.size(), Sense::drag());
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(cursor);
                }
                if resp.drag_started() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
                    forget_pointer(ui.ctx());
                }
            });
    }
}
