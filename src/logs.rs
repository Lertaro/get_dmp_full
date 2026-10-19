use std::{fs, io, path::Path};

use crate::environment_path;
use crate::error_log::ErrorLog;

const SOURCES: [(&str, &str, &str); 2] = [
    ("LOCALAPPDATA", "Lertaro/logs", "logs/user"),
    (
        "ProgramData",
        "Lertaro/logs/service.log",
        "logs/service/service.log",
    ),
];

pub(crate) fn collect(work_dir: &Path, log: &ErrorLog) {
    for (variable, relative_source, relative_destination) in SOURCES {
        let Some(base) = log.report("LOGS", variable, environment_path(variable)) else {
            continue;
        };
        let source = base.join(relative_source);
        let destination = work_dir.join(relative_destination);
        let _ = log.report(
            "LOGS",
            source.display(),
            copy_path(&source, &destination, log),
        );
    }
}

pub(crate) fn copy_path(source: &Path, destination: &Path, log: &ErrorLog) -> io::Result<()> {
    if fs::symlink_metadata(source)?.is_dir() {
        let entries = fs::read_dir(source)?;
        fs::create_dir_all(destination)?;
        for entry in entries {
            if let Some(entry) = log.report("LOGS", source.display(), entry) {
                let path = entry.path();
                let _ = log.report(
                    "LOGS",
                    path.display(),
                    copy_path(&path, &destination.join(entry.file_name()), log),
                );
            }
        }
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
    }
    Ok(())
}
