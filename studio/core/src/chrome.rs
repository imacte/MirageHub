//! Window controls drawn directly over the page, without a separate title bar.

use crate::{t, theme};
use egui::{pos2, vec2, Color32, Rect, Sense, Stroke, StrokeKind, ViewportCommand};

pub const HEIGHT: f32 = 32.0;
pub const BUTTON_WIDTH: f32 = 46.0;

pub fn show(ctx: &egui::Context, resizable: bool, can_close: bool) {
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let width = BUTTON_WIDTH * if resizable { 3.0 } else { 2.0 };
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("window-controls"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(screen.right() - width, screen.top()))
        .default_size(vec2(width, HEIGHT))
        .constrain(false)
        .movable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.horizontal(|ui| {
                if control(ui, t("Minimize"), false, true, |painter, rect, stroke| {
                    painter.line_segment([pos2(rect.left(), rect.center().y), pos2(rect.right(), rect.center().y)], stroke);
                }) {
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                }
                if resizable && control(ui, if maximized { t("Restore") } else { t("Maximize") }, false, true, |painter, rect, stroke| {
                    if maximized {
                        let back = rect.translate(vec2(2.0, -2.0)).shrink(1.0);
                        painter.line_segment([back.left_top(), back.right_top()], stroke);
                        painter.line_segment([back.right_top(), back.right_bottom()], stroke);
                        painter.rect_stroke(rect.translate(vec2(-1.0, 1.0)).shrink(1.0), 0, stroke, StrokeKind::Inside);
                    } else {
                        painter.rect_stroke(rect, 0, stroke, StrokeKind::Inside);
                    }
                }) {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                }
                if control(ui, t("Close"), true, can_close, |painter, rect, stroke| {
                    painter.line_segment([rect.left_top(), rect.right_bottom()], stroke);
                    painter.line_segment([rect.right_top(), rect.left_bottom()], stroke);
                }) {
                    // Use the normal close request so close-to-tray and the
                    // installer's in-progress guard keep working.
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            });
        });
}

fn control(ui: &mut egui::Ui, label: &str, close: bool, enabled: bool, draw: impl FnOnce(&egui::Painter, Rect, Stroke)) -> bool {
    ui.add_enabled_ui(enabled, |ui| {
        let (rect, response) = ui.allocate_exact_size(vec2(BUTTON_WIDTH, HEIGHT), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, 0, if close { Color32::from_rgb(0xC4, 0x2B, 0x1C) } else { theme::SURFACE_HOVER });
        }
        let color = if !enabled { theme::TEXT_FAINT } else if response.hovered() { theme::TEXT } else { theme::TEXT_DIM };
        draw(ui.painter(), Rect::from_center_size(rect.center(), vec2(10.0, 10.0)), Stroke::new(1.0, color));
        response.on_hover_text(label).clicked()
    }).inner
}
