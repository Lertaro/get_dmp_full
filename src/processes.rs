use std::{ffi::OsStr, fmt, io};

use crate::command;

const TARGET_NAMES: [&str; 2] = ["lertaro.app.exe", "lertaro.service.exe"];

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
