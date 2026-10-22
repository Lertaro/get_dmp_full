use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    error_log::{self, ErrorLog},
    processes,
};

/// The state folders of an installed copy, each with the folder it becomes inside the bundle.
/// A portable copy keeps the same state under its own `Data`.
const APP_DATA: [(&str, &str, &str); 2] = [
    ("LOCALAPPDATA", "Lertaro", "state/user"),
    ("ProgramData", "Lertaro", "state/service"),
];

/// The settings files a copy keeps inside a state folder, without their extension: every rotation
/// the app writes hangs off one of these stems.
const SETTINGS: [&str; 3] = ["user-settings", "plugin-settings", "machine-settings"];

/// The ranking calibration log, which rotates its own way: the app moves `ranking-events.jsonl`
/// aside as `ranking-events.<number>.jsonl` once it passes its size cap, and keeps two of those.
/// Its queries and paths are truncated hashes, so it says how the ranking behaved without naming
/// what the user typed or owns.
const EVENTS: &str = "ranking-events";
const EVENTS_EXTENSION: &str = "jsonl";

/// The two files that say when a folder's settings were last written, in the order to prefer. A
/// folder of neither holds only rotations, whose newest is the next best thing to read.
const PRIMARY_SETTINGS: [&str; 2] = ["user-settings.json", "machine-settings.json"];

/// How much of a state folder's own name survives into its dated name inside the bundle.
const NAME_PREFIX: usize = 8;

/// The file that records which dated folder came from where, under each copy's folder.
const FOLDER_NAMES: &str = "folders.txt";

/// Collects the logs and the data files of every running copy from where it actually writes them.
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
            // One identity, one folder: the installed copy keeps a single account's state here, so
            // it needs none of the dating a portable `Data` folder has to do without.
            collect_state(
                &work_dir.join(relative_destination),
                &base.join(relative_source),
                log,
            );
        }
    }
}

/// The state trees of the running copies: the `Data` folder of every portable copy, plus the AppData
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

/// The logs and data files under a `Data` folder, kept apart from the AppData ones so a machine
/// that runs one form and has the other installed loses neither.
fn collect_portable(work_dir: &Path, log: &ErrorLog, data: &Path) {
    // The executable folder names the copy inside the bundle, because two portable copies on one
    // machine each keep their own `Data`.
    let folder = data
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .unwrap_or("unknown");
    let root = work_dir.join("state/portable").join(folder);
    let mut named = Vec::new();

    // One account's state is one folder of logs and one of data files, and the machine's state has
    // that shape too, so both go through the same door. The machine needs no dated name: there is
    // one of it per copy.
    let machine = data.join("Machine");
    collect_state(&root.join("machine"), &machine, log);
    named.push(("machine".to_owned(), machine));

    let users = data.join("Users");
    let Some(entries) = log.report("LOGS", users.display(), fs::read_dir(&users)) else {
        write_names(&root, &named, log);
        return;
    };
    for entry in entries {
        let Some(entry) = log.report("LOGS", users.display(), entry) else {
            continue;
        };
        // Every user of a portable copy gets a folder of their own, named by a hash of their SID.
        let source = entry.path();
        let name = label(
            settings_stamp(&source, log),
            &entry.file_name().to_string_lossy(),
        );
        collect_state(&root.join("user").join(&name), &source, log);
        named.push((name, source));
    }
    write_names(&root, &named, log);
}

/// Everything worth reading from one state folder, wherever that folder lives.
fn collect_state(destination: &Path, source: &Path, log: &ErrorLog) {
    copy_logs(&source.join("logs"), &destination.join("logs"), log);
    copy_data(source, &destination.join("data"), log);
}

/// The name a state folder gets inside the bundle.
///
/// Several folders under one `Data` hold different generations of settings: one account has a
/// folder per hash spelling, and the service adds a folder of its own for SYSTEM. A hash of a SID
/// cannot say which of those is the newest, and the date can. The prefix of the original name
/// stays on the end because two accounts do save within the same second, when an update lands, and
/// a bare date would collide.
fn label(stamp: Option<String>, name: &str) -> String {
    match stamp {
        Some(stamp) => format!(
            "{stamp}__{}",
            name.chars().take(NAME_PREFIX).collect::<String>()
        ),
        None => name.to_owned(),
    }
}

