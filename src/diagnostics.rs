use std::{ffi::OsStr, fs, io, path::Path};

use crate::environment_path;
use crate::error_log::ErrorLog;
use crate::i18n::t;
use crate::{command, logs, processes, status};

// wevtutil and these event schemas are available on Windows 10.
const APPLICATION_QUERY: &str = "*[System[Level=2 and TimeCreated[timediff(@SystemTime)>=0 and timediff(@SystemTime)<=86400000] and ((Provider[@Name='Application Error'] and EventID=1000) or (Provider[@Name='.NET Runtime'] and EventID=1026) or (Provider[@Name='Windows Error Reporting'] and EventID=1001))]]";
const SYSTEM_QUERY: &str = "*[System[Level=2 and TimeCreated[timediff(@SystemTime)>=0 and timediff(@SystemTime)<=86400000] and Provider[@Name='Service Control Manager'] and (EventID=7000 or EventID=7001 or EventID=7009 or EventID=7011 or EventID=7022 or EventID=7023 or EventID=7024 or EventID=7031 or EventID=7034)]]";

pub(crate) fn collect(work_dir: &Path, log: &ErrorLog) {
    let _ = status::run(t("status.logs"), || {
        logs::collect(work_dir, log);
        Ok(())
    });
    let _ = status::run(t("status.events"), || {
        for (channel, query) in [("Application", APPLICATION_QUERY), ("System", SYSTEM_QUERY)] {
            let _ = log.report("EVENTS", channel, collect_events(channel, query, work_dir));
        }
        Ok(())
    });
    let _ = status::run(t("status.wer"), || {
        for (variable, label) in [("LOCALAPPDATA", "user"), ("ProgramData", "machine")] {
            let Some(base) = log.report("WER", variable, environment_path(variable)) else {
                continue;
            };
            for queue in ["ReportArchive", "ReportQueue"] {
                let source = base.join("Microsoft/Windows/WER").join(queue);
                let destination = work_dir.join("crash/wer").join(label).join(queue);
                let _ = log.report(
                    "WER",
                    source.display(),
                    copy_reports(&source, &destination, log),
                );
            }
        }
        Ok(())
    });
    let _ = status::run(t("status.crashdumps"), || {
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
            let Some(base) = log.report("CRASHDUMPS", variable, environment_path(variable)) else {
                continue;
            };
            let source = base.join(suffix);
            let destination = work_dir.join("crash/dumps").join(label);
            let _ = log.report(
                "CRASHDUMPS",
                source.display(),
                copy_dumps(&source, &destination, log),
            );
        }
        Ok(())
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

fn copy_reports(source: &Path, destination: &Path, log: &ErrorLog) -> io::Result<()> {
    if !source.try_exists()? {
        return Ok(());
    }
    for entry in fs::read_dir(source)? {
        let Some(entry) = log.report("WER", source.display(), entry) else {
            continue;
        };
        // Folder names are only a prefilter; Report.wer must identify a target executable.
        if !entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("lertaro")
        {
            continue;
        }
        let path = entry.path();
        let Some(kind) = log.report("WER", path.display(), entry.file_type()) else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let report_path = path.join("Report.wer");
        let Some(bytes) = log.report("WER", report_path.display(), fs::read(&report_path)) else {
            continue;
        };
        if report_is_target(&command::decode(&bytes, 0)) {
            let _ = log.report(
                "WER",
                path.display(),
                logs::copy_path(&path, &destination.join(entry.file_name()), log),
            );
        }
    }
    Ok(())
}

fn copy_dumps(source: &Path, destination: &Path, log: &ErrorLog) -> io::Result<()> {
    if !source.try_exists()? {
        return Ok(());
    }
    for entry in fs::read_dir(source)? {
        let Some(entry) = log.report("CRASHDUMPS", source.display(), entry) else {
            continue;
        };
        if !processes::is_dump_name(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let Some(kind) = log.report("CRASHDUMPS", path.display(), entry.file_type()) else {
            continue;
        };
        if kind.is_file() {
            let _ = log.report(
                "CRASHDUMPS",
                path.display(),
                logs::copy_path(&path, &destination.join(entry.file_name()), log),
            );
        }
    }
    Ok(())
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
