//! Console entry point. Studio starts `mirage-hub-daemon daemon run` with
//! CREATE_NO_WINDOW, so the native daemon needs no helper or interpreter.
use mirage_hub_daemon::{cli, logging};
fn main() {
    let code = match cli::run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(error) => {
            log::error!("{error}");
            eprintln!("error: {error}");
            match error {
                mirage_hub_daemon::Error::Device(_) | mirage_hub_daemon::Error::NotSupported(_) => 3,
                _ => 2,
            }
        }
    };
    logging::finish();
    std::process::exit(code);
}
