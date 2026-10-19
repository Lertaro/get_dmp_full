use std::{
    fmt::Display,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub(crate) struct ErrorLog {
    path: PathBuf,
}

impl ErrorLog {
    pub(crate) fn new(work_dir: &Path) -> io::Result<Self> {
        let path = work_dir.join("run.log");
        // A BOM lets Windows text editors recognize UTF-8 in any system locale.
        fs::write(&path, b"\xEF\xBB\xBF")?;
        Ok(Self { path })
    }

    pub(crate) fn report<T>(
        &self,
        module: &str,
        context: impl Display,
        result: io::Result<T>,
    ) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.error(module, context, error);
                None
            }
        }
    }

    pub(crate) fn error(&self, module: &str, context: impl Display, error: impl Display) {
        let message = format_record(module, format_args!("{context}: {error}"));
        eprintln!("{message}");
        // Open only while writing, so the completed log can be included in the ZIP.
        if let Err(error) = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .and_then(|mut file| write!(file, "{message}\r\n"))
        {
            eprintln!(
                "{}",
                format_record("LOGGER", format_args!("run.log: {error}"))
            );
        }
    }
}

pub(crate) fn format_record(module: &str, context: impl Display) -> String {
    let context = context
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!("[{}][ERROR][{module}]: {context}", local_timestamp())
}

pub(crate) fn local_timestamp() -> String {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }

    let mut time = SystemTime::default();
    // SAFETY: repr(C) matches Win32 SYSTEMTIME's eight WORD fields. The API
    // writes to this valid, exclusively borrowed buffer and retains no pointer.
    unsafe { GetLocalTime(&mut time) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        time.year, time.month, time.day, time.hour, time.minute, time.second, time.milliseconds,
    )
}
