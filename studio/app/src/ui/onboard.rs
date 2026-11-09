//! The mouse's onboard mappings and macros. PC profiles are independent.
use super::app::App;
use crate::worker::{Command, Device, Snapshot};
use egui::RichText;
use mirage_hub_core::{t, tf, theme, widgets::{self, Button}};
use serde_json::{json, Value};

#[derive(Clone)]
struct Edit {
    button: u8, layer: u8, store: u8, kind: u8, key: String, modifiers: u8,
    mouse: u8, binding_macro: u16, held: bool, macro_id: u16, events: String,
    requested: Option<(u8,u8,u8)>,
}
impl Default for Edit {
    fn default() -> Self { Self {
        button:0x50, layer:0, store:1, kind:0, key:"A".into(), modifiers:0,
        mouse:4, binding_macro:1, held:false, macro_id:1,
        events:"down A\nwait 50\nup A".into(), requested:None,
    } }
}
fn selected(binding: &Value, edit: &Edit) -> bool {
    binding["button"] == edit.button && binding["layer"] == edit.layer && binding["store"] == edit.store
}
fn segments(ui: &mut egui::Ui, options: &[(u8,&str)], selected: &mut u8) {
    let options: Vec<_> = options.iter().map(|(value,label)| (*value,label.to_string())).collect();
    widgets::segmented(ui,&options,selected);
}
fn key_name(usage: u64) -> String {
    match usage {
        4..=29 => char::from(b'A'+usage as u8-4).to_string(),
        30..=38 => (usage-29).to_string(), 39 => "0".into(),
        40 => "Enter".into(), 41 => "Esc".into(), 42 => "Backspace".into(), 43 => "Tab".into(), 44 => "Space".into(),
        58..=69 => format!("F{}",usage-57), 104..=115 => format!("F{}",usage-91),
        _ => format!("HID 0x{usage:02X}"),
    }
}
fn action_label(action: &Value) -> String {
    let Some(bytes) = action.as_array() else { return t("Not reported yet.").into() };
    let byte = |i| bytes.get(i).and_then(Value::as_u64).unwrap_or(0);
    match byte(0) {
        0 => t("Disabled").into(),
        1 => tf("Mouse button {}", &[&byte(2)]),
        2 => {
            let modifiers = byte(2);
            let mut parts: Vec<String> = [(1,"Ctrl"),(2,"Shift"),(4,"Alt"),(8,"Win"),(16,"Right Ctrl"),(32,"Right Shift"),(64,"Right Alt"),(128,"Right Win")]
                .into_iter().filter(|(mask,_)| modifiers & mask != 0).map(|(_,name)|name.to_string()).collect();
            parts.push(key_name(byte(3))); parts.join(" + ")
        }
        3 | 4 => tf("Onboard macro {}", &[&(byte(2)*256+byte(3))]),
        _ => bytes.iter().filter_map(Value::as_u64).map(|v| format!("{v:02X}")).collect::<Vec<_>>().join(" "),
    }
}
fn mapping(edit: &Edit, serial: &str) -> Command {
    let (method,args) = match edit.kind {
        0 => ("set_button_key",json!([edit.button,edit.key,edit.modifiers,edit.layer,edit.store])),
        1 => ("set_button_mouse",json!([edit.button,edit.mouse,edit.layer,edit.store])),
        2 => ("set_button_macro",json!([edit.button,edit.binding_macro,edit.layer,edit.held,edit.store])),
        _ => ("set_button_action",json!([edit.button,[0,0],edit.layer,edit.store])),
    };
    Command::Onboard {serial:serial.into(),method:method.into(),args:args.as_array().unwrap().clone()}
}

