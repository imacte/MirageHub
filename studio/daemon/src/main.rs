//! Console entry point. Studio starts `openrazer-win daemon run` with
//! CREATE_NO_WINDOW, so the native daemon needs no helper or interpreter.
use openrazer_daemon::{cli, logging};
fn main() {
    let code = match cli::run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(error) => {
            log::error!("{error}");
            eprintln!("error: {error}");
            match error {
                openrazer_daemon::Error::Device(_) | openrazer_daemon::Error::NotSupported(_) => 3,
                _ => 2,
            }
        }
    };
    logging::finish();
    std::process::exit(code);
}
