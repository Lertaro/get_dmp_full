use std::{
    ffi::OsStr,
    fs,
    io::{self},
    path::PathBuf,
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

    // Machine, permission and runtime details belong in every bundle, dump or no dump.
    system::collect(&output_dir, &log);

    // An unsuccessful query is not evidence that all target processes have stopped.
    let processes = status::run("正在检查 Lertaro 进程", processes::running);
    if let Some(processes) = log.report("PROCESSES", "query", processes) {
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
    let archived = status::run("正在压缩诊断文件", || archive::create(&output_dir));
    if let Some(zip) = log.report("ZIP", "archive", archived) {
        println!("诊断包已生成：{}", zip.display());
    }
    Ok(())
}

fn pause() {
    println!("Press Enter to continue...");
    let _ = io::stdin().read_line(&mut String::new());
}

fn environment_path(variable: &str) -> io::Result<PathBuf> {
    let value = std::env::var_os(variable)
        .ok_or_else(|| io::Error::other("environment variable is not set"))?;
    Ok(PathBuf::from(value))
}

mod status {
    use std::{
        io::{self, Write},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    /// Runs a step while its label line refreshes its own elapsed time once a second, so
    /// a multi-minute dump or archive never looks like a hang.
    pub(super) fn run<T>(label: &str, work: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        let stage = Stage::start(label);
        let result = work();
        stage.finish();
        result
    }

    struct Stage {
        label: String,
        started: Instant,
        stop: Arc<AtomicBool>,
        ticker: Option<thread::JoinHandle<()>>,
    }

    impl Stage {
        fn start(label: &str) -> Self {
            let started = Instant::now();
            let label = label.to_owned();
            print_line(&label, started.elapsed());
            let stop = Arc::new(AtomicBool::new(false));
            let ticker = {
                let (label, stop) = (label.clone(), Arc::clone(&stop));
                thread::spawn(move || {
                    loop {
                        thread::sleep(Duration::from_secs(1));
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        print_line(&label, started.elapsed());
                    }
                })
            };
            Self {
                label,
                started,
                stop,
                ticker: Some(ticker),
            }
        }

        fn halt(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(ticker) = self.ticker.take() {
                let _ = ticker.join();
            }
        }

        fn finish(mut self) {
            self.halt();
            println!(
                "\r{}（{}）",
                self.label,
                elapsed_text(self.started.elapsed())
            );
        }
    }

    impl Drop for Stage {
        fn drop(&mut self) {
            self.halt();
        }
    }

    fn print_line(label: &str, elapsed: Duration) {
        // The trailing space covers a render shorter than the previous one.
        print!("\r{}（{}） ", label, elapsed_text(elapsed));
        let _ = io::stdout().flush();
    }

    fn elapsed_text(elapsed: Duration) -> String {
        let seconds = elapsed.as_secs();
        format!("{} min {} s", seconds / 60, seconds % 60)
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
        ffi::OsStr,
        io,
        process::{Command, Output},
    };

    pub(super) const UTF8: u32 = 65001;

    /// Runs a program with the console's output code page and returns its stdout text.
    pub(super) fn text<C: AsRef<OsStr>>(program: C, arguments: &[&str]) -> io::Result<String> {
        text_with(program, arguments, console_code_page())
    }

    pub(super) fn text_utf8<C: AsRef<OsStr>>(program: C, arguments: &[&str]) -> io::Result<String> {
        text_with(program, arguments, UTF8)
    }

    fn text_with<C: AsRef<OsStr>>(
        program: C,
        arguments: &[&str],
        code_page: u32,
    ) -> io::Result<String> {
        let captured = output(Command::new(program).args(arguments), code_page)?;
        Ok(decode(&captured.stdout, code_page))
    }

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
            &[("procdump.exe", include_bytes!("../res/procdump.exe"))],
        )
    }

    pub(super) fn fastfetch(output_dir: &Path) -> io::Result<PathBuf> {
        extract(
            output_dir,
            &[("fastfetch.exe", include_bytes!("../res/fastfetch.exe"))],
        )
    }

    pub(super) fn zip(output_dir: &Path) -> io::Result<PathBuf> {
        extract(
            output_dir,
            &[
                ("7z.exe", include_bytes!("../res/7z.exe")),
                ("7z.dll", include_bytes!("../res/7z.dll")),
            ],
        )
    }
}

