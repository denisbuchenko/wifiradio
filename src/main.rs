#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod connection;
mod engine;
mod instance;
mod logging;
mod network;
mod protocol;
mod session;
mod startup;
mod ui;

fn main() {
    logging::initialize();
    let args: Vec<String> = std::env::args().collect();
    if let Some(at) = args.iter().position(|arg| arg == "--diagnostics") {
        let path = args
            .get(at + 1)
            .map(String::as_str)
            .unwrap_or("network-diagnostics.txt");
        let _ = std::fs::write(path, network::report());
        return;
    }
    if let Some(at) = args.iter().position(|arg| arg == "--self-test") {
        let path = args
            .get(at + 1)
            .map(String::as_str)
            .unwrap_or("self-test.txt");
        let result = engine::self_test();
        let _ = std::fs::write(path, &result);
        if !result.starts_with("PASS") {
            std::process::exit(1);
        }
        return;
    }
    ui::run(
        args.iter().any(|arg| arg == "--background"),
        args.iter().any(|arg| arg == "--no-autostart"),
        args.iter().any(|arg| arg == "--restart"),
    );
}