pub fn page(app: &mut App, ui: &mut egui::Ui, device: &Device, snap: &Snapshot) {
    let Some(detail) = snap.details.get(&device.serial) else { return };
    let Some(caps) = detail.caps.as_ref().filter(|c| c["onboard_buttons"] == true) else { return };
    let id = egui::Id::new(("onboard-editor",&device.serial));
    let mut edit = ui.data_mut(|data| data.get_temp::<Edit>(id).unwrap_or_default());
    let mut refresh = false;
    widgets::panel(ui,t("Onboard memory"),|ui| {
        widgets::help(ui,t("These mappings and macros are saved in the mouse and work after the daemon exits. They are independent of PC profiles."));
        if let Some(error) = detail.onboard["error"].as_str() { ui.label(RichText::new(error).color(theme::WARN)); }
        let plate = match detail.onboard["side_plate"].as_u64() {
            Some(0) => t("No side plate"), Some(1) => t("2-button side plate"), Some(3) => t("12-button side plate"), Some(4) => t("6-button side plate"), _ => t("Not reported yet."),
        };
        ui.horizontal(|ui| {
            ui.label(widgets::dim(plate));
            if ui.add(Button::secondary(t("Refresh")).enabled(!snap.busy)).clicked() { refresh = true; }
            if snap.busy { ui.spinner(); ui.label(widgets::dim(t("Working with device memory…"))); }
        });
    });
    ui.add_enabled_ui(!snap.busy,|ui| {
        widgets::panel(ui,t("Onboard button mapping"),|ui| {
            let stores: Vec<(u8,String)> = (1..=5).map(|s| (s, if s == 1 {t("Live memory (slot 1)").into()} else {tf("Onboard slot {}",&[&s])})).collect();
            widgets::row(ui,t("Memory slot"),t("Slot 1 is active and survives power cycles. Slots 2–5 hold other bindings."),|ui| { widgets::segmented(ui,&stores,&mut edit.store); });
            widgets::row(ui,t("Key layer"),"",|ui| { segments(ui,&[(0,t("Standard")),(1,"Hypershift")],&mut edit.layer); });
            let buttons: Vec<u8> = caps["onboard_button_ids"].as_array().into_iter().flatten().filter_map(Value::as_u64).filter_map(|v|u8::try_from(v).ok()).collect();
            widgets::row(ui,t("Button ID"),t("Button IDs include all interchangeable side plates."),|ui| {
                widgets::select(ui,"onboard-button",format!("0x{:02X}",edit.button),130.0,|ui| {
                    for button in &buttons { if widgets::option(ui,edit.button == *button,&format!("0x{button:02X}")).clicked() { edit.button = *button; } }
                });
            });
            if selected(&detail.onboard_binding,&edit) {
                if let Some(error) = detail.onboard_binding["error"].as_str() { ui.label(RichText::new(error).color(theme::WARN)); }
                else { ui.label(widgets::dim(tf("Current action: {}",&[&action_label(&detail.onboard_binding["action"])]))); }
            }
            widgets::divider(ui);
            segments(ui,&[(0,t("Keyboard key")),(1,t("Mouse button")),(2,t("Onboard macro")),(3,t("Disabled"))],&mut edit.kind);
            match edit.kind {
                0 => {
                    widgets::row(ui,t("Keyboard key"),t("Examples: A, Space, F5, LeftCtrl"),|ui| { widgets::text_field(ui,&mut edit.key,"A",180.0); });
                    ui.horizontal_wrapped(|ui| {
                        for (mask,name) in [(1,"Ctrl"),(2,"Shift"),(4,"Alt"),(8,"Win")] {
                            let mut checked = edit.modifiers & mask != 0;
                            if ui.checkbox(&mut checked,name).changed() { if checked { edit.modifiers |= mask; } else { edit.modifiers &= !mask; } }
                        }
                    });
                }
                1 => { segments(ui,&[(1,t("Left")),(2,t("Right")),(3,t("Middle")),(4,t("Back")),(5,t("Forward"))],&mut edit.mouse); }
                2 => {
                    ui.horizontal(|ui| { ui.label(t("Macro ID")); ui.add(egui::DragValue::new(&mut edit.binding_macro).range(1..=65535)); ui.checkbox(&mut edit.held,t("Repeat while held")); });
                }
                _ => {}
            }
            if ui.add(Button::primary(t("Save to mouse"))).clicked() { app.send(mapping(&edit,&device.serial)); }
        });
        widgets::panel(ui,t("Onboard macros"),|ui| {
            let inventory = &detail.onboard["inventory"];
            let ids = inventory["ids"].as_array().map(|ids| ids.iter().map(Value::to_string).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            ui.label(widgets::dim(tf("Stored macro IDs: {}",&[&ids])));
            if inventory["complete"] == false { ui.label(RichText::new(t("The device omitted some macro IDs. A missing ID may still be in use.")).color(theme::WARN)); }
            let storage = &detail.onboard["storage"];
            if let (Some(free),Some(total)) = (storage["free"].as_u64(),storage["total"].as_u64()) { ui.label(widgets::dim(tf("{} of {} bytes free",&[&free,&total]))); }
            widgets::help(ui,t("Upload to an unused ID, then choose it in a button mapping. A bound macro cannot be deleted."));
            ui.horizontal(|ui| { ui.label(t("Macro ID")); ui.add(egui::DragValue::new(&mut edit.macro_id).range(1..=65535)); });
            widgets::help(ui,t("One step per line: down A, wait 50, up A. Delays are in milliseconds."));
            ui.add(egui::TextEdit::multiline(&mut edit.events).font(egui::TextStyle::Monospace).desired_rows(5).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                if ui.add(Button::primary(t("Upload macro"))).clicked() {
                    app.send(Command::Onboard {serial:device.serial.clone(),method:"add_onboard_macro".into(),args:vec![json!(edit.macro_id),json!(edit.events)]});
                }
                if ui.add(Button::secondary(t("Delete macro"))).clicked() {
                    app.send(Command::Onboard {serial:device.serial.clone(),method:"delete_onboard_macro".into(),args:vec![json!(edit.macro_id)]});
                }
            });
        });
    });
    let selection = (edit.button,edit.layer,edit.store);
    if !snap.busy && (refresh || edit.requested != Some(selection)) {
        app.send(Command::ReadOnboard {serial:device.serial.clone(),button:edit.button,layer:edit.layer,store:edit.store});
        edit.requested = Some(selection);
    }
    ui.data_mut(|data| data.insert_temp(id,edit));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readback_is_scoped_to_button_layer_and_store_and_mapping_keeps_the_selection() {
        let edit = Edit {button:0x55,layer:1,store:5,kind:2,binding_macro:0x1234,held:true,..Edit::default()};
        assert!(selected(&json!({"button":0x55,"layer":1,"store":5}),&edit));
        assert!(!selected(&json!({"button":0x55,"layer":0,"store":5}),&edit));
        assert!(!selected(&json!({"button":0x55,"layer":1,"store":1}),&edit));
        let Command::Onboard {method,args,..} = mapping(&edit,"S") else { panic!() };
        assert_eq!(method,"set_button_macro");
        assert_eq!(args,json!([0x55,0x1234,1,true,5]).as_array().unwrap().clone());
        assert_eq!(action_label(&json!([2,2,1,4])),"Ctrl + A");
        assert_eq!(key_name(39),"0");
    }
}