mod processes {
    use super::command;
    use std::{ffi::OsStr, fmt, io};

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
        Ok(parse(&command::text(
            "tasklist.exe",
            &["/FO", "CSV", "/NH"],
        )?))
    }

    pub(super) fn is_target(name: &str) -> bool {
        TARGET_NAMES
            .iter()
            .any(|target| name.eq_ignore_ascii_case(target))
    }

    /// WER's local crash dumps are named `<executable name>.<pid>.dmp`.
    pub(super) fn is_dump_name(name: &OsStr) -> bool {
        let name = name.to_string_lossy().to_ascii_lowercase();
        name.ends_with(".dmp")
            && TARGET_NAMES
                .iter()
                .any(|target| name.starts_with(&format!("{target}.")))
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
    use super::{ErrorLog, command, processes, status, tools};
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
            let _ = log.report(
                "DUMP",
                process,
                status::run(&format!("正在转储 {process}"), || {
                    create(&executable, output_dir, process)
                }),
            );
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
    use super::{ErrorLog, environment_path};
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
            let Some(base) = log.report("LOGS", variable, environment_path(variable)) else {
                continue;
            };
            let source = base.join(relative_source);
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

mod system {
    use super::{ErrorLog, command, environment_path, status, tools};
    use std::{
        fs, io,
        path::{Path, PathBuf},
    };

    const PRODUCT: &str = "lertaro";
    const SERVICE_NAME: &str = "LertaroService";
    // An x64 process reads the 64-bit registry view, so the 32-bit and per-user
    // uninstall roots have to be enumerated as well.
    const UNINSTALL_ROOTS: [&str; 3] = [
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ];

    const ADMINISTRATORS_SID: &str = "S-1-5-32-544";
    const UAC_ROOT: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
    const UAC_VALUES: [&str; 6] = [
        "EnableLUA",
        "ConsentPromptBehaviorAdmin",
        "ConsentPromptBehaviorUser",
        "PromptOnSecureDesktop",
        "FilterAdministratorToken",
        "LocalAccountTokenFilterPolicy",
    ];

    const FRAMEWORK_KEYS: [&str; 2] = [
        r"HKLM\SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full",
        r"HKLM\SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Client",
    ];
    // Microsoft documents 4.x as cumulative: the Release value names the newest build
    // present, so every threshold below means "that version and its predecessors".
    const FRAMEWORK_RELEASES: [(&str, u32); 11] = [
        ("4.5", 378_389),
        ("4.5.1", 379_893),
        ("4.5.2", 380_522),
        ("4.6", 393_297),
        ("4.6.1", 394_294),
        ("4.6.2", 394_802),
        ("4.7", 460_793),
        ("4.7.1", 461_308),
        ("4.7.2", 461_808),
        ("4.8", 528_040),
        ("4.8.1", 533_320),
    ];

    const DOTNET_COMPONENTS: [&str; 3] = [
        "shared/Microsoft.NETCore.App",
        "shared/Microsoft.WindowsDesktop.App",
        "sdk",
    ];
    const DOTNET_MAJORS: [u32; 4] = [7, 8, 9, 10];

    pub(super) fn collect(output_dir: &Path, log: &ErrorLog) {
        let _ = log.report(
            "SYSTEM",
            "fastfetch.txt",
            status::run("正在采集系统信息", || fastfetch(output_dir)),
        );
        let _ = log.report(
            "SYSTEM",
            "icacls.txt",
            status::run("正在读取目录权限", || permissions(output_dir, log)),
        );
        let _ = log.report(
            "SYSTEM",
            "account.txt",
            status::run("正在检查账户权限", || account(output_dir, log)),
        );
        let _ = log.report(
            "SYSTEM",
            "dotnet.txt",
            status::run("正在检查 .NET 环境", || dotnet(output_dir, log)),
        );
    }

    fn write_report(output_dir: &Path, name: &str, text: &str) -> io::Result<()> {
        let directory = output_dir.join("system");
        fs::create_dir_all(&directory)?;
        // A BOM lets Windows text editors recognize UTF-8 in any system locale.
        fs::write(directory.join(name), format!("\u{feff}{text}"))
    }

    fn fastfetch(output_dir: &Path) -> io::Result<()> {
        // --logo None keeps the report plain text instead of ASCII art.
        let text = command::text_utf8(tools::fastfetch(output_dir)?, &["--logo", "None"])?;
        write_report(output_dir, "fastfetch.txt", &text)
    }

    fn permissions(output_dir: &Path, log: &ErrorLog) -> io::Result<()> {
        let mut targets: Vec<(&'static str, PathBuf)> = Vec::new();
        for (label, path) in install_dirs(log) {
            push(&mut targets, label, path);
        }
        for (label, variable) in [
            ("config-user", "LOCALAPPDATA"),
            ("config-machine", "ProgramData"),
        ] {
            let Some(base) = log.report("PERMS", variable, environment_path(variable)) else {
                continue;
            };
            push(&mut targets, label, base.join("Lertaro"));
        }

        let mut text = String::new();
        for (label, path) in &targets {
            text.push_str(&format!("[{label}] {}\r\n", path.display()));
            text.push_str(&match describe(path) {
                // A failure is a finding: keep it in the report next to its directory.
                Ok(output) => output,
                Err(error) => format!("({error})\r\n\r\n"),
            });
        }
        if targets.is_empty() {
            text.push_str("(no directory to inspect)\r\n");
        }
        write_report(output_dir, "icacls.txt", &text)
    }

    fn describe(path: &Path) -> io::Result<String> {
        if !path.try_exists()? {
            return Ok("(directory does not exist)\r\n\r\n".to_owned());
        }
        let mut text = command::text("icacls.exe", &[&path.to_string_lossy()])?;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("\r\n");
        Ok(text)
    }

    fn account(output_dir: &Path, log: &ErrorLog) -> io::Result<()> {
        let mut text = String::new();
        let mut groups = String::new();
        for argument in ["/user", "/groups", "/priv"] {
            let section = format!("whoami {argument}");
            match log.report(
                "ACCOUNT",
                &section,
                command::text("whoami.exe", &[argument]),
            ) {
                Some(output) => {
                    if argument == "/groups" {
                        groups = output.clone();
                    }
                    text.push_str(&format!("[{section}]\r\n{output}\r\n"));
                }
                None => text.push_str(&format!("[{section}]\r\n(unavailable)\r\n")),
            }
        }

        text.push_str(&format!("\r\n[UAC policy] {UAC_ROOT}\r\n"));
        if let Some(values) = log.report("ACCOUNT", UAC_ROOT, registry_values(UAC_ROOT)) {
            for name in UAC_VALUES {
                let value = registry_value(&values, name).unwrap_or("(absent)");
                text.push_str(&format!("{name} = {value}\r\n"));
            }
        }

        // Both an elevated and a UAC-filtered token carry the Administrators SID, so its
        // presence is what proves the account holds local administrator rights.
        let admin = groups.contains(ADMINISTRATORS_SID);
        let integrity = integrity(&groups);
        text.push_str(&format!(
            "\r\n[summary]\r\nlocal administrator: {}\r\nintegrity: {integrity}\r\nalready elevated: {}\r\n",
            yes_no(admin),
            yes_no(integrity.starts_with("High")),
        ));
        write_report(output_dir, "account.txt", &text)
    }

    fn yes_no(value: bool) -> &'static str {
        if value { "yes" } else { "no" }
    }

    fn integrity(groups: &str) -> String {
        let sid = groups
            .split_whitespace()
            .find(|token| token.starts_with("S-1-16-"))
            .unwrap_or("unknown");
        let level = match sid {
            "S-1-16-4096" => "Low",
            "S-1-16-8192" => "Medium",
            "S-1-16-12288" => "High",
            "S-1-16-16384" => "System",
            _ => "Unrecognized",
        };
        format!("{level} ({sid})")
    }

    fn dotnet(output_dir: &Path, log: &ErrorLog) -> io::Result<()> {
        let mut text = String::new();
        let mut release = None;
        let mut installed = false;
        text.push_str("[.NET Framework]\r\n");
        for key in FRAMEWORK_KEYS {
            match log.report("DOTNET", key, registry_values(key)) {
                Some(values) => {
                    text.push_str(&format!("{key}\r\n"));
                    for name in ["Install", "Release", "Version"] {
                        if let Some(value) = registry_value(&values, name) {
                            text.push_str(&format!("    {name} = {value}\r\n"));
                        }
                    }
                    installed = true;
                    if release.is_none() {
                        release = registry_value(&values, "Release").and_then(parse_release);
                    }
                }
                None => text.push_str(&format!("{key} = (absent)\r\n")),
            }
        }
        text.push_str("\r\n[framework checklist]\r\n");
        text.push_str(&format!("4: {}\r\n", yes_no(installed)));
        for (label, minimum) in [("4.5", 378_389), ("4.8", 528_040)] {
            text.push_str(&format!(
                "{label}: {}\r\n",
                match release {
                    Some(value) => yes_no(value >= minimum),
                    None => "unknown",
                }
            ));
        }
        if let Some(value) = release {
            text.push_str(&format!("at least: {}\r\n", framework_name(value)));
        }

        text.push_str("\r\n[.NET runtime and SDK]\r\n");
        let mut versions: Vec<(&'static str, &'static str, String)> = Vec::new();
        for (label, variable) in [("x64", "ProgramFiles"), ("x86", "ProgramFiles(x86)")] {
            let Some(base) = log.report("DOTNET", variable, environment_path(variable)) else {
                continue;
            };
            let root = base.join("dotnet");
            if !root.try_exists()? {
                text.push_str(&format!("{} ({label}) = (absent)\r\n", root.display()));
                continue;
            }
            text.push_str(&format!("{} ({label})\r\n", root.display()));
            for component in DOTNET_COMPONENTS {
                let found = installed_versions(&root.join(component))?;
                text.push_str(&format!(
                    "    {component}: {}\r\n",
                    if found.is_empty() {
                        String::from("(none)")
                    } else {
                        found.join(", ")
                    }
                ));
                versions.extend(found.into_iter().map(|version| (label, component, version)));
            }
        }
        text.push_str("\r\n[dotnet checklist]\r\n");
        for major in DOTNET_MAJORS {
            let found: Vec<String> = versions
                .iter()
                .filter(|(_, _, version)| version_major(version) == Some(major))
                .map(|(label, component, version)| {
                    let leaf = component.rsplit('/').next().unwrap_or(component);
                    format!("{leaf} {version} ({label})")
                })
                .collect();
            text.push_str(&format!(
                "{major}: {}\r\n",
                if found.is_empty() {
                    String::from("not installed")
                } else {
                    found.join("; ")
                }
            ));
        }

        write_report(output_dir, "dotnet.txt", &text)
    }

    fn installed_versions(directory: &Path) -> io::Result<Vec<String>> {
        if !directory.try_exists()? {
            return Ok(Vec::new());
        }
        let mut versions: Vec<String> = fs::read_dir(directory)?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        versions.sort_by_key(|version| version_key(version));
        Ok(versions)
    }

    fn version_key(version: &str) -> (u32, u32, u32) {
        let mut parts = version
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or_default());
        (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
        )
    }

    fn version_major(version: &str) -> Option<u32> {
        version.split('.').next()?.parse().ok()
    }

    fn parse_release(value: &str) -> Option<u32> {
        u32::from_str_radix(value.trim().strip_prefix("0x")?, 16).ok()
    }

    fn framework_name(release: u32) -> &'static str {
        let mut name = "4.0";
        for (label, minimum) in FRAMEWORK_RELEASES {
            if release >= minimum {
                name = label;
            }
        }
        name
    }

    fn install_dirs(log: &ErrorLog) -> Vec<(&'static str, PathBuf)> {
        let mut dirs: Vec<(&'static str, PathBuf)> = Vec::new();
        for root in UNINSTALL_ROOTS {
            let Some(text) = log.report("INSTALL", root, registry_query(root)) else {
                continue;
            };
            parse_uninstall(&text, &mut dirs);
        }
        // The service image path is where lertaro.service.exe really runs from, which
        // outranks the installer's record after a manual move or a development build.
        if let Some(path) = log.report("INSTALL", SERVICE_NAME, service_dir()).flatten() {
            push(&mut dirs, "install-service", path);
        }
        dirs
    }

    fn parse_uninstall(text: &str, dirs: &mut Vec<(&'static str, PathBuf)>) {
        let mut values: Vec<(&str, &str)> = Vec::new();
        for line in text.lines() {
            if line.starts_with("HKEY_") {
                if let Some(path) = install_location(&values) {
                    push(dirs, "install-registry", path);
                }
                values.clear();
            } else if let Some(value) = parse_value(line) {
                values.push(value);
            }
        }
        if let Some(path) = install_location(&values) {
            push(dirs, "install-registry", path);
        }
    }

    fn registry_query(root: &str) -> io::Result<String> {
        command::text("reg.exe", &["query", root, "/s"])
    }

    fn registry_values(key: &str) -> io::Result<String> {
        command::text("reg.exe", &["query", key])
    }

    fn registry_value<'a>(text: &'a str, name: &str) -> Option<&'a str> {
        text.lines()
            .filter_map(parse_value)
            .find(|(found, _)| found.eq_ignore_ascii_case(name))
            .map(|(_, data)| data)
    }

    fn parse_value(line: &str) -> Option<(&str, &str)> {
        // reg writes every value as `name    REG_TYPE    data`, indented by four spaces.
        let mut fields = line.trim_start().splitn(3, "    ");
        let name = fields.next()?.trim_end();
        let _ = fields.next()?;
        Some((name, fields.next()?.trim()))
    }

    fn install_location(values: &[(&str, &str)]) -> Option<PathBuf> {
        if !values.iter().any(|(name, data)| {
            name.eq_ignore_ascii_case("DisplayName") && data.to_ascii_lowercase().contains(PRODUCT)
        }) {
            return None;
        }
        ["InstallLocation", "Inno Setup: App Path"]
            .iter()
            .find_map(|key| {
                values
                    .iter()
                    .find(|(name, data)| name.eq_ignore_ascii_case(key) && !data.is_empty())
                    .map(|(_, data)| PathBuf::from(data))
            })
    }

    fn service_dir() -> io::Result<Option<PathBuf>> {
        let text = command::text("sc.exe", &["qc", SERVICE_NAME])?;
        let line = text
            .lines()
            .find(|line| line.trim_start().starts_with("BINARY_PATH_NAME"))
            .ok_or_else(|| io::Error::other("BINARY_PATH_NAME is missing"))?;
        let value = line
            .split_once(':')
            .map(|(_, value)| value.trim())
            .ok_or_else(|| io::Error::other("unexpected service configuration format"))?;
        let executable = value
            .strip_prefix('"')
            .and_then(|quoted| quoted.split('"').next())
            .unwrap_or(value)
            .split_whitespace()
            .next()
            .unwrap_or_default();
        Ok(Path::new(executable)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf))
    }

    // Registry values, service image paths and environment variables describe the same
    // directory differently, so compare them as trimmed, case-folded text.
    fn push(targets: &mut Vec<(&'static str, PathBuf)>, label: &'static str, path: PathBuf) {
        let text = path
            .to_string_lossy()
            .trim_matches(['"', ' ', '/', '\\'])
            .to_owned();
        if text.is_empty()
            || targets
                .iter()
                .any(|(_, existing)| existing.to_string_lossy().eq_ignore_ascii_case(&text))
        {
            return;
        }
        targets.push((label, PathBuf::from(text)));
    }
}