/// The local time the settings of one folder were last written, or nothing when it holds none.
///
/// Only the settings files answer this. The ranking log is appended to while the app is merely
/// being used, so a folder dated from it would read as though settings were written when none were.
fn settings_stamp(source: &Path, log: &ErrorLog) -> Option<String> {
    let mut found: Vec<(String, SystemTime)> = Vec::new();
    let entries = log.report("LOGS", source.display(), fs::read_dir(source))?;
    for entry in entries {
        let Some(entry) = log.report("LOGS", source.display(), entry) else {
            continue;
        };
        let name = entry.file_name();
        if !is_settings(&name) {
            continue;
        }
        let Some(time) = log.report(
            "LOGS",
            entry.path().display(),
            entry.metadata().and_then(|metadata| metadata.modified()),
        ) else {
            continue;
        };
        found.push((name.to_string_lossy().into_owned(), time));
    }
    // The live file says when the settings were written; a folder of only rotations says when the
    // last backup was made, which is the next best thing to read.
    let time = PRIMARY_SETTINGS
        .iter()
        .find_map(|primary| found.iter().find(|(name, _)| name == primary))
        .or_else(|| found.iter().max_by_key(|(_, time)| *time))
        .map(|(_, time)| *time)?;
    // A Windows folder name holds neither of the two characters a readable time is made of.
    Some(
        error_log::local_stamp_of(time)
            .ok()?
            .replace([':', '.'], "-")
            .replace(' ', "_"),
    )
}

/// The settings and the ranking log of one folder, copied flat into `destination`. Nothing else in
/// the folder: a portable copy keeps a TLS private key and an index cache next door, and the bundle
/// goes to someone else.
fn copy_data(source: &Path, destination: &Path, log: &ErrorLog) {
    // A folder of an account that never saved has no settings, which is a normal answer and not a
    // failure to record.
    if !log
        .report("LOGS", source.display(), source.try_exists())
        .unwrap_or(false)
    {
        return;
    }
    let Some(entries) = log.report("LOGS", source.display(), fs::read_dir(source)) else {
        return;
    };
    for entry in entries {
        let Some(entry) = log.report("LOGS", source.display(), entry) else {
            continue;
        };
        if !is_collected(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let _ = log.report(
            "LOGS",
            path.display(),
            fs::create_dir_all(destination)
                .and_then(|()| fs::copy(&path, destination.join(entry.file_name()))),
        );
    }
}

/// Whether a name is a file the bundle wants: a settings file or the ranking log, with the
/// rotations either of them writes.
fn is_collected(name: &OsStr) -> bool {
    is_settings(name) || is_events(name)
}

/// Whether a name is one of the settings files or a rotation of one.
fn is_settings(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    SETTINGS.iter().any(|stem| {
        let Some(rest) = name.strip_prefix(stem) else {
            return false;
        };
        match rest.strip_prefix(".json") {
            // `<stem>.json`, and the `<stem>.json.bak.<number>` a save rotates out of the way.
            Some(suffix) => suffix.is_empty() || is_backup(suffix),
            // `<stem>.legacy-<number>.json`, what a migration leaves where the name was taken.
            None => is_number(
                rest.strip_prefix(".legacy-")
                    .and_then(|digits| digits.strip_suffix(".json"))
                    .unwrap_or_default(),
            ),
        }
    })
}

/// `ranking-events.jsonl`, and the `ranking-events.<number>.jsonl` its size rotation leaves behind.
fn is_events(name: &OsStr) -> bool {
    let Some(rest) = name
        .to_str()
        .and_then(|name| name.strip_prefix(EVENTS))
        .and_then(|rest| rest.strip_prefix('.'))
    else {
        return false;
    };
    rest == EVENTS_EXTENSION
        || rest
            .strip_suffix(&format!(".{EVENTS_EXTENSION}"))
            .is_some_and(is_number)
}

fn is_backup(suffix: &str) -> bool {
    suffix.strip_prefix(".bak.").is_some_and(is_number)
}

/// A run of digits, and not an empty one: `.bak.` on its own is not a rotation.
fn is_number(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|character| character.is_ascii_digit())
}

