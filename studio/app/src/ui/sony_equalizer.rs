//! Preset selection is kept separately from device readback so a custom curve survives switching.
use egui::{pos2, vec2, Align2, Color32, Rect, Sense, Stroke};
use mirage_hub_core::{t, theme, widgets};

const MUSIC: [f64; 10] = [3., 2., 2., 1., 0., -1., -2., -2., -9., -9.];
const LABELS: [&str; 10] = [
    "31.5", "63", "125", "250", "500", "1k", "2k", "4k", "8k", "16k",
];

#[derive(Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
enum Preset {
    Music,
    #[default]
    Custom,
    Flat,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Equalizer {
    custom: Option<[f64; 10]>,
    selected: Preset,
    #[serde(skip)]
    last: Option<[f64; 10]>,
}

impl Equalizer {
    fn sync(&mut self, bands: [f64; 10]) {
        if self.last == Some(bands) {
            return;
        }
        self.selected = if bands == MUSIC {
            Preset::Music
        } else if bands == [0.; 10] {
            Preset::Flat
        } else {
            Preset::Custom
        };
        if self.selected == Preset::Custom {
            self.custom = Some(bands);
        }
        self.last = Some(bands);
    }

    fn choose(&mut self, preset: Preset, bands: &mut [f64; 10]) {
        if self.selected == Preset::Custom {
            self.custom = Some(*bands);
        }
        *bands = match preset {
            Preset::Music => MUSIC,
            Preset::Flat => [0.; 10],
            Preset::Custom => self.custom.unwrap_or(*bands),
        };
        self.selected = preset;
        self.last = Some(*bands);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, serial: &str, values: &mut [f64]) -> bool {
        let Ok(mut bands): Result<[f64; 10], _> = values.try_into() else {
            return false;
        };
        self.sync(bands);
        let before = bands;
        ui.add_space(8.0);
        ui.columns(3, |columns| {
            for (ui, (preset, title, description)) in columns.iter_mut().zip([
                (
                    Preset::Music,
                    t("Music / video"),
                    t("A warm curve for music and video."),
                ),
                (
                    Preset::Custom,
                    t("Custom"),
                    t("Keep and adjust your own sound."),
                ),
                (Preset::Flat, t("Flat"), t("All bands at 0 dB.")),
            ]) {
                let selected = self.selected == preset;
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 82.), Sense::click());
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        ui.is_enabled(),
                        selected,
                        title,
                    )
                });
                let fill = if selected {
                    theme::SURFACE_ACTIVE
                } else if response.hovered() {
                    theme::SURFACE_HI
                } else {
                    theme::BG
                };
                ui.painter().rect_filled(rect, 5., fill);
                ui.painter().rect_stroke(
                    rect,
                    5.,
                    Stroke::new(
                        1.,
                        if selected || response.has_focus() {
                            theme::ACCENT
                        } else {
                            theme::BORDER
                        },
                    ),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.min + vec2(12., 14.),
                    Align2::LEFT_TOP,
                    title,
                    theme::bold(14.),
                    if selected { theme::ACCENT } else { theme::TEXT },
                );
                let text = ui.painter().layout(
                    description.into(),
                    theme::body(12.),
                    theme::TEXT_DIM,
                    (rect.width() - 24.).max(1.),
                );
                ui.painter()
                    .galley(rect.min + vec2(12., 39.), text, theme::TEXT_DIM);
                if response.clicked() {
                    self.choose(preset, &mut bands);
                }
            }
        });
        ui.add_space(20.);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 270.), Sense::hover());
        let graph = Rect::from_min_max(rect.min + vec2(44., 9.), rect.max - vec2(5., 64.));
        let cell = graph.width() / 10.;
        let y = |gain: f64| egui::lerp(graph.y_range(), (12. - gain) as f32 / 24.);
        for db in (-12..=12).step_by(3) {
            let position = y(db as f64);
            ui.painter().hline(
                graph.x_range(),
                position,
                Stroke::new(
                    1.,
                    if db == 0 {
                        theme::BORDER_HI
                    } else {
                        theme::BORDER
                    },
                ),
            );
            if db % 6 == 0 {
                ui.painter().text(
                    pos2(graph.left() - 9., position),
                    Align2::RIGHT_CENTER,
                    format!("{db:+}"),
                    theme::body(11.),
                    theme::TEXT_DIM,
                );
            }
        }
        ui.painter().text(
            rect.min,
            Align2::LEFT_TOP,
            t("dB"),
            theme::body(10.),
            theme::TEXT_FAINT,
        );
        let mut edited = false;
        for (index, gain) in bands.iter_mut().enumerate() {
            let x = graph.left() + cell * (index as f32 + 0.5);
            ui.painter()
                .vline(x, graph.y_range(), Stroke::new(1., theme::BORDER));
            // Let egui handle focus, keyboard and accessibility; paint the rail and flat handle ourselves.
            let lane =
                Rect::from_center_size(pos2(x, graph.center().y), vec2(30., graph.height() + 6.));
            let before_gain = *gain;
            let response = ui
                .scope_builder(
                    egui::UiBuilder::new()
                        .id_salt(("sony-band", serial, index))
                        .max_rect(lane),
                    |ui| {
                        ui.spacing_mut().slider_width = lane.height();
                        ui.spacing_mut().interact_size.y = 30.;
                        let style = ui.visuals_mut();
                        for visuals in [
                            &mut style.widgets.inactive,
                            &mut style.widgets.hovered,
                            &mut style.widgets.active,
                        ] {
                            visuals.bg_fill = Color32::TRANSPARENT;
                            visuals.fg_stroke = Stroke::NONE;
                            visuals.bg_stroke = Stroke::NONE;
                        }
                        style.selection.bg_fill = Color32::TRANSPARENT;
                        ui.add(
                            egui::Slider::new(gain, -12.0..=12.0)
                                .vertical()
                                .show_value(false)
                                .step_by(0.5)
                                .trailing_fill(false)
                                .handle_shape(egui::style::HandleShape::Rect {
                                    aspect_ratio: 0.25,
                                }),
                        )
                    },
                )
                .inner;
            edited |= response.changed();
            // egui 0.31 applies an explicit step without reversing it for a vertical slider.
            if response.has_focus() {
                let steps = ui.input(|i| {
                    i.num_presses(egui::Key::ArrowUp) as i32
                        - i.num_presses(egui::Key::ArrowDown) as i32
                });
                if steps != 0 {
                    *gain = (before_gain + steps as f64 * 0.5).clamp(-12., 12.);
                    edited = true;
                }
            }
            if response.double_clicked() {
                *gain = 0.;
                edited = true;
            }
            response.widget_info(|| {
                egui::WidgetInfo::slider(ui.is_enabled(), *gain, format!("{} Hz", LABELS[index]))
            });
            let handle =
                Rect::from_center_size(pos2(x, y(*gain)), vec2((cell - 12.).clamp(15., 34.), 6.));
            ui.painter().rect_filled(
                handle,
                1.,
                if ui.is_enabled() {
                    theme::TEXT
                } else {
                    theme::TEXT_FAINT
                },
            );
            ui.painter().hline(
                handle.x_range(),
                handle.bottom(),
                Stroke::new(
                    2.,
                    if ui.is_enabled() {
                        theme::ACCENT
                    } else {
                        theme::BORDER
                    },
                ),
            );
            if response.has_focus() {
                ui.painter().rect_stroke(
                    handle.expand(3.),
                    2.,
                    Stroke::new(1., theme::ACCENT),
                    egui::StrokeKind::Outside,
                );
            }
            response.on_hover_text(format!("{} Hz · {gain:+.1} dB", LABELS[index]));
            ui.painter().text(
                pos2(x, graph.bottom() + 14.),
                Align2::CENTER_CENTER,
                LABELS[index],
                theme::body(11.),
                theme::TEXT_DIM,
            );
            let field = Rect::from_min_size(
                pos2(x - (cell - 5.) / 2., graph.bottom() + 27.),
                vec2(cell - 5., 28.),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
                edited |= widgets::number_field(
                    ui,
                    ("sony-gain", serial, index),
                    gain,
                    -12.0..=12.0,
                    1,
                    "",
                    cell - 5.,
                );
            });
        }
        if edited {
            self.selected = Preset::Custom;
            self.custom = Some(bands);
            self.last = Some(bands);
        }
        values.copy_from_slice(&bands);
        self.last = Some(bands);
        bands != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_curve_survives_presets_and_readback() {
        let custom = [1.5; 10];
        let mut bands = custom;
        let mut eq = Equalizer::default();
        eq.sync(bands);
        eq.choose(Preset::Music, &mut bands);
        eq.sync(bands);
        eq.choose(Preset::Flat, &mut bands);
        eq.sync(bands);
        eq.choose(Preset::Custom, &mut bands);
        assert_eq!(bands, custom);
        eq.sync(bands);
        assert!(eq.selected == Preset::Custom);
    }
}