mod diagnostics {
    use super::{ErrorLog, command, environment_path, logs, processes, status};
    use std::{ffi::OsStr, fs, io, path::Path};

    // wevtutil and these event schemas are available on Windows 10.
    pub(super) const APPLICATION_QUERY: &str = "*[System[Level=2 and TimeCreated[timediff(@SystemTime)>=0 and timediff(@SystemTime)<=86400000] and ((Provider[@Name='Application Error'] and EventID=1000) or (Provider[@Name='.NET Runtime'] and EventID=1026) or (Provider[@Name='Windows Error Reporting'] and EventID=1001))]]";
    pub(super) const SYSTEM_QUERY: &str = "*[System[Level=2 and TimeCreated[timediff(@SystemTime)>=0 and timediff(@SystemTime)<=86400000] and Provider[@Name='Service Control Manager'] and (EventID=7000 or EventID=7001 or EventID=7009 or EventID=7011 or EventID=7022 or EventID=7023 or EventID=7024 or EventID=7031 or EventID=7034)]]";

    pub(super) fn collect(output_dir: &Path, log: &ErrorLog) {
        let _ = status::run("正在复制应用日志", || {
            logs::collect(output_dir, log);
            Ok(())
        });
        let _ = status::run("正在查询事件日志", || {
            for (channel, query) in [("Application", APPLICATION_QUERY), ("System", SYSTEM_QUERY)] {
                let _ = log.report(
                    "EVENTS",
                    channel,
                    collect_events(channel, query, output_dir),
                );
            }
            Ok(())
        });
        let _ = status::run("正在收集崩溃报告", || {
            for (variable, label) in [("LOCALAPPDATA", "user"), ("ProgramData", "machine")] {
                let Some(base) = log.report("WER", variable, environment_path(variable)) else {
                    continue;
                };
                for queue in ["ReportArchive", "ReportQueue"] {
                    let source = base.join("Microsoft/Windows/WER").join(queue);
                    let destination = output_dir.join("crash/wer").join(label).join(queue);
                    let _ = log.report(
                        "WER",
                        source.display(),
                        copy_reports(&source, &destination, log),
                    );
                }
            }
            Ok(())
        });
        let _ = status::run("正在收集崩溃转储", || {
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
                let Some(base) = log.report("CRASHDUMPS", variable, environment_path(variable))
                else {
                    continue;
                };
                let source = base.join(suffix);
                let destination = output_dir.join("crash/dumps").join(label);
                let _ = log.report(
                    "CRASHDUMPS",
                    source.display(),
                    copy_dumps(&source, &destination, log),
                );
            }
            Ok(())
        });
    }

    fn collect_events(channel: &str, query: &str, output_dir: &Path) -> io::Result<()> {
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
                    .is_some_and(processes::is_target)
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

    pub(super) fn copy_dumps(source: &Path, destination: &Path, log: &ErrorLog) -> io::Result<()> {
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
