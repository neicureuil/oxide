//! Local diagnostic logs in `~/Library/Logs/Oxide`, or `$OXIDE_LOG_DIR` (cargo runs, see `.cargo/config.toml`):
//! never tokens or message content.
use std::io::Write;
use std::sync::{OnceLock, mpsc};

/// One log file, written by its own background thread, never on the caller's (async or state) thread.
pub struct Log {
    name: &'static str,
    writer: OnceLock<Option<mpsc::Sender<String>>>,
}

impl Log {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            writer: OnceLock::new(),
        }
    }

    /// Queues the line, prefixed with the Unix time.
    pub fn record(&self, message: &str) {
        let Some(writer) = self.writer.get_or_init(|| spawn_writer(self.name)) else {
            return;
        };
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let _ = writer.send(format!("{time} {message}\n"));
    }
}

fn spawn_writer(name: &str) -> Option<mpsc::Sender<String>> {
    let directory = match std::env::var_os("OXIDE_LOG_DIR") {
        Some(directory) => std::path::PathBuf::from(directory),
        None => std::path::PathBuf::from(std::env::var_os("HOME")?).join("Library/Logs/Oxide"),
    };
    let path = directory.join(name);
    let (sender, lines) = mpsc::channel::<String>();
    std::thread::Builder::new()
        .name(name.replace('.', "-"))
        .spawn(move || {
            let mut file = None;
            for line in lines {
                // Reopen when the log was removed, as the per-line open did.
                if file.is_none() || !path.exists() {
                    let _ = std::fs::create_dir_all(&directory);
                    let mut options = std::fs::OpenOptions::new();
                    options.create(true).append(true);
                    // Owner-only on Unix; Windows keeps the user profile's default access.
                    #[cfg(unix)]
                    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
                    file = options.open(&path).ok();
                }
                if let Some(file) = file.as_mut() {
                    let _ = file.write_all(line.as_bytes());
                }
            }
        })
        .ok()?;
    Some(sender)
}
