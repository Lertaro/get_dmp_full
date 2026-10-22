use std::{
    ffi::OsStr,
    fs,
    fs::{DirEntry, FileType},
    io,
    path::{Path, PathBuf},
};

use crate::error_log::ErrorLog;
use crate::i18n::t;
use crate::{command, logs, processes, status};

// wevtutil and these event schemas are available on Windows 10.
const APPLICATION_QUERY: &str = "*[System[Level=2 and TimeCreated[timediff(@SystemTime)>=0 and timediff(@SystemTime)<=86400000] and ((Provider[@Name='Application Error'] and EventID=1000) or (Provider[@Name='.NET Runtime'] and EventID=1026) or (Provider[@Name='Windows Error Reporting'] and EventID=1001))]]";
const SYSTEM_QUERY: &str = "*[System[Level=2 and TimeCreated[timediff(@SystemTime)>=0 and timediff(@SystemTime)<=86400000] and Provider[@Name='Service Control Manager'] and (EventID=7000 or EventID=7001 or EventID=7009 or EventID=7011 or EventID=7022 or EventID=7023 or EventID=7024 or EventID=7031 or EventID=7034)]]";

/// What a failure leaves behind while the app is still running: the state trees, then the crash
/// evidence collected around them.
pub(crate) fn collect(work_dir: &Path, log: &ErrorLog, origins: &[Option<PathBuf>]) {
    collect_state(work_dir, log, origins);
    collect_crash_evidence(work_dir, log);
}

/// The logs and data files of every running copy, which is the only part of this a ranking report
/// asks for. Split out because the event log, the error reports and the dumps on disk below answer a
/// different question than "why is this row third".
pub(crate) fn collect_state(work_dir: &Path, log: &ErrorLog, origins: &[Option<PathBuf>]) {
    status::step(t("status.logs"), || logs::collect(work_dir, log, origins));
}

fn collect_crash_evidence(work_dir: &Path, log: &ErrorLog) {
    status::step(t("status.events"), || {
        for (channel, query) in [("Application", APPLICATION_QUERY), ("System", SYSTEM_QUERY)] {
            let _ = log.report("EVENTS", channel, collect_events(channel, query, work_dir));
        }
    });
    status::step(t("status.wer"), || {
        for (variable, label) in [("LOCALAPPDATA", "user"), ("ProgramData", "machine")] {
            let Some(base) = log.environment_path("WER", variable) else {
                continue;
            };
            for queue in ["ReportArchive", "ReportQueue"] {
                let source = base.join("Microsoft/Windows/WER").join(queue);
                let destination = work_dir.join("crash/wer").join(label).join(queue);
                let copied = copy_matching("WER", &source, &destination, log, |entry, kind| {
                    is_target_report(entry, kind, log)
                });
                let _ = log.report("WER", source.display(), copied);
            }
        }
    });
    status::step(t("status.crashdumps"), || {
        for (variable, suffix, label) in [
            ("LOCALAPPDATA", "CrashDumps", "user"),
            (
                "SystemRoot",
                "System32/config/systemprofile/AppData/Local/CrashDumps",
                "system",
            ),
            (
                "SystemRoot",
                "ServiceProfiles/LocalService/AppData/Local/CrashDumps",
                "local-service",
            ),
            (
                "SystemRoot",
                "ServiceProfiles/NetworkService/AppData/Local/CrashDumps",
                "network-service",
            ),
        ] {
            let Some(base) = log.environment_path("CRASHDUMPS", variable) else {
                continue;
            };
            let source = base.join(suffix);
            let destination = work_dir.join("crash/dumps").join(label);
            let copied = copy_matching("CRASHDUMPS", &source, &destination, log, |entry, kind| {
                processes::is_dump_name(&entry.file_name()) && kind.is_file()
            });
            let _ = log.report("CRASHDUMPS", source.display(), copied);
        }
    });
}

fn collect_events(channel: &str, query: &str, work_dir: &Path) -> io::Result<()> {
    let filter = format!("/q:{query}");
    let events = filter_events(&command::text(
        "wevtutil.exe",
        &[
            "qe",
            channel,
            "/rd:true",
            "/f:RenderedXml",
            "/e:Events",
            "/uni:true",
            &filter,
        ],
    )?);
    if !events.is_empty() {
        let directory = work_dir.join("crash/events");
        fs::create_dir_all(&directory)?;
        fs::write(
            directory.join(format!("{channel}.xml")),
            format!(
                "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Events>\n{events}</Events>\n"
            ),
        )?;
    }
    Ok(())
}

