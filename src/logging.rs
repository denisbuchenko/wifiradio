use std::{fs::OpenOptions, io::Write, time::SystemTime};

pub fn write(message: &str) {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let path = executable.with_file_name("wifiradio.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 64 * 1024) {
        let _ = std::fs::copy(&path, path.with_extension("previous.log"));
        let _ = std::fs::write(&path, []);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let _ = writeln!(
            file,
            "{time} pid={} v{} {message}",
            std::process::id(),
            env!("CARGO_PKG_VERSION")
        );
    }
}

pub fn initialize() {
    std::panic::set_hook(Box::new(|error| write(&format!("PANIC: {error}"))));
}
