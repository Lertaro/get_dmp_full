use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use crate::{error_log::ErrorLog, processes};

/// The log folders of an installed copy, each with the folder it becomes inside the bundle.
const APP_DATA: [(&str, &str, &str); 2] = [
    ("LOCALAPPDATA", "Lertaro/logs", "logs/user"),
    ("ProgramData", "Lertaro/logs", "logs/service"),
];

/// Collects the logs of every running copy from where it actually writes them.
///
/// A portable copy keeps its state in a `Data` folder beside its executable instead of in the
/// AppData folders. On a machine carrying both forms, reading only AppData put the logs of the
/// copy that never ran in the bundle, so each running copy is read from its own side.
pub(crate) fn collect(work_dir: &Path, log: &ErrorLog, origins: &[Option<PathBuf>]) {
    let (data, app_data) = sources(origins, log);

    for directory in data {
        collect_portable(work_dir, log, &directory);
    }
    if app_data {
        for (variable, relative_source, relative_destination) in APP_DATA {
            let Some(base) = log.environment_path("LOGS", variable) else {
                continue;
            };
            copy_logs(
                &base.join(relative_source),
                &work_dir.join(relative_destination),
                log,
            );
        }
    }
}

/// The log trees of the running copies: the `Data` folder of every portable copy, plus the AppData
/// folders unless every copy was seen to be portable. Nothing running means AppData alone.
fn sources(origins: &[Option<PathBuf>], log: &ErrorLog) -> (Vec<PathBuf>, bool) {
    let mut data: Vec<PathBuf> = Vec::new();
    let mut app_data = origins.is_empty();
    for origin in origins {
        match origin
            .as_deref()
            .and_then(|directory| processes::portable_data(directory, log))
        {
            Some(found) => push(&mut data, found),
            None => app_data = true,
        }
    }
    (data, app_data)
}

/// The two log trees under a `Data` folder, kept apart from the AppData ones so a machine that
/// runs one form and has the other installed loses neither.
fn collect_portable(work_dir: &Path, log: &ErrorLog, data: &Path) {
    // The executable folder names the copy inside the bundle, because two portable copies on one
    // machine each keep their own `Data`.
    let folder = data
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .unwrap_or("unknown");
    let root = work_dir.join("logs/portable").join(folder);
    copy_logs(&data.join("Machine/logs"), &root.join("machine"), log);
    let users = data.join("Users");
    let Some(entries) = log.report("LOGS", users.display(), fs::read_dir(&users)) else {
        return;
    };
    for entry in entries {
        let Some(entry) = log.report("LOGS", users.display(), entry) else {
            continue;
        };
        // Every user of a portable copy gets a folder of their own, named by a hash of their SID.
        copy_logs(
            &entry.path().join("logs"),
            &root.join("user").join(entry.file_name()),
            log,
        );
    }
}

/// A whole log tree, or nothing: a folder that is not there is recorded and skipped.
fn copy_logs(source: &Path, destination: &Path, log: &ErrorLog) {
    let _ = log.report(
        "LOGS",
        source.display(),
        copy_path(source, destination, log),
    );
}

/// Keeps one entry per `Data` folder, which several processes of the same copy point at.
fn push(list: &mut Vec<PathBuf>, data: PathBuf) {
    if !list.iter().any(|known| {
        known
            .to_string_lossy()
            .eq_ignore_ascii_case(&data.to_string_lossy())
    }) {
        list.push(data);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder holding a `Data` tree, or an empty one, standing in for a portable and an
    /// installed copy. Each test gets its own folder so they cannot see each other's fixtures.
    fn fixture(name: &str, portable: bool) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("get_dump_full-logs-{name}"));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        if portable {
            let logs = directory.join("Data/Machine/logs");
            fs::create_dir_all(&logs).unwrap();
            fs::write(logs.join("service.log"), "portable").unwrap();
        }
        directory
    }

    fn log_in(directory: &Path) -> ErrorLog {
        let work = directory.join("bundle");
        fs::create_dir_all(&work).unwrap();
        ErrorLog::new(&work).unwrap()
    }

    #[test]
    fn a_portable_copy_is_read_from_the_folder_beside_its_executable() {
        let portable = fixture("portable-only", true);
        let log = log_in(&portable);
        let (data, app_data) = sources(&[Some(portable.clone())], &log);
        assert_eq!(data, vec![portable.join("Data")]);
        assert!(!app_data, "AppData belongs to a copy that never ran here");
    }

    #[test]
    fn an_installed_copy_is_read_from_the_app_data_folders() {
        let installed = fixture("installed", false);
        let log = log_in(&installed);
        let (data, app_data) = sources(&[Some(installed)], &log);
        assert!(data.is_empty());
        assert!(app_data);
    }

    /// Both forms on one machine is the case that used to lose the portable logs, and a copy whose
    /// image path cannot be read is a third answer. All of them keep AppData in the bundle.
    #[test]
    fn a_machine_running_both_forms_reads_from_both() {
        let portable = fixture("both-portable", true);
        let installed = fixture("both-installed", false);
        let log = log_in(&portable);
        let (data, app_data) = sources(&[Some(portable.clone()), Some(installed), None], &log);
        assert_eq!(data, vec![portable.join("Data")]);
        assert!(app_data);
    }

    #[test]
    fn nothing_running_leaves_only_the_app_data_folders() {
        let directory = fixture("none", false);
        let log = log_in(&directory);
        let (data, app_data) = sources(&[], &log);
        assert!(data.is_empty());
        assert!(app_data);
    }

    /// Two processes of one copy name the same folder, and Windows spells it however the caller
    /// did, so the queue of folders to read is compared without regard to case.
    #[test]
    fn one_data_folder_is_read_once_however_many_processes_share_it() {
        let portable = fixture("deduplicated", true);
        let log = log_in(&portable);
        let upper = PathBuf::from(portable.to_string_lossy().to_uppercase());
        let (data, app_data) = sources(&[Some(portable.clone()), Some(upper)], &log);
        assert_eq!(data.len(), 1, "the same Data folder was queued twice");
        assert!(!app_data);
    }

    #[test]
    fn every_user_of_a_portable_copy_gets_its_own_tree() {
        let portable = fixture("users", true);
        let log = log_in(&portable);
        let work = portable.join("bundle");
        for hash in ["aaaa", "bbbb"] {
            let logs = portable.join("Data/Users").join(hash).join("logs");
            fs::create_dir_all(&logs).unwrap();
            fs::write(logs.join("app.log"), hash).unwrap();
        }
        // A folder of a user who never wrote a log has no `logs`, and is not in the bundle.
        fs::create_dir_all(portable.join("Data/Users/cccc")).unwrap();

        collect_portable(&work, &log, &portable.join("Data"));
        for hash in ["aaaa", "bbbb"] {
            assert_eq!(
                fs::read_to_string(
                    work.join("logs/portable")
                        .join(portable.file_name().unwrap())
                        .join("user")
                        .join(hash)
                        .join("app.log")
                )
                .unwrap(),
                hash
            );
        }
        assert!(!work.join("logs/portable").join("machine").exists());
    }

    #[test]
    fn an_absent_log_tree_is_recorded_instead_of_stopping_the_run() {
        let directory = fixture("absent", false);
        let log = log_in(&directory);
        let work = directory.join("bundle");
        copy_logs(
            &directory.join("Data/Machine/logs"),
            &work.join("machine"),
            &log,
        );
        assert_eq!(
            fs::read_to_string(work.join("run.log"))
                .unwrap()
                .matches("ERROR")
                .count(),
            1
        );
    }
}