fn filter_events(xml: &str) -> String {
    let mut selected = String::new();
    // wevtutil emits complete Event elements; text inside them is XML-escaped.
    for part in xml.split_inclusive("</Event>") {
        let Some(start) = part.find("<Event ").or_else(|| part.find("<Event>")) else {
            continue;
        };
        let event = &part[start..];
        let Some((system, payload)) = event.split_once("</System>") else {
            continue;
        };
        // Match event data, not unrelated computer/account names in System metadata.
        let payload = payload.split("<RenderingInfo").next().unwrap_or_default();
        if event.ends_with("</Event>")
            && system.contains("<Level>2</Level>")
            && mentions_lertaro(payload)
        {
            selected.push_str(event);
            selected.push('\n');
        }
    }
    selected
}

fn mentions_lertaro(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.match_indices("lertaro").any(|(start, _)| {
        let word_character = |c: char| c.is_alphanumeric() || c == '_';
        !lower[..start]
            .chars()
            .next_back()
            .is_some_and(word_character)
            && !lower[start + "lertaro".len()..]
                .chars()
                .next()
                .is_some_and(word_character)
    })
}

fn report_is_target(report: &str) -> bool {
    report
        .lines()
        .filter_map(|line| line.split_once('='))
        .any(|(key, value)| {
            let key = key.trim();
            if !key.eq_ignore_ascii_case("AppName") && !key.eq_ignore_ascii_case("AppPath") {
                return false;
            }
            Path::new(value.trim().trim_matches('"'))
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(processes::is_target)
        })
}

/// Copies every entry of `source` the predicate accepts. Both crash-artifact walks differ only
/// in what they accept, so the walk, the per-entry logging and the destination naming live here
/// once instead of twice.
fn copy_matching(
    module: &str,
    source: &Path,
    destination: &Path,
    log: &ErrorLog,
    accept: impl Fn(&DirEntry, FileType) -> bool,
) -> io::Result<()> {
    if !source.try_exists()? {
        return Ok(());
    }
    for entry in fs::read_dir(source)? {
        let Some(entry) = log.report(module, source.display(), entry) else {
            continue;
        };
        let path = entry.path();
        let Some(kind) = log.report(module, path.display(), entry.file_type()) else {
            continue;
        };
        if !accept(&entry, kind) {
            continue;
        }
        let _ = log.report(
            module,
            path.display(),
            logs::copy_path(&path, &destination.join(entry.file_name()), log),
        );
    }
    Ok(())
}

/// A WER folder name is only a prefilter; the report inside must name a target executable.
fn is_target_report(entry: &DirEntry, kind: FileType, log: &ErrorLog) -> bool {
    let named = entry
        .file_name()
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains("lertaro");
    if !named || !kind.is_dir() {
        return false;
    }
    let path = entry.path().join("Report.wer");
    match log.report("WER", path.display(), fs::read(&path)) {
        Some(bytes) => report_is_target(&command::decode(&bytes, 0)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_events_that_name_a_target_process_are_selected() {
        let xml = concat!(
            "<Event xmlns='e'><System><Provider Name='Application Error'/>",
            "<Level>2</Level></System><EventData><Data>lertaro.app.exe</Data></EventData></Event>",
            "<Event xmlns='e'><System><Provider Name='.NET Runtime'/><Level>2</Level></System>",
            "<EventData><Data>other.app.exe</Data></EventData></Event>",
        );
        let selected = filter_events(xml);
        assert!(selected.contains("lertaro.app.exe"));
        assert!(!selected.contains("other.app.exe"));
    }

    #[test]
    fn a_lertaro_substring_in_another_word_is_not_a_match() {
        let xml = "<Event><System><Level>2</Level></System><EventData><Data>notlertaro</Data></EventData></Event>";
        assert!(!filter_events(xml).contains("notlertaro"));
    }

    #[test]
    fn report_identifies_target_by_app_name_or_path() {
        assert!(report_is_target("AppName=lertaro.app.exe\r\nAppPath=C:\\x"));
        assert!(report_is_target(
            "AppPath=\"D:\\Lertaro\\lertaro.service.exe\""
        ));
        assert!(!report_is_target(
            "AppName=notepad.exe\r\nAppPath=C:\\Windows"
        ));
    }
}
