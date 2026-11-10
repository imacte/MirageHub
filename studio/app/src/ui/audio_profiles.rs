use crate::audio_profiles::{self, AudioEffects, Library, Profile};
use mirage_hub_core::{
    t, theme,
    widgets::{self, Button},
};

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Editor {
    pub selected: Option<String>,
    name: String,
    #[serde(skip)]
    error: Option<String>,
    #[serde(skip)]
    deleting: bool,
    #[serde(skip)]
    last: Option<AudioEffects>,
}

#[derive(Default)]
pub struct Changes {
    pub apply: Option<AudioEffects>,
    pub link: Option<String>,
}

impl Editor {
    pub fn select(&mut self, name: Option<String>) {
        self.name = name.clone().unwrap_or_default();
        self.selected = name;
        self.deleting = false;
    }

    /// Returns a preset to open in the application's existing executable picker.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        serial: &str,
        effects: Result<AudioEffects, String>,
        library: &mut Library,
        sync_selection: bool,
    ) -> Changes {
        let mut changes = Changes::default();
        if effects.as_ref().ok() != self.last.as_ref() {
            if let Some(profile) = library
                .presets(serial)
                .iter()
                .find(|p| sync_selection && effects.as_ref().ok() == Some(&p.effects))
            {
                self.select(Some(profile.name.clone()));
            }
            self.last = effects.as_ref().ok().cloned();
        }
        ui.add_space(10.);
        ui.label(widgets::dim(t("Audio effects profiles")));
        widgets::help(ui, t("Saves EQ on/off and all ten bands, dynamic range control and standard spatial sound."));
        ui.horizontal_wrapped(|ui| {
            for curve in library.presets(serial).to_vec() {
                let selected = self.selected.as_deref() == Some(&curve.name);
                let dirty = selected && effects.as_ref().ok() != Some(&curve.effects);
                let label = format!("{}{}", curve.name, if dirty { " *" } else { "" });
                if ui.selectable_label(selected, label).clicked() {
                    changes.apply = Some(curve.effects);
                    self.select(Some(curve.name));
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            widgets::text_field(ui, &mut self.name, t("Preset name"), 180.);
            if ui
                .add(Button::secondary(t("Save as new")).enabled(!self.name.trim().is_empty()))
                .clicked()
            {
                match effects
                    .clone()
                    .and_then(|e| Profile::new(&self.name, e))
                    .and_then(|curve| {
                        let name = curve.name.clone();
                        library.add(serial, curve)?;
                        Ok(name)
                    }) {
                    Ok(name) => {
                        self.select(Some(name));
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            if ui.add(Button::secondary(t("Import preset…"))).clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Mirage Hub Audio Effects", &["json"])
                    .pick_file()
                {
                    match audio_profiles::import(&path).and_then(|curve| {
                        let name = curve.name.clone();
                        library.add(serial, curve)?;
                        Ok(name)
                    }) {
                        Ok(name) => {
                            changes.apply = Some(
                                library
                                    .get(serial, &name)
                                    .expect("just imported")
                                    .effects
                                    .clone(),
                            );
                            self.select(Some(name));
                            self.error = None;
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
            }
        });
        if let Some(selected) = self
            .selected
            .clone()
            .filter(|name| library.get(serial, name).is_some())
        {
            ui.horizontal_wrapped(|ui| {
                if ui.add(Button::secondary(t("Save changes"))).clicked() {
                    match effects
                        .clone()
                        .and_then(|e| Profile::new(&self.name, e))
                        .and_then(|c| {
                            let name = c.name.clone();
                            library.update(serial, &selected, c)?;
                            Ok(name)
                        }) {
                        Ok(name) => {
                            self.select(Some(name));
                            self.error = None;
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                // Rename and curve changes are saved together, preserving associations.
                let current = self.selected.as_ref().unwrap().clone();
                if ui.add(Button::secondary(t("Export preset…"))).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Mirage Hub Audio Effects", &["json"])
                        .set_file_name("audio-effects.json")
                        .save_file()
                    {
                        // Export all visible effects, including unsaved edits.
                        self.error = effects
                            .clone()
                            .and_then(|e| Profile::new(&self.name, e))
                            .and_then(|curve| audio_profiles::export(&path, &curve))
                            .err();
                    }
                }
                if ui.add(Button::secondary(t("Link app…"))).clicked() {
                    changes.link = Some(current.clone());
                }
                let mut default = library.defaults.get(serial) == Some(&current);
                if ui.checkbox(&mut default, t("Default")).changed() {
                    if default {
                        library.defaults.insert(serial.into(), current.clone());
                    } else {
                        library.defaults.remove(serial);
                    }
                }
                if ui.add(Button::secondary(t("Delete"))).clicked() {
                    self.deleting = true;
                }
            });
            if self.deleting {
                ui.horizontal_wrapped(|ui| {
                    ui.label(t("Delete this preset and its app links?"));
                    if ui.add(Button::secondary(t("Delete"))).clicked() {
                        if let Some(name) = &self.selected {
                            library.remove(serial, name);
                        }
                        self.select(None);
                    }
                    if ui.add(Button::secondary(t("Cancel"))).clicked() {
                        self.deleting = false;
                    }
                });
            }
            let current = self.selected.clone();
            ui.horizontal_wrapped(|ui| {
                for rule in library
                    .rules
                    .clone()
                    .into_iter()
                    .filter(|r| r.serial == serial && Some(&r.profile) == current.as_ref())
                {
                    if ui
                        .small_button(format!("{} ×", rule.exe))
                        .on_hover_text(t("Unlink app"))
                        .clicked()
                    {
                        library.rules.retain(|r| r != &rule);
                    }
                }
            });
        }
        if !library.presets(serial).is_empty() {
            ui.checkbox(
                &mut library.auto_switch,
                t("Switch audio effects profiles with the foreground app"),
            );
            widgets::help(ui, t("Default is restored outside linked apps. Keep Mirage Hub in the tray for automatic switching."));
        }
        if let Some(error) = &self.error {
            ui.colored_label(theme::DANGER, error);
        }
        changes
    }
}
