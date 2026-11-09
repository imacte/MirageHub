//! Console entry point. Studio starts `mirage-hub-daemon daemon run` with
//! CREATE_NO_WINDOW, so the native daemon needs no helper or interpreter.
use mirage_hub_daemon::{cli, logging};
fn main() {
    // Dedicated elevated entry point. No RPC, device discovery, settings migration or shell.
    if std::env::args_os().nth(1).as_deref()==Some(std::ffi::OsStr::new("--sony-audio-install")) {
        let args:Vec<_>=std::env::args_os().skip(2).collect();
        let result=if args.len()==1 {mirage_hub_daemon::inzone::apo::components::install_elevated(std::path::Path::new(&args[0]))} else {Err(mirage_hub_daemon::Error::Invalid("Expected Sony component directory".into()))};
        std::process::exit(match result {Ok(code)=>code,Err(error)=>{eprintln!("{error}");2}});
    }
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
