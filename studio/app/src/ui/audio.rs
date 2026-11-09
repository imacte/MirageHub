//! A headset's sound: the settings it keeps itself, and where its mute button stands.
//!
//! Hardware settings follow the headset; Sony APO settings belong to this PC.

use super::app::App;
use super::device::TWO_COLUMNS;
use crate::worker::{Command, Detail, Device, Snapshot, Then};
use mirage_hub_core::theme::{self, icon, ACCENT};
use mirage_hub_core::widgets::{self, Button};
use mirage_hub_core::t;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[path = "inzone_volume.rs"]
mod inzone_volume;

/// How often to look at the mute button while its state is on screen.
const MIC_POLL: Duration = Duration::from_secs(3);

fn preset_label(preset: &str) -> &'static str {
    match preset {
        "flat" => t("Flat"),
        "game" => t("Game"),
        "music" => t("Music"),
        "movie" => t("Movie"),
        "custom" => t("Custom"),
        _ => "—",
    }
}

fn led_label(mode: &str) -> &'static str {
    match mode {
        "off" => t("Off"),
        "link" => t("Connection"),
        "battery" => t("Battery"),
        "warning" => t("Warnings"),
        _ => "—",
    }
}

fn band_label(frequency: i64) -> String {
    if frequency >= 1000 {
        format!("{} kHz", frequency / 1000)
    } else {
        format!("{frequency} Hz")
    }
}

