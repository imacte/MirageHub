//! Development probe. Writes require an explicit subcommand; no driver installation here.
use mirage_hub_daemon::inzone::apo;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("read") => {}
        Some("prepare") => {
            if let Some(root) = args.get(1) {
                apo::components::prepare(std::path::Path::new(root))?;
                println!("Verified Sony components: {root}");
                return Ok(());
            }
            apo::components::start(false)?;
            loop {
                let state = apo::components::state();
                match state["phase"].as_str() {
                    Some("ready") => break,
                    Some("failed") => return Err(state.to_string().into()),
                    _ => std::thread::sleep(std::time::Duration::from_millis(250)),
                }
            }
        }
        Some("eq") | Some("bands") | Some("spatial") => {
            let method = match args[0].as_str() {
                "eq" => "set_sony_equalizer",
                "bands" => "set_sony_bands",
                _ => "set_sony_spatial",
            };
            let value = serde_json::from_str(args.get(1).ok_or("Expected JSON argument")?)?;
            apo::change(method, &value)?;
        }
        Some("restore") => {
            apo::change("restore_sony_audio", &serde_json::Value::Null)?;
        }
        _ => {
            return Err(
                "Usage: sony_audio [read|prepare|eq BOOL|bands JSON_ARRAY|spatial BOOL|restore]"
                    .into(),
            )
        }
    }
    println!("{}", serde_json::to_string_pretty(&apo::state())?);
    Ok(())
}
