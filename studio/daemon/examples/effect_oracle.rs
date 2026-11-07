//! Offline differential runner. No hardware, keyboard hook or audio access.
use openrazer_daemon::effects::{
    frame::Frame,
    layers::{self, LayerStack},
    render::Renderer,
};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn run(request: Value) -> Value {
    let result = (|| -> openrazer_daemon::Result<Value> {
        if request["op"] == "keynames" {
            return Ok(json!((0..256)
                .map(|vk| [
                    openrazer_daemon::keyboard::matrix_key(vk, false),
                    openrazer_daemon::keyboard::matrix_key(vk, true)
                ])
                .collect::<Vec<_>>()));
        }
        if request["op"] == "layers" {
            let rows = request["rows"].as_u64().unwrap() as usize;
            let columns = request["columns"].as_u64().unwrap() as usize;
            let layers = layers::clean_layers(&request["layers"], rows, columns)?;
            let clean = serde_json::to_value(&layers)?;
            let stack = LayerStack::new(layers);
            let mut frame = Frame::new(rows, columns)?;
            let mut frames = Vec::new();
            for step in request["steps"].as_array().unwrap() {
                let presses: Vec<(f64, [usize; 2])> =
                    serde_json::from_value(step["presses"].clone())?;
                stack.render(&mut frame, step["elapsed"].as_f64().unwrap(), &presses);
                frames.push(frame.payload());
            }
            return Ok(
                json!({"clean":clean,"animated":stack.animated,"key_driven":stack.key_driven,"longest_press":stack.longest_press,"frames":frames}),
            );
        }
        let mut renderer = Renderer::new(
            request["effect"].as_str().unwrap(),
            request["options"].as_object().unwrap().clone(),
            &request["capabilities"],
        )?;
        let mut frames = Vec::new();
        for step in request["steps"].as_array().unwrap() {
            for key in step["keys"].as_array().unwrap() {
                renderer.on_key(
                    key["name"].as_str().unwrap(),
                    key["at"].as_f64().unwrap(),
                    key["hue"].as_f64().unwrap(),
                );
            }
            let accent = serde_json::from_value(step["accent"].clone())?;
            let changed = renderer.step(
                step["elapsed"].as_f64().unwrap(),
                step["peak"].as_f64().unwrap(),
                accent,
            );
            frames.push(json!({"changed":changed,"payload":renderer.frame.payload(),"phase":renderer.phase,"peak":renderer.recent_peak,"level":renderer.level}));
        }
        Ok(
            json!({"frames":frames,"still":renderer.still,"needs_keys":renderer.needs_keys(),"needs_audio":renderer.needs_audio()}),
        )
    })();
    match result {
        Ok(result) => json!({"ok":true,"result":result}),
        Err(_) => json!({"ok":false}),
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        serde_json::to_writer(&mut output, &run(serde_json::from_str(&line?)?))?;
        writeln!(&mut output)?;
        output.flush()?;
    }
    Ok(())
}