fn names(list: &Value) -> Vec<String> {
    list.as_array().map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn call(serial: &str, method: &str, argument: Value) -> Command {
    Command::Call { serial: serial.into(), method: method.into(), args: vec![argument], zone: None, then: Then::Audio }
}

pub fn page(app: &mut App, ui: &mut egui::Ui, device: &Device, snap: &Snapshot) {
    let Some(detail) = snap.details.get(&device.serial) else { return };
    let Some(caps) = detail.caps.clone() else { return };
    if app.device_ui.audio_read.is_none_or(|at| at.elapsed() >= MIC_POLL) {
        app.device_ui.audio_read = Some(Instant::now());
        app.send(Command::ReadAudio(device.serial.clone()));
    }
    let Some(audio) = detail.audio.clone() else {
        ui.label(widgets::dim(t("Not reported yet.")));
        return;
    };
    if caps["protocol"] == "inzone" { return inzone(app, ui, device, &audio); }
    let tx = app.tx.clone();
    if !snap.synapse.is_empty() {
        widgets::banner(
            ui,
            theme::WARN,
            icon::WARNING,
            t("Razer Synapse is running"),
            t("With Synapse open, this headset's mute button can leave the microphone level in Windows at zero. Close it and the button simply mutes, and everything on this page still works."),
            |ui| {
                if ui.add(Button::primary(t("Close Synapse")).small()).clicked() {
                    let _ = tx.send(Command::CloseSynapse);
                }
            },
        );
    }
    let leds = names(&caps["audio"]["dongle_led_modes"]);
    let connected = audio["connected"] == true;
    if !connected {
        widgets::plain_panel(ui, |ui| widgets::empty_state(ui, t("The headset is switched off"), t("Switch it on to see and change its settings."), |_| {}));
        receiver(app, ui, device, &audio, &leds);
        return;
    }
    widgets::columns(ui, 0.46, TWO_COLUMNS, |ui, column| {
        if column == 0 {
            microphone(app, ui, device, &audio, &caps);
            receiver(app, ui, device, &audio, &leds);
        } else {
            equalizer(app, ui, device, &caps);
        }
    });
}

fn microphone(app: &mut App, ui: &mut egui::Ui, device: &Device, audio: &Value, caps: &Value) {
    let serial = device.serial.clone();
    let mut send: Vec<Command> = Vec::new();
    let mut later: Vec<(String, Command)> = Vec::new();
    let edit = app.edits.entry(serial.clone()).or_default();
    widgets::panel(ui, t("Microphone"), |ui| {
        widgets::row(ui, t("Right now"), t("The mute button on the headset decides this."), |ui| {
            if audio["microphone_muted"] == true {
                widgets::badge(ui, t("Muted"), theme::WARN);
            } else {
                widgets::badge(ui, t("On"), ACCENT);
            }
        });
        widgets::divider(ui);
        if widgets::toggle_row(ui, t("Hear your own voice"), t("The microphone played back quietly in the headset, so you do not shout."), &mut edit.audio.sidetone) {
            send.push(call(&serial, "set_sidetone", json!(edit.audio.sidetone)));
        }
        widgets::divider(ui);
        let top = caps["audio"]["sidetone_max"].as_f64().unwrap_or(15.0);
        widgets::row(ui, t("Level"), "", |ui| {
            ui.add_enabled_ui(edit.audio.sidetone, |ui| {
                if widgets::slider_field(ui, ("sidetone", &serial), &mut edit.audio.sidetone_level, 0.0..=top, 1.0, 0, "", 72.0) {
                    later.push((format!("sidetone:{serial}"), call(&serial, "set_sidetone_level", json!(edit.audio.sidetone_level.round() as i64))));
                }
            });
        });
        widgets::divider(ui);
        if widgets::toggle_row(ui, t("Do not disturb"), t("Silences Bluetooth calls while you are on the 2.4 GHz receiver."), &mut edit.audio.do_not_disturb) {
            send.push(call(&serial, "set_do_not_disturb", json!(edit.audio.do_not_disturb)));
        }
    });
    if !send.is_empty() || !later.is_empty() {
        edit.touch();
    }
    for command in send {
        app.send(command);
    }
    for (key, command) in later {
        app.debounce(key, command, 250);
    }
}

fn equalizer(app: &mut App, ui: &mut egui::Ui, device: &Device, caps: &Value) {
    let serial = device.serial.clone();
    let presets = names(&caps["audio"]["equalizer_presets"]);
    let frequencies: Vec<i64> = caps["audio"]["equalizer_frequencies"].as_array().map(|f| f.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    let low = caps["audio"]["equalizer_range"][0].as_f64().unwrap_or(-9.0);
    let high = caps["audio"]["equalizer_range"][1].as_f64().unwrap_or(6.0);
    let mut send: Vec<Command> = Vec::new();
    let mut later: Vec<(String, Command)> = Vec::new();
    let edit = app.edits.entry(serial.clone()).or_default();
    widgets::panel(ui, t("Equalizer"), |ui| {
        widgets::help(ui, t("Kept in the headset, so it also works on a phone or a console."));
        ui.add_space(4.0);
        if widgets::toggle_row(ui, t("Use the equalizer"), "", &mut edit.audio.equalizer) {
            send.push(call(&serial, "set_equalizer", json!(edit.audio.equalizer)));
        }
        widgets::divider(ui);
        ui.add_enabled_ui(edit.audio.equalizer, |ui| {
            let options: Vec<(String, String)> = presets.iter().map(|p| (p.clone(), preset_label(p).to_string())).collect();
            widgets::stacked(ui, t("Preset"), |ui| {
                if widgets::segmented(ui, &options, &mut edit.audio.preset) {
                    send.push(call(&serial, "set_equalizer_preset", json!(edit.audio.preset)));
                }
            });
            if edit.audio.preset == "custom" {
                widgets::divider(ui);
                let mut changed = false;
                for (band, frequency) in frequencies.iter().enumerate() {
                    let Some(gain) = edit.audio.bands.get_mut(band) else { continue };
                    widgets::row(ui, &band_label(*frequency), "", |ui| {
                        changed |= widgets::slider_field(ui, ("band", &serial, band), gain, low..=high, 1.0, 0, t("dB"), 72.0);
                    });
                }
                if changed {
                    let gains: Vec<i64> = edit.audio.bands.iter().map(|g| g.round() as i64).collect();
                    later.push((format!("bands:{serial}"), call(&serial, "set_equalizer_bands", json!(gains))));
                }
            }
        });
    });
    if !send.is_empty() || !later.is_empty() {
        edit.touch();
    }
    for command in send {
        app.send(command);
    }
    for (key, command) in later {
        app.debounce(key, command, 300);
    }
}

/// The light on the USB receiver, which answers with the headset off too.
fn receiver(app: &mut App, ui: &mut egui::Ui, device: &Device, audio: &Value, leds: &[String]) {
    if leds.is_empty() {
        return;
    }
    let serial = device.serial.clone();
    let current = audio["dongle_led"].as_str().unwrap_or_default().to_string();
    let mut chosen = None;
    widgets::panel(ui, t("Receiver"), |ui| {
        widgets::row(ui, t("Its light shows"), "", |ui| {
            widgets::select(ui, ("dongle-led", &serial), led_label(&current), ui.available_width().min(200.0), |ui| {
                for mode in leds {
                    if widgets::option(ui, *mode == current, led_label(mode)).clicked() {
                        chosen = Some(mode.clone());
                    }
                }
            });
        });
    });
    if let Some(mode) = chosen {
        app.send(call(&serial, "set_dongle_led", json!(mode)));
    }
}

/// The one sound setting worth having on Home.
pub fn quick(app: &mut App, ui: &mut egui::Ui, device: &Device, detail: &Detail, caps: &Value) {
    let Some(audio) = detail.audio.as_ref().filter(|audio| audio["connected"] == true) else {
        ui.label(widgets::dim(t("The headset is switched off")));
        return;
    };
    let serial = device.serial.clone();
    if caps["protocol"] == "inzone" {
        if let Some(settings) = audio["settings"].as_array() {
            for setting in settings.iter().filter(|s| matches!(s["key"].as_str(),Some("headphone_volume" | "game_chat_balance"))) {
                let key = setting["key"].as_str().unwrap_or_default();
                let value = if key == "headphone_volume" {
                    setting["value"].as_f64().map(|level| format!("{:.0}%", inzone_volume::to_percent(level))).unwrap_or_else(|| "—".into())
                } else { setting["value"].to_string() };
                widgets::fact(ui, inzone_label(key), &value);
            }
        }
        return;
    }
    let presets = names(&caps["audio"]["equalizer_presets"]);
    let current = if audio["equalizer"] == true { audio["equalizer_preset"].as_str().unwrap_or_default() } else { "" };
    let mut chosen = None;
    widgets::row(ui, t("Equalizer"), "", |ui| {
        widgets::select(ui, ("home-eq", &serial), if current.is_empty() { t("Off") } else { preset_label(current) }, ui.available_width(), |ui| {
            for preset in &presets {
                if widgets::option(ui, preset == current, preset_label(preset)).clicked() {
                    chosen = Some(preset.clone());
                }
            }
        });
    });
    if let Some(preset) = chosen {
        app.send(call(&serial, "set_equalizer_preset", json!(preset)));
    }
    widgets::row(ui, t("Microphone"), "", |ui| {
        if audio["microphone_muted"] == true {
            widgets::badge(ui, t("Muted"), theme::WARN);
        } else {
            widgets::badge(ui, t("On"), ACCENT);
        }
    });
}

fn inzone_label(key: &str) -> &'static str {
    match key {
        "headphone_volume" => t("Headphone volume"),
        "headphone_muted" => t("Mute headphones"),
        "game_chat_balance" => t("Game / chat balance"),
        "microphone_muted" => t("Microphone muted"),
        "microphone_level" => t("Microphone level"),
        "sidetone_level" => t("Hear your own voice"),
        "ambient_mode" => t("Noise control"),
        "ambient_level" => t("Ambient sound level"),
        "voice_focus" => t("Voice focus"),
        "auto_power_off" => t("Automatic power off"),
        "voice_guidance" => t("Voice guidance"),
        "voice_guidance_language" => t("Voice guidance language"),
        "bluetooth_auto_switch" => t("Bluetooth automatic switching"),
        _ => "",
    }
}

fn inzone_choice(key: &str, value: i64) -> String {
    match (key,value) {
        ("ambient_mode",0) => t("Off").into(),
        ("ambient_mode",1) => t("Noise cancelling").into(),
        ("ambient_mode",2) => t("Ambient sound").into(),
        ("voice_guidance_language",0) => t("English").into(),
        ("voice_guidance_language",1) => t("Japanese").into(),
        ("voice_guidance_language",2) => t("Simplified Chinese").into(),
        ("auto_power_off",0) => t("Never").into(),
        ("auto_power_off",minutes) => mirage_hub_core::tf("{} minutes", &[&minutes]),
        _ => value.to_string(),
    }
}

fn inzone(app: &mut App, ui: &mut egui::Ui, device: &Device, audio: &Value) {
    if let Some(error)=audio["error"].as_str() {
        widgets::plain_panel(ui,|ui| {widgets::help(ui,t("Not reported yet."));ui.label(error);});
        return;
    }
    if audio["connected"] != true {
        widgets::plain_panel(ui, |ui| widgets::empty_state(ui, t("The headset is switched off"), t("Switch it on to see and change its settings."), |_| {}));
        return;
    }
    let Some(settings) = audio["settings"].as_array() else { return };
    let serial = device.serial.clone();
    let edit = app.edits.entry(serial.clone()).or_default();
    let mut send = Vec::new();
    let mut later = Vec::new();
    widgets::columns(ui, 0.5, TWO_COLUMNS, |ui, column| {
        let is_sound = |key: &str| matches!(key,"headphone_volume"|"headphone_muted"|"game_chat_balance"|"microphone_muted"|"microphone_level"|"sidetone_level");
        let entries: Vec<_> = settings.iter().filter(|s| is_sound(s["key"].as_str().unwrap_or_default()) == (column==0)).collect();
        if !entries.is_empty() {
            widgets::panel(ui, if column==0 {t("Sound")} else {t("Headset settings")}, |ui| {
                for (index,setting) in entries.iter().enumerate() {
                    let key=setting["key"].as_str().unwrap_or_default();
                    if index>0 {widgets::divider(ui);}
                    let value=edit.audio.settings.entry(key.into()).or_insert_with(||setting["value"].as_f64().unwrap_or_default());
                    let hint=match key {
                        "game_chat_balance" => t("0: game only · 50: balanced · 100: chat only"),
                        "microphone_level" => t("Changes this headset's microphone level in Windows."),
                        "microphone_muted" if setting["readonly"]==true => t("Use the headset's physical microphone control."),
                        _ => "",
                    };
                    ui.add_enabled_ui(setting["readonly"]!=true && setting["enabled"]!=false, |ui| {
                        match setting["kind"].as_str() {
                            Some("toggle") => {
                                let mut on=*value!=0.0;
                                if widgets::toggle_row(ui,inzone_label(key),hint,&mut on) { *value=u8::from(on) as f64; send.push(call(&serial,setting["method"].as_str().unwrap_or_default(),json!(on))); }
                            }
                            Some("choice") => widgets::row(ui,inzone_label(key),hint,|ui| {
                                widgets::select(ui,("inzone-choice",&serial,key),inzone_choice(key,*value as i64),200.0,|ui| {
                                    for choice in setting["choices"].as_array().into_iter().flatten().filter_map(Value::as_i64) {
                                        if widgets::option(ui,*value as i64==choice,&inzone_choice(key,choice)).clicked() {*value=choice as f64;send.push(call(&serial,setting["method"].as_str().unwrap_or_default(),json!(choice)));}
                                    }
                                });
                            }),
                            _ => widgets::row(ui,inzone_label(key),hint,|ui| {
                                let changed = if key == "headphone_volume" {
                                    let mut percent = inzone_volume::to_percent(*value);
                                    if widgets::slider_field(ui,("inzone",&serial,key),&mut percent,0.0..=100.0,inzone_volume::PERCENT_STEP,0,"%",72.0) {
                                        let level = inzone_volume::from_percent(percent);
                                        let changed = level != *value;
                                        *value = level;
                                        changed
                                    } else { false }
                                } else {
                                    let range=setting["min"].as_f64().unwrap_or_default()..=setting["max"].as_f64().unwrap_or(100.0);
                                    widgets::slider_field(ui,("inzone",&serial,key),value,range,1.0,0,"",72.0)
                                };
                                if changed {later.push((format!("inzone:{serial}:{key}"),call(&serial,setting["method"].as_str().unwrap_or_default(),json!(value.round() as i64))));}
                            }),
                        }
                    });
                }
            });
        }
        if column==1 {
            inzone_battery(ui,audio);
        }
    });
    if !send.is_empty() || !later.is_empty() {edit.touch();}
    for command in send {app.send(command);}
    for (key,command) in later {app.debounce(key,command,250);}
    sony_apo(app,ui,device,&audio["sony_apo"]);
}

fn sony_apo(app: &mut App, ui: &mut egui::Ui, device: &Device, apo: &Value) {
    if apo.is_null() {return;}
    let serial=&device.serial;
    let edit=app.edits.entry(serial.clone()).or_default();
    let mut send=Vec::new();
    let mut bands_changed=false;
    widgets::panel(ui,t("Sony audio effects"),|ui| {
        widgets::help(ui,t("H5 Game output on this PC. Effects keep working after Mirage Hub exits."));
        if apo["hub_running"]==true {widgets::help(ui,t("Exit INZONE Hub from its tray menu before changing these settings."));}
        if apo["effects_disabled"]==true {widgets::help(ui,t("Enable audio enhancements for the H5 Game output in Windows Sound settings."));}
        if apo["available"]==true {
            if apo["needs_initialization"]==true {widgets::help(ui,t("The first change creates a Sony audio configuration for this Windows account."));}
            ui.add_enabled_ui(apo["hub_running"]!=true && apo["effects_disabled"]!=true,|ui| {
                if widgets::toggle_row(ui,t("Equalizer"),"",&mut edit.audio.sony_equalizer) {send.push(call(serial,"set_sony_equalizer",json!(edit.audio.sony_equalizer)));}
                ui.add_enabled_ui(edit.audio.sony_equalizer,|ui| {
                    let labels=["31.5 Hz","63 Hz","125 Hz","250 Hz","500 Hz","1 kHz","2 kHz","4 kHz","8 kHz","16 kHz"];
                    if edit.audio.sony_bands.len()==10 {
                        // Two columns also fit the minimum window width and translated labels.
                        ui.columns(2,|columns| {
                            for (i,value) in edit.audio.sony_bands.iter_mut().enumerate() {
                                widgets::row(&mut columns[i/5],labels[i],"",|ui| {
                                    bands_changed |= widgets::slider_field(ui,("sony-band",serial,i),value,-12.0..=12.0,0.5,1," dB",76.0);
                                });
                            }
                        });
                        if ui.add(Button::secondary(t("Flat"))).clicked() {edit.audio.sony_bands.fill(0.0);bands_changed=true;}
                    }
                });
                widgets::divider(ui);
                if apo["spatial"]=="custom" {widgets::help(ui,t("A custom Sony spatial profile is active. Switching this control selects the standard H5 profile."));}
                ui.add_enabled_ui(apo["resources"]==true,|ui| {
                    if widgets::toggle_row(ui,t("Standard spatial sound"),t("Uses Sony's standard H5 profile for the Game output."),&mut edit.audio.sony_spatial) {send.push(call(serial,"set_sony_spatial",json!(edit.audio.sony_spatial)));}
                });
                if apo["backup"]==true && ui.add(Button::secondary(t("Restore original audio settings"))).clicked() {send.push(call(serial,"restore_sony_audio",Value::Null));}
            });
        } else {
            widgets::help(ui,t("Sony APO is not ready. Connect one H5 receiver and select its 48 kHz Game output."));
            if let Some(error)=apo["error"].as_str() {ui.label(error);}
        }
        widgets::divider(ui);
        let components=&apo["components"];
        let phase=components["phase"].as_str().unwrap_or("absent");
        let busy=matches!(phase,"starting"|"downloading"|"verifying"|"extracting"|"installing");
        let status=match phase {
            "starting"|"downloading"=>t("Downloading Sony components…"),
            "verifying"=>t("Verifying Sony signatures and file hashes…"),
            "extracting"=>t("Preparing Sony audio resources…"),
            "ready"=>t("Sony components are ready."),
            "installing"=>t("Installing Sony drivers…"),
            "installed"=>t("Driver installation finished. Reconnect the receiver if effects are unavailable."),
            "restart_required"=>t("Sony driver installation requires a Windows restart."),
            "failed"=>t("Sony component operation failed."),
            _=>t("Sony components have not been downloaded.")
        };
        widgets::help(ui,status);
        if let Some(error)=components["error"].as_str().filter(|s|!s.is_empty()) {ui.label(error);}
        widgets::help(ui,t("Downloads Sony's official 138 MiB package and keeps about 2.9 MB of verified drivers and resources. INZONE Hub is not installed."));
        ui.hyperlink_to(t("Sony download and license information"),"https://www.sony.com/electronics/support/software/00279252");
        ui.add_enabled_ui(!busy,|ui| {
            if ui.add(Button::secondary(t("Prepare Sony audio components"))).clicked() {send.push(call(serial,"prepare_sony_audio",Value::Null));}
            if matches!(phase,"ready"|"installed"|"restart_required") {
                widgets::help(ui,t("Driver installation requires administrator approval and may reconnect audio. Independent installation on a clean PC is experimental."));
                if ui.add(Button::secondary(t("Install Sony drivers…"))).clicked() {send.push(call(serial,"install_sony_audio",Value::Null));}
            }
        });
    });
    if bands_changed {send.push(call(serial,"set_sony_bands",json!(edit.audio.sony_bands)));}
    if !send.is_empty() {edit.touch();}
    for command in send {
        if matches!(&command,Command::Call{method,..} if method=="restore_sony_audio") {app.cancel_pending(&format!("sony-bands:{serial}"));}
        if matches!(&command,Command::Call{method,..} if method=="set_sony_bands") {app.debounce(format!("sony-bands:{serial}"),command,300);} else {app.send(command);}
    }
}

pub fn inzone_battery(ui: &mut egui::Ui, audio: &Value) {
    widgets::panel(ui,t("Battery"),|ui| {
        let Some(parts)=audio["battery_parts"].as_array() else {widgets::help(ui,t("Not reported yet."));return;};
        for part in parts {
            let label=match part["part"].as_str() {Some("left")=>t("Left earbud"),Some("right")=>t("Right earbud"),Some("case")=>t("Charging case"),_=>t("Headset")};
            let value=part["percent"].as_u64().map(|v|format!("{v}%")).unwrap_or_else(||"—".into());
            widgets::fact(ui,label,&value);
            if part["snapshot"]==true {widgets::help(ui,t("Case battery is the last reading relayed by an earbud."));}
        }
    });
}