/// What each folder inside the bundle really was. The dated names are for reading; the hash is the
/// key the app itself resolves, so the whole of it survives here.
fn write_names(root: &Path, named: &[(String, PathBuf)], log: &ErrorLog) {
    let mut text = String::from("\u{feff}");
    for (label, source) in named {
        text.push_str(&format!("{label}\t{}\r\n", source.display()));
    }
    let _ = log.report(
        "LOGS",
        root.display(),
        fs::write(root.join(FOLDER_NAMES), text),
    );
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

    /// Where one portable copy's tree lands inside a bundle: the `Data` folder's own parent names it.
    fn portable_root(work: &Path, portable: &Path) -> PathBuf {
        work.join("state/portable")
            .join(portable.file_name().unwrap())
    }

    /// The one account folder a fixture made, whatever name it was given inside the bundle.
    fn only_account(root: &Path) -> PathBuf {
        let entries = fs::read_dir(root.join("user"))
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1, "one account became more than one folder");
        entries.into_iter().next().unwrap().path()
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
        // A folder that holds neither a log nor a setting is visited and lands nowhere.
        fs::create_dir_all(portable.join("Data/Users/cccc")).unwrap();

        collect_portable(&work, &log, &portable.join("Data"));
        let root = portable_root(&work, &portable);
        for hash in ["aaaa", "bbbb"] {
            assert_eq!(
                fs::read_to_string(root.join("user").join(hash).join("logs/app.log")).unwrap(),
                hash
            );
        }
        assert!(!root.join("user/cccc").exists());
        assert_eq!(
            fs::read_to_string(root.join("machine/logs/service.log")).unwrap(),
            "portable"
        );
    }

    /// The hash is why a bundle cannot be read as a timeline: the same machine holds one folder per
    /// hash spelling of an account and one for the service's SYSTEM identity, each a different
    /// generation of settings. The date says which is which.
    #[test]
    fn an_account_folder_is_named_for_the_day_its_settings_were_written() {
        let portable = fixture("dated", true);
        let log = log_in(&portable);
        let work = portable.join("bundle");
        let account = portable.join("Data/Users").join("aaaa");
        fs::create_dir_all(&account).unwrap();
        fs::write(account.join("user-settings.json"), "{}").unwrap();

        collect_portable(&work, &log, &portable.join("Data"));
        let root = portable_root(&work, &portable);
        let dated = only_account(&root);
        let name = dated.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            name.chars()
                .map(|character| if character.is_ascii_digit() {
                    '#'
                } else {
                    character
                })
                .collect::<String>(),
            "####-##-##_##-##-##-###__aaaa",
            "the folder is not the settings date followed by the name it came from"
        );
        assert_eq!(
            fs::read_to_string(dated.join("data/user-settings.json")).unwrap(),
            "{}"
        );

        let mapping = fs::read_to_string(root.join(FOLDER_NAMES)).unwrap();
        let line = mapping
            .lines()
            .find(|line| line.starts_with(&name))
            .expect("the dated folder is not in the mapping");
        assert!(
            line.ends_with("aaaa"),
            "the mapping does not say which folder a dated name came from: {line}"
        );
    }

    /// The whitelist is the whole point: the folder holding these files also holds a private key
    /// and an index cache, and the ZIP goes to somebody else.
    #[test]
    fn only_the_listed_files_are_taken_from_an_account_folder() {
        let portable = fixture("whitelist", true);
        let log = log_in(&portable);
        let work = portable.join("bundle");
        let account = portable.join("Data/Users").join("bbbb");
        fs::create_dir_all(account.join("ContentIndex")).unwrap();
        for name in [
            "user-settings.json",
            "user-settings.json.bak.1",
            "plugin-settings.json",
            "user-settings.legacy-1.json",
            "ranking-events.jsonl",
            "ranking-events.2.jsonl",
            "user-settings.json.tmp",
            "ranking-events.jsonl.tmp",
            "localsend.pfx",
        ] {
            fs::write(account.join(name), name).unwrap();
        }
        fs::write(account.join("ContentIndex/index.bin"), "cache").unwrap();

        collect_portable(&work, &log, &portable.join("Data"));
        let mut taken = fs::read_dir(only_account(&portable_root(&work, &portable)).join("data"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        taken.sort();
        assert_eq!(
            taken,
            [
                "plugin-settings.json",
                "ranking-events.2.jsonl",
                "ranking-events.jsonl",
                "user-settings.json",
                "user-settings.json.bak.1",
                "user-settings.legacy-1.json"
            ]
        );
    }

    /// A folder of the account that only ever searched: the log is taken, and the folder keeps its
    /// hash name, because a file written by ordinary use would date it to a moment when nothing was
    /// saved.
    #[test]
    fn a_folder_of_only_the_ranking_log_is_taken_but_not_dated() {
        let portable = fixture("events-only", true);
        let log = log_in(&portable);
        let work = portable.join("bundle");
        let account = portable.join("Data/Users").join("cccc");
        fs::create_dir_all(&account).unwrap();
        fs::write(account.join("ranking-events.jsonl"), "{}\n").unwrap();

        collect_portable(&work, &log, &portable.join("Data"));
        let taken = only_account(&portable_root(&work, &portable));
        assert_eq!(
            taken.file_name().unwrap().to_string_lossy(),
            "cccc",
            "the log was allowed to name the folder"
        );
        assert_eq!(
            fs::read_to_string(taken.join("data/ranking-events.jsonl")).unwrap(),
            "{}\n"
        );
    }

    /// The AppData side has no set of hashed folders to tell apart, so its state folders are the
    /// portable ones read through the same door: `%LOCALAPPDATA%\Lertaro` and `%ProgramData%\Lertaro`
    /// hold a log tree and their data files beside it.
    #[test]
    fn an_installed_state_folder_gives_up_its_logs_and_data() {
        let directory = fixture("installed-state", false);
        let log = log_in(&directory);
        let work = directory.join("bundle");
        let source = directory.join("Lertaro");
        fs::create_dir_all(source.join("logs")).unwrap();
        fs::write(source.join("logs/app.log"), "installed").unwrap();
        for name in [
            "user-settings.json",
            "user-settings.json.bak.1",
            "plugin-settings.json",
            "ranking-events.jsonl",
            "search-history.json",
            "localsend.pfx",
        ] {
            fs::write(source.join(name), name).unwrap();
        }

        collect_state(&work.join("state/user"), &source, &log);
        let state = work.join("state/user");
        assert_eq!(
            fs::read_to_string(state.join("logs/app.log")).unwrap(),
            "installed"
        );
        let mut taken = fs::read_dir(state.join("data"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        taken.sort();
        assert_eq!(
            taken,
            [
                "plugin-settings.json",
                "ranking-events.jsonl",
                "user-settings.json",
                "user-settings.json.bak.1"
            ],
            "the search history or the private key rode along"
        );
    }

    /// A folder whose live file is gone still says when the app last wrote, read off its newest
    /// backup; a folder that never held settings is named by its hash alone.
    #[test]
    fn a_folder_of_only_backups_is_still_dated() {
        let portable = fixture("rotations", true);
        let log = log_in(&portable);
        let account = portable.join("Data/Users").join("cccc");
        fs::create_dir_all(&account).unwrap();
        fs::write(account.join("user-settings.json.bak.2"), "{}").unwrap();
        assert_eq!(
            settings_stamp(&account, &log)
                .unwrap()
                .chars()
                .map(|character| if character.is_ascii_digit() {
                    '#'
                } else {
                    character
                })
                .collect::<String>(),
            "####-##-##_##-##-##-###"
        );
        assert!(settings_stamp(&portable.join("Data/Users/none"), &log).is_none());
    }

    /// The prefix of the original name is what keeps two identities that saved in the same second
    /// apart; without it the bundle loses a folder to a collision.
    #[test]
    fn a_dated_name_keeps_the_folder_it_came_from() {
        assert_eq!(label(None, "aaaa"), "aaaa");
        let stamp = "2026-10-22_00-12-00-000".to_owned();
        assert_eq!(
            label(Some(stamp.clone()), &"a".repeat(64)),
            format!("{stamp}__aaaaaaaa")
        );
        assert_ne!(
            label(Some(stamp.clone()), "aaaa1111bbbb"),
            label(Some(stamp), "aaaa2222bbbb")
        );
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
