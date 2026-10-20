use std::{
    ffi::{OsStr, OsString},
    fmt, io,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
};

use crate::{command, error_log::ErrorLog};

const TARGET_NAMES: [&str; 2] = ["lertaro.app.exe", "lertaro.service.exe"];

/// The access right that lets a name be read from any process handle, including one this
/// user's elevation level cannot otherwise touch.
const QUERY_LIMITED_INFORMATION: u32 = 0x1000;
/// The longest path Windows can name, in UTF-16 characters.
const MAX_PATH_CHARACTERS: usize = 32_768;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(
        desired_access: u32,
        inherit_handle: i32,
        process_id: u32,
    ) -> *mut core::ffi::c_void;
    fn QueryFullProcessImageNameW(
        process: *mut core::ffi::c_void,
        flags: u32,
        executable_name: *mut u16,
        characters: *mut u32,
    ) -> i32;
    fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
}

/// The folder `pid` was started from. A portable copy keeps its `Data` beside its executable, so
/// the folder is what says where that copy's logs are.
pub(crate) fn image_directory(pid: u32) -> io::Result<PathBuf> {
    // SAFETY: OpenProcess reports failure as a null handle, which is checked before the handle is
    // used or closed. `characters` holds the buffer's capacity when the query is made and the
    // number of characters it wrote when it returns, which is clamped before the buffer is read.
    unsafe {
        let process = OpenProcess(QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0u16; MAX_PATH_CHARACTERS];
        let mut characters = buffer.len() as u32;
        let written = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut characters);
        CloseHandle(process);
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        let image = PathBuf::from(OsString::from_wide(
            &buffer[..(characters as usize).min(buffer.len())],
        ));
        image
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .ok_or_else(|| io::Error::other("the image path has no folder"))
    }
}

/// The folder each running copy was started from, one entry per process. `None` is a copy whose
/// image path cannot be read, which the collectors answer as an installed one because the AppData
/// folders are then all there is to look at.
pub(crate) fn origins(running: &[Process], log: &ErrorLog) -> Vec<Option<PathBuf>> {
    running
        .iter()
        .map(|process| log.report("PROCESSES", process.pid, image_directory(process.pid)))
        .collect()
}

/// The `Data` folder of a portable copy, which exists only where one was unpacked or built; an
/// installed copy keeps the same state in the AppData folders. The log collector and the permission
/// report both ask, so the folder name is written down once.
pub(crate) fn portable_data(directory: &Path, log: &ErrorLog) -> Option<PathBuf> {
    let data = directory.join("Data");
    match log.report("PROCESSES", data.display(), data.try_exists()) {
        Some(true) => Some(data),
        // An absent folder names an installed copy, and an unreadable one leaves AppData as all
        // there is to go on. Both answers fall back rather than guess.
        _ => None,
    }
}

pub(crate) struct Process {
    pub name: &'static str,
    pub pid: u32,
}

impl fmt::Display for Process {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} (PID {})", self.name, self.pid)
    }
}

pub(crate) fn running() -> io::Result<Vec<Process>> {
    Ok(parse(&command::text(
        "tasklist.exe",
        &["/FO", "CSV", "/NH"],
    )?))
}

pub(crate) fn is_target(name: &str) -> bool {
    TARGET_NAMES
        .iter()
        .any(|target| name.eq_ignore_ascii_case(target))
}

/// WER's local crash dumps are named `<executable name>.<pid>.dmp`.
pub(crate) fn is_dump_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_lowercase();
    name.ends_with(".dmp")
        && TARGET_NAMES
            .iter()
            .any(|target| name.starts_with(&format!("{target}.")))
}

fn parse(output: &str) -> Vec<Process> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, ',');
            let image_name = fields.next()?.trim().trim_matches('"');
            let name = TARGET_NAMES
                .into_iter()
                .find(|name| name.eq_ignore_ascii_case(image_name))?;
            let pid = fields.next()?.trim().trim_matches('"').parse().ok()?;
            Some(Process { name, pid })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the query is that it answers for a process this program is not, so the
    /// test asks it about the only other one it can always see: itself.
    #[test]
    fn a_running_process_answers_with_its_own_folder() {
        let own = std::env::current_exe().unwrap();
        let found = image_directory(std::process::id()).unwrap();
        assert_eq!(
            found.to_string_lossy().to_ascii_lowercase(),
            own.parent().unwrap().to_string_lossy().to_ascii_lowercase()
        );
    }

    #[test]
    fn a_pid_that_is_not_running_has_no_folder() {
        assert!(image_directory(u32::MAX).is_err());
    }
}
