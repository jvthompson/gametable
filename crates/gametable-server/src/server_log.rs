//! The server log: `YYYY/MM/DD HH:MM:SS message` lines (Go's `log.LstdFlags`) to stdout and
//! `server.log`, or nowhere (tests).

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

pub struct ServerLog {
    log_file: Option<Mutex<File>>,
    echo_to_stdout: bool,
}

impl ServerLog {
    /// Logs to stdout and appends to `path`.
    pub fn to_stdout_and_file(path: &Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { log_file: Some(Mutex::new(file)), echo_to_stdout: true })
    }

    /// Discards everything.
    pub fn silent() -> Self {
        Self { log_file: None, echo_to_stdout: false }
    }

    pub fn write(&self, message: &str) {
        if !self.echo_to_stdout && self.log_file.is_none() {
            return;
        }
        let now = jiff::ZonedDateTime::now();
        let line = format!("{} {message}\n", now.strftime("%Y/%m/%d %H:%M:%S"));
        if self.echo_to_stdout {
            print!("{line}");
        }
        if let Some(file) = &self.log_file {
            let _ = file.lock().unwrap().write_all(line.as_bytes());
        }
    }
}

/// `log!(server, "format", args...)`.
#[macro_export]
macro_rules! log {
    ($server:expr, $($argument:tt)*) => {
        $server.log.write(&format!($($argument)*))
    };
}
