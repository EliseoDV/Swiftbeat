use log::{Level, LevelFilter, Log, Metadata, Record};
use std::{fs::File, io::Write, sync::Mutex};

/// Log mínimo a %APPDATA%\Swiftbeat\swiftbeat.log (se sobrescribe en cada arranque).
struct FileLogger(Mutex<File>);

impl Log for FileLogger {
    fn enabled(&self, m: &Metadata) -> bool {
        let t = m.target();
        if t.starts_with("swiftbeat") || t.starts_with("librespot") {
            m.level() <= Level::Info
        } else {
            m.level() <= Level::Warn
        }
    }

    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() % 86_400)
            .unwrap_or(0);
        let line = format!(
            "{:02}:{:02}:{:02} {:5} {}: {}\n",
            secs / 3600,
            secs / 60 % 60,
            secs % 60,
            r.level(),
            r.target(),
            r.args()
        );
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
        if let Ok(mut f) = self.0.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    let dir = crate::config::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(file) = File::create(dir.join("swiftbeat.log")) {
        let _ = log::set_boxed_logger(Box::new(FileLogger(Mutex::new(file))));
        log::set_max_level(LevelFilter::Info);
    }
}
