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

    /// The folder an environment variable names. An unset variable is a finding to record,
    /// not a reason to stop collecting, so it comes back as `None`.
    pub(crate) fn environment_path(&self, module: &str, variable: &str) -> Option<PathBuf> {
        let path = std::env::var_os(variable)
            .ok_or_else(|| io::Error::other("environment variable is not set"))
            .map(PathBuf::from);
        self.report(module, variable, path)
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
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }

    let mut time = SystemTime::default();
    // SAFETY: repr(C) matches Win32 SYSTEMTIME's eight WORD fields. The API
    // writes to this valid, exclusively borrowed buffer and retains no pointer.
    unsafe { GetLocalTime(&mut time) };
    format_system_time(&time)
}

/// A file's modification time as local wall-clock text, for naming something a person reads.
///
/// The time a file carries is an instant, and a bundle is read in the timezone it was collected
/// in: a save at 23:59 local must not be filed under the following day.
pub(crate) fn local_stamp_of(time: std::time::SystemTime) -> io::Result<String> {
    /// The 100-nanosecond intervals between the two epochs FILETIME and Unix use.
    const TICKS_BETWEEN_EPOCHS: u128 = 116_444_736_000_000_000;

    let since = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| io::Error::other("modification time is before 1970"))?;
    let ticks = TICKS_BETWEEN_EPOCHS
        + u128::from(since.as_secs()) * 10_000_000
        + u128::from(since.subsec_nanos()) / 100;
    let source = FileTime {
        low: ticks as u32,
        high: (ticks >> 32) as u32,
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FileTimeToLocalFileTime(source: *const FileTime, destination: *mut FileTime) -> i32;
        fn FileTimeToSystemTime(source: *const FileTime, destination: *mut SystemTime) -> i32;
    }

    let mut local = FileTime::default();
    let mut parts = SystemTime::default();
    // SAFETY: both structs are repr(C) with the fields their Win32 declarations give them, both
    // arguments are locals the caller owns, and each API only writes into the buffer it is handed.
    // A zero return is the failure these report, which is checked before the buffer is read.
    unsafe {
        if FileTimeToLocalFileTime(&source, &mut local) == 0 {
            return Err(io::Error::last_os_error());
        }
        if FileTimeToSystemTime(&local, &mut parts) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(format_system_time(&parts))
}

/// The Win32 `SYSTEMTIME`: local wall-clock fields, no timezone and no offset.
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

/// The Win32 `FILETIME`: 100-nanosecond intervals since 1601-01-01, as two halves of a 64-bit count.
#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}

fn format_system_time(time: &SystemTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        time.year, time.month, time.day, time.hour, time.minute, time.second, time.milliseconds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The FILETIME arithmetic is what a wrong epoch constant breaks, and a wrong constant reads as
    /// a plausible year rather than an error, so the year is checked against the clock of the run.
    #[test]
    fn a_file_time_comes_out_as_local_text() {
        let path = std::env::temp_dir().join("get_dump_full-stamp.txt");
        fs::write(&path, "now").unwrap();
        let stamp = local_stamp_of(fs::metadata(&path).unwrap().modified().unwrap()).unwrap();
        let _ = fs::remove_file(&path);
        let today = local_timestamp();
        let year = |text: &str| text[..4].parse::<u16>().unwrap();
        assert!(
            year(&today).saturating_sub(year(&stamp)) <= 1,
            "{stamp} is not this year, which the run began in ({today})"
        );
        assert_eq!(
            stamp
                .chars()
                .map(|character| if character.is_ascii_digit() {
                    '#'
                } else {
                    character
                })
                .collect::<String>(),
            "####-##-## ##:##:##.###"
        );
    }

    /// A time the conversion cannot name comes back as an error, which the caller answers by
    /// keeping the folder's own name rather than inventing a date.
    #[test]
    fn a_time_before_the_unix_epoch_has_no_stamp() {
        assert!(local_stamp_of(std::time::UNIX_EPOCH - std::time::Duration::from_secs(1)).is_err());
    }
}
