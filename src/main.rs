use std::{
    ffi::OsStr,
    fs,
    io::{self, IsTerminal},
};

use error_log::ErrorLog;

fn main() {
    let argument = std::env::args_os().nth(1);
    if let Err(error) = run(argument.as_deref()) {
        eprintln!("{}", error_log::format_record("RUN", error));
    }
    if argument.is_none() {
        pause();
    }
}

fn run(argument: Option<&OsStr>) -> io::Result<()> {
    let root = std::env::current_exe()?.with_file_name("dump_file");
    fs::create_dir_all(&root)?;
    let output_dir = if argument.is_none() {
        // Keep previous runs untouched and exclude their contents from this run's ZIP.
        let session = format!(
            "{}-{}",
            error_log::local_timestamp()
                .replace([':', '.'], "-")
                .replace(' ', "_"),
            std::process::id(),
        );
        let directory = root.join(session);
        fs::create_dir(&directory)?;
        directory
    } else {
        root
    };
    let log = ErrorLog::new(&output_dir)?;

    if let Some(argument) = argument {
        if matches!(
            argument.to_str(),
            Some("--help" | "-h" | "help" | "extract" | "--extract")
        ) {
            let _ = log.report(
                "SOURCE",
                "main.rs",
                fs::write(output_dir.join("main.rs"), include_str!("main.rs")),
            );
        }
        println!(
            "Assistant for creating dump file.\nVersion: {}",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    // An unsuccessful query is not evidence that all target processes have stopped.
    if let Some(processes) = log.report("PROCESSES", "query", processes::running()) {
        if processes.is_empty() {
            diagnostics::collect(&output_dir, &log);
        } else {
            let _ = log.report(
                "DUMP",
                "collect",
                dump::collect(&output_dir, &processes, &log),
            );
        }
    }
    let _ = log.report("ZIP", "archive", archive::create(&output_dir));
    Ok(())
}

fn pause() {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        println!("Press Enter to continue...");
        let _ = stdin.read_line(&mut String::new());
    }
}

mod error_log {
    use std::{
        fmt::Display,
        fs::{self, OpenOptions},
        io::{self, Write},
        path::{Path, PathBuf},
    };

    pub(super) struct ErrorLog {
        path: PathBuf,
    }

    impl ErrorLog {
        pub(super) fn new(output_dir: &Path) -> io::Result<Self> {
            let path = output_dir.join("run.log");
            // A BOM lets Windows text editors recognize UTF-8 in any system locale.
            fs::write(&path, b"\xEF\xBB\xBF")?;
            Ok(Self { path })
        }

        pub(super) fn report<T>(
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

        pub(super) fn error(&self, module: &str, context: impl Display, error: impl Display) {
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

    pub(super) fn format_record(module: &str, context: impl Display) -> String {
        let context = context
            .to_string()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        format!("[{}][ERROR][{module}]: {context}", local_timestamp())
    }

    pub(super) fn local_timestamp() -> String {
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
}

mod command {
    use std::{
        io,
        process::{Command, Output},
    };

    pub(super) const UTF8: u32 = 65001;

    pub(super) fn output(command: &mut Command, code_page: u32) -> io::Result<Output> {
        check(command.output()?, code_page)
    }

    pub(super) fn check(output: Output, code_page: u32) -> io::Result<Output> {
        if output.status.success() {
            return Ok(output);
        }

        let stderr = decode(&output.stderr, code_page);
        let stdout = decode(&output.stdout, code_page);
        let detail = if stderr.trim().is_empty() {
            &stdout
        } else {
            &stderr
        };
        // Prefer diagnostic lines over version banners and progress messages.
        let errors: Vec<_> = detail
            .lines()
            .map(str::trim)
            .filter(|line| {
                let lower = line.to_ascii_lowercase();
                [
                    "error",
                    "failed",
                    "cannot",
                    "denied",
                    "unable",
                    "not found",
                    "can't",
                ]
                .iter()
                .any(|word| lower.contains(word))
                    || ["错误", "失败", "拒绝", "无法", "找不到"]
                        .iter()
                        .any(|word| line.contains(word))
            })
            .collect();
        let reason = if errors.is_empty() {
            detail
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_owned()
        } else {
            errors.join("; ")
        };
        Err(io::Error::other(format!("{}: {reason}", output.status)))
    }

    pub(super) fn decode(bytes: &[u8], code_page: u32) -> String {
        if let Some(bytes) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let (bytes, big_endian) = if let Some(bytes) = bytes.strip_prefix(b"\xFF\xFE") {
            (bytes, false)
        } else if let Some(bytes) = bytes.strip_prefix(b"\xFE\xFF") {
            (bytes, true)
        } else if bytes.len() >= 4 && bytes[1] == 0 && bytes[3] == 0 {
            (bytes, false)
        } else {
            return decode_code_page(bytes, code_page);
        };
        let wide: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                if big_endian {
                    u16::from_be_bytes([pair[0], pair[1]])
                } else {
                    u16::from_le_bytes([pair[0], pair[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&wide)
    }

    #[cfg(windows)]
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleOutputCP() -> u32;
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            input: *const u8,
            input_len: i32,
            output: *mut u16,
            output_len: i32,
        ) -> i32;
    }

    pub(super) fn console_code_page() -> u32 {
        #[cfg(windows)]
        {
            // SAFETY: This Win32 query takes no pointers and has no preconditions.
            let code_page = unsafe { GetConsoleOutputCP() };
            if code_page == 0 { 1 } else { code_page } // CP_OEMCP when no console exists.
        }
        #[cfg(not(windows))]
        {
            UTF8
        }
    }

    pub(super) fn decode_code_page(bytes: &[u8], code_page: u32) -> String {
        if bytes.is_empty() {
            return String::new();
        }
        if code_page == UTF8 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        #[cfg(windows)]
        if let Ok(length) = i32::try_from(bytes.len()) {
            let mut wide = vec![0u16; bytes.len()];
            // SAFETY: Both buffers remain alive, have the stated lengths, and do not
            // overlap. Win32 bounds its writes by output_len; zero indicates failure.
            let written = unsafe {
                MultiByteToWideChar(
                    code_page,
                    0,
                    bytes.as_ptr(),
                    length,
                    wide.as_mut_ptr(),
                    length,
                )
            };
            if written > 0 {
                return String::from_utf16_lossy(&wide[..written as usize]);
            }
        }
        String::from_utf8_lossy(bytes).into_owned()
    }
}

mod tools {
    use std::{
        fs, io,
        path::{Path, PathBuf},
    };

    fn extract(output_dir: &Path, files: &[(&str, &[u8])]) -> io::Result<PathBuf> {
        // Tools are siblings of dump_file, never part of its contents.
        let directory = output_dir.with_file_name(".dump_tools");
        fs::create_dir_all(&directory)?;
        for (name, bytes) in files {
            fs::write(directory.join(name), bytes)?;
        }
        Ok(directory.join(files[0].0))
    }

    pub(super) fn procdump(output_dir: &Path) -> io::Result<PathBuf> {
        extract(
            output_dir,
            &[("procdump.exe", include_bytes!("../procdump.exe"))],
        )
    }

    pub(super) fn zip(output_dir: &Path) -> io::Result<PathBuf> {
        extract(
            output_dir,
            &[
                ("7z.exe", include_bytes!("../7z.exe")),
                ("7z.dll", include_bytes!("../7z.dll")),
            ],
        )
    }
}

mod processes {
    use super::command;
    use std::{fmt, io, process::Command};

    pub(super) const TARGET_NAMES: [&str; 2] = ["lertaro.app.exe", "lertaro.service.exe"];

    pub(super) struct Process {
        pub name: &'static str,
        pub pid: u32,
    }

    impl fmt::Display for Process {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "{} (PID {})", self.name, self.pid)
        }
    }

    pub(super) fn running() -> io::Result<Vec<Process>> {
        let code_page = command::console_code_page();
        let output = command::output(
            Command::new("tasklist").args(["/FO", "CSV", "/NH"]),
            code_page,
        )?;
        Ok(parse(&command::decode(&output.stdout, code_page)))
    }

    pub(super) fn parse(output: &str) -> Vec<Process> {
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
}

mod dump {
    use super::{ErrorLog, command, processes, tools};
    use std::{
        io,
        path::Path,
        process::{Command, Output},
    };

    pub(super) fn collect(
        output_dir: &Path,
        processes: &[processes::Process],
        log: &ErrorLog,
    ) -> io::Result<()> {
        let executable = tools::procdump(output_dir)?;
        for process in processes {
            let _ = log.report("DUMP", process, create(&executable, output_dir, process));
        }
        Ok(())
    }

    fn create(
        executable: &Path,
        output_dir: &Path,
        process: &processes::Process,
    ) -> io::Result<()> {
        let output = Command::new(executable)
            .args([
                "-accepteula",
                "-ma",
                "-o",
                &process.pid.to_string(),
                "PROCESSNAME_PID.dmp",
            ])
            .current_dir(output_dir)
            .output()?;
        check(output, command::console_code_page())
    }

    pub(super) fn check(output: Output, code_page: u32) -> io::Result<()> {
        // ProcDump 12.01 returns -2 after reaching the requested dump count.
        // Require confirmation that the dump was written, not just that monitoring ended.
        if output.status.code() == Some(-2) {
            let stdout = command::decode(&output.stdout, code_page);
            if stdout.contains("Dump 1 complete:") && stdout.contains("Dump count reached.") {
                return Ok(());
            }
        }
        command::check(output, code_page).map(|_| ())
    }
}

mod logs {
    use super::ErrorLog;
    use std::{fs, io, path::Path};

    const SOURCES: [(&str, &str, &str); 2] = [
        ("LOCALAPPDATA", "Lertaro/logs", "logs/user"),
        (
            "ProgramData",
            "Lertaro/logs/service.log",
            "logs/service/service.log",
        ),
    ];

    pub(super) fn collect(output_dir: &Path, log: &ErrorLog) {
        for (variable, relative_source, relative_destination) in SOURCES {
            let Some(base) = std::env::var_os(variable) else {
                log.error("LOGS", variable, "environment variable is not set");
                continue;
            };
            let source = Path::new(&base).join(relative_source);
            let destination = output_dir.join(relative_destination);
            let _ = log.report(
                "LOGS",
                source.display(),
                copy_path(&source, &destination, log),
            );
        }
    }

    pub(super) fn copy_path(source: &Path, destination: &Path, log: &ErrorLog) -> io::Result<()> {
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
}

mod diagnostics {
    use super::{ErrorLog, command, logs, processes};
    use std::{ffi::OsStr, fs, io, path::Path, process::Command};

    // wevtutil and these event schemas are available on Windows 10.
    pub(super) const APPLICATION_QUERY: &str = "*[System[Level=2 and TimeCreated[timediff(@SystemTime)>=0 and timediff(@SystemTime)<=86400000] and ((Provider[@Name='Application Error'] and EventID=1000) or (Provider[@Name='.NET Runtime'] and EventID=1026) or (Provider[@Name='Windows Error Reporting'] and EventID=1001))]]";
    pub(super) const SYSTEM_QUERY: &str = "*[System[Level=2 and TimeCreated[timediff(@SystemTime)>=0 and timediff(@SystemTime)<=86400000] and Provider[@Name='Service Control Manager'] and (EventID=7000 or EventID=7001 or EventID=7009 or EventID=7011 or EventID=7022 or EventID=7023 or EventID=7024 or EventID=7031 or EventID=7034)]]";

    pub(super) fn collect(output_dir: &Path, log: &ErrorLog) {
        logs::collect(output_dir, log);
        for (channel, query) in [("Application", APPLICATION_QUERY), ("System", SYSTEM_QUERY)] {
            let _ = log.report(
                "EVENTS",
                channel,
                collect_events(channel, query, output_dir),
            );
        }

        for (variable, label) in [("LOCALAPPDATA", "user"), ("ProgramData", "machine")] {
            if let Some(base) = std::env::var_os(variable) {
                for queue in ["ReportArchive", "ReportQueue"] {
                    let source = Path::new(&base).join("Microsoft/Windows/WER").join(queue);
                    let destination = output_dir.join("crash/wer").join(label).join(queue);
                    let _ = log.report(
                        "WER",
                        source.display(),
                        copy_reports(&source, &destination, log),
                    );
                }
            }
        }

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
            if let Some(base) = std::env::var_os(variable) {
                let source = Path::new(&base).join(suffix);
                let destination = output_dir.join("crash/dumps").join(label);
                let _ = log.report(
                    "CRASHDUMPS",
                    source.display(),
                    copy_dumps(&source, &destination, log),
                );
            }
        }
    }

    fn collect_events(channel: &str, query: &str, output_dir: &Path) -> io::Result<()> {
        let output = command::output(
            Command::new("wevtutil.exe")
                .args([
                    "qe",
                    channel,
                    "/rd:true",
                    "/f:RenderedXml",
                    "/e:Events",
                    "/uni:true",
                ])
                .arg(format!("/q:{query}")),
            command::console_code_page(),
        )?;
        let events = filter_events(&command::decode(
            &output.stdout,
            command::console_code_page(),
        ));
        if !events.is_empty() {
            let directory = output_dir.join("crash/events");
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

    pub(super) fn filter_events(xml: &str) -> String {
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

    pub(super) fn report_is_target(report: &str) -> bool {
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
                    .is_some_and(|name| {
                        processes::TARGET_NAMES
                            .iter()
                            .any(|target| target.eq_ignore_ascii_case(name))
                    })
            })
    }

    pub(super) fn copy_reports(
        source: &Path,
        destination: &Path,
        log: &ErrorLog,
    ) -> io::Result<()> {
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
            let Some(bytes) = log.report("WER", report_path.display(), fs::read(&report_path))
            else {
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

    pub(super) fn is_target_dump(name: &OsStr) -> bool {
        let name = name.to_string_lossy().to_ascii_lowercase();
        name.ends_with(".dmp")
            && processes::TARGET_NAMES
                .iter()
                .any(|target| name.starts_with(&format!("{target}.")))
    }

    pub(super) fn copy_dumps(source: &Path, destination: &Path, log: &ErrorLog) -> io::Result<()> {
        if !source.try_exists()? {
            return Ok(());
        }
        for entry in fs::read_dir(source)? {
            let Some(entry) = log.report("CRASHDUMPS", source.display(), entry) else {
                continue;
            };
            if !is_target_dump(&entry.file_name()) {
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
}

mod archive {
    use super::{command, tools};
    use std::{
        fs, io,
        path::{Path, PathBuf},
        process::Command,
    };

    pub(super) fn create(output_dir: &Path) -> io::Result<PathBuf> {
        let executable = tools::zip(output_dir)?;
        let destination = output_dir.with_extension("zip");
        let pending = output_dir.with_extension("pending.zip");
        if pending.try_exists()? {
            fs::remove_file(&pending)?;
        }

        command::output(
            Command::new(executable)
                .args([
                    "a",
                    "-tzip",
                    "-mx=0",
                    "-y",
                    "-sccUTF-8",
                    "-bb0",
                    "-bd",
                    "-sse",
                    "-ssw",
                    "-xr!procdump*.exe",
                    "-xr!7z.exe",
                    "-xr!7z.dll",
                    "-xr!.dump_tools",
                ])
                .arg(&pending)
                .arg(".")
                .current_dir(output_dir),
            command::UTF8,
        )?;
        // Replace only after 7-Zip completed without warnings or errors.
        fs::rename(&pending, &destination)?;
        Ok(destination)
    }
}
