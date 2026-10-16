use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self},
    path::{Path, PathBuf},
};

use error_log::ErrorLog;

const UPLOAD_SITE: &str = "https://app.dashbeam.net";

fn main() {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if let Err(error) = run(&arguments) {
        eprintln!("{}", error_log::format_record("RUN", error));
    }
    if arguments.is_empty() {
        pause();
    }
}

/// A bare run collects and packages; `--zip <路径>` says where the bundle goes. Any other
/// argument only prints the version, so a mistyped option never starts a capture.
fn resolve(arguments: &[OsString]) -> io::Result<(Option<&OsStr>, Option<&Path>)> {
    let mut other = None;
    let mut zip = None;
    let mut iterator = arguments.iter();
    while let Some(argument) = iterator.next() {
        if argument == "--zip" {
            let path = iterator
                .next()
                .ok_or_else(|| io::Error::other("missing path after --zip"))?;
            zip = Some(Path::new(path));
        } else if other.is_none() {
            other = Some(argument.as_os_str());
        }
    }
    Ok((other, zip))
}

/// `--zip` may name a folder, which keeps the timestamped file name, or a file, which is used
/// exactly as given.
fn zip_destination(folder: &Path, session: &str) -> PathBuf {
    if folder
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        folder.to_path_buf()
    } else {
        folder.join(format!("{session}.zip"))
    }
}

fn run(arguments: &[OsString]) -> io::Result<()> {
    let (argument, zip_override) = resolve(arguments)?;
    // The finished ZIP is the only deliverable of the program folder. A release build collects
    // into %TEMP%; a debug build keeps the scratch session next to the working directory.
    let zip_dir = std::env::current_exe()?.with_file_name("dump_file");
    fs::create_dir_all(&zip_dir)?;
    #[allow(unused_assignments)]
    let mut work_root = PathBuf::new();
    if cfg!(debug_assertions) {
        work_root = std::env::current_dir()?.join("dump_file");
    } else {
        work_root = std::env::temp_dir().join(env!("CARGO_PKG_NAME"));
    }
    fs::create_dir_all(&work_root)?;
    // Keep previous runs untouched and exclude their contents from this run's ZIP.
    let session = format!(
        "{}-{}",
        error_log::local_timestamp()
            .replace([':', '.'], "-")
            .replace(' ', "_"),
        std::process::id(),
    );
    let work_dir = if argument.is_none() {
        work_root.join(&session)
    } else {
        zip_dir.clone()
    };
    fs::create_dir_all(&work_dir)?;
    let log = ErrorLog::new(&work_dir)?;

    if let Some(argument) = argument {
        if matches!(
            argument.to_str(),
            Some("--help" | "-h" | "help" | "extract" | "--extract")
        ) {
            let _ = log.report(
                "SOURCE",
                "main.rs",
                fs::write(work_dir.join("main.rs"), include_str!("main.rs")),
            );
        }
        println!(
            "Assistant for creating dump file.\nVersion: {}",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    }

    // Only a collecting run has a bundle to place.
    let zip_path = match zip_override {
        Some(folder) => zip_destination(folder, &session),
        None => zip_dir.join(format!("{session}.zip")),
    };
    if let Some(parent) = zip_path.parent() {
        fs::create_dir_all(parent)?;
    }

    prompt::ready();

    // The reproduction has to survive reading the console, so the processes are frozen before
    // anything is asked or collected and released as soon as the dumps have landed.
    let processes = status::run("正在检查 Lertaro 进程", processes::running);
    let mut frozen = log.report(
        "FREEZE",
        "suspend",
        status::run("正在冻结 Lertaro 进程", || {
            freeze::suspend(processes.as_deref().unwrap_or_default(), &log)
        }),
    );
    if let Some(frozen) = &frozen {
        frozen.announce();
    }
    let memory = matches!(&processes, Ok(list) if !list.is_empty())
        && prompt::confirm("本次问题是否与内存有关（越用越占内存、内存泄漏、内存暴涨）？");

    // Machine, permission and runtime details belong in every bundle, dump or no dump.
    system::collect(&work_dir, &log);
    // Logs and previously written crash artifacts describe the failure just as well while
    // the app is still running, so they are collected before the dump decision.
    diagnostics::collect(&work_dir, &log);

    // An unsuccessful query is not evidence that all target processes have stopped.
    let mut uncompressed = false;
    if let Some(processes) = log.report("PROCESSES", "query", processes)
        && !processes.is_empty()
    {
        uncompressed = log
            .report(
                "DUMP",
                "collect",
                dump::collect(&work_dir, &processes, &log, memory),
            )
            .unwrap_or_default();
    }
    // Frozen processes keep their locks, so Lertaro runs again while 7-Zip works.
    if let Some(frozen) = &mut frozen
        && frozen.has_processes()
    {
        let _ = log.report(
            "FREEZE",
            "resume",
            status::run("正在恢复 Lertaro 进程", || frozen.resume()),
        );
    }
    // An uncompressed full dump is raw memory, so archiving earns its keep. Mini dumps and
    // ProDump's own compressed dumps are already small, where deflating only costs time.
    let level = if uncompressed { "-mx=1" } else { "-mx=0" };
    let archived = status::run("正在压缩诊断文件", || {
        archive::create(&work_dir, &zip_path, level)
    });
    if let Some(zip) = log.report("ZIP", "archive", archived) {
        println!("抓取与打包已完成。");
        println!("诊断包已生成：{}", zip.display());
        let _ = log.report("FOLDER", "explorer", open_folder(&zip));
        offer_transfer(&zip);
    }
    Ok(())
}

/// Opens the folder holding the ZIP so the user never has to reconstruct the path.
fn open_folder(zip: &Path) -> io::Result<()> {
    let folder = zip
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("the ZIP path has no parent folder"))?;
    // Explorer reports failure for a folder it opened successfully, so its exit code is ignored.
    std::process::Command::new("explorer.exe")
        .arg(folder)
        .spawn()
        .map(|_| ())
}

fn offer_transfer(zip: &Path) {
    println!();
    if prompt::confirm("是否需要现在打开上传站点，把这个文件交给开发者？") {
        // rundll32 hands the URL to the default browser without a shell, so it needs no quoting.
        match command::text(
            "rundll32.exe",
            &["url.dll,FileProtocolHandler", UPLOAD_SITE],
        ) {
            Ok(_) => println!(
                "已打开 {UPLOAD_SITE}：把诊断包拖进页面上传，再把页面给出的一次性传输票据发给开发者。\n开发者接收完成之前请不要关闭这个浏览器窗口。该方式不限速、无需登录。\n诊断包位置：{}",
                zip.display()
            ),
            Err(error) => println!("打开浏览器失败（{error}），请手动访问 {UPLOAD_SITE}"),
        }
    }
    println!("请不要使用百度网盘等限速网盘传输诊断包：下载会被限速，开发者往往拿不到完整文件。");
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

mod prompt {
    use std::ffi::c_void;
    use std::io::{self, Write};

    const STD_INPUT_HANDLE: u32 = (-10i32) as u32;
    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(index: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
        fn FlushConsoleInputBuffer(handle: *mut c_void) -> i32;
        fn ReadConsoleW(
            handle: *mut c_void,
            buffer: *mut u16,
            count: u32,
            read: *mut u32,
            pending: *mut c_void,
        ) -> i32;
    }

    /// Waits until the reproduction is on screen, because a dump of a state the user has
    /// already navigated away from cannot be taken again.
    pub(super) fn ready() {
        println!(
            "你是否已准备完毕，当前复现了bug？\n\
             如果是搜索相关的bug，请将复现问题的Lertaro窗口置顶（鼠标中键点击搜索框的Lertaro图标），然后按回车键进入下一步"
        );
        wait_key();
    }

    /// Takes one keypress without waiting for Enter or echoing it. A redirected console has no
    /// console mode to relax, so it falls back to reading a line.
    fn wait_key() {
        let stdin = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let mut mode = 0;
        if stdin.is_null()
            || stdin == usize::MAX as *mut c_void
            || unsafe { GetConsoleMode(stdin, &mut mode) } == 0
        {
            let _ = io::stdin().read_line(&mut String::new());
            return;
        }
        // Keys typed while the instructions were printed must not skip the wait.
        unsafe { FlushConsoleInputBuffer(stdin) };
        if unsafe { SetConsoleMode(stdin, mode & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT)) } == 0 {
            let _ = io::stdin().read_line(&mut String::new());
            return;
        }
        let mut buffer = [0u16; 8];
        let mut read = 0;
        // SAFETY: The buffer outlives the call and its length is passed with it; the last
        // pointer is the documented optional OVERLAPPED parameter.
        let arrived = unsafe {
            ReadConsoleW(
                stdin,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        unsafe { SetConsoleMode(stdin, mode) };
        if arrived == 0 {
            let _ = io::stdin().read_line(&mut String::new());
        }
    }

    /// Asks a yes/no question on the console. An unattended run gives no answer at all, so
    /// the default keeps it moving instead of blocking forever.
    pub(super) fn confirm(question: &str) -> bool {
        print!("{question}（是/否，默认否）");
        let _ = io::stdout().flush();
        let mut answer = String::new();
        let _ = io::stdin().read_line(&mut answer);
        println!();
        matches!(
            answer.trim().to_lowercase().as_str(),
            "是" | "对" | "y" | "yes" | "1"
        )
    }
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
        pub(super) fn new(work_dir: &Path) -> io::Result<Self> {
            let path = work_dir.join("run.log");
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
    use std::{fs, io, path::PathBuf};

    /// Embedded helpers live under %TEMP%, away from the collected files, so nothing of the
    /// tool set can end up inside the bundle and the program folder stays free of binaries.
    fn directory() -> io::Result<PathBuf> {
        let directory = std::env::temp_dir()
            .join(env!("CARGO_PKG_NAME"))
            .join(".dump_tools");
        fs::create_dir_all(&directory)?;
        Ok(directory)
    }

    fn extract(files: &[(&str, &[u8])]) -> io::Result<PathBuf> {
        let directory = directory()?;
        for (name, bytes) in files {
            fs::write(directory.join(name), bytes)?;
        }
        Ok(directory.join(files[0].0))
    }

    pub(super) fn procdump() -> io::Result<PathBuf> {
        extract(&[("procdump.exe", include_bytes!("../res/procdump.exe"))])
    }

    pub(super) fn fastfetch() -> io::Result<PathBuf> {
        extract(&[("fastfetch.exe", include_bytes!("../res/fastfetch.exe"))])
    }

    pub(super) fn zip() -> io::Result<PathBuf> {
        extract(&[
            ("7z.exe", include_bytes!("../res/7z.exe")),
            ("7z.dll", include_bytes!("../res/7z.dll")),
        ])
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

mod freeze {
    use super::{ErrorLog, processes};
    use std::ffi::c_void;
    use std::io;

    /// The single right `NtSuspendProcess` needs on the process handle.
    const PROCESS_SUSPEND_RESUME: u32 = 0x0800;
    const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
    const TOKEN_QUERY: u32 = 0x0001;
    const SE_PRIVILEGE_ENABLED: u32 = 0x0000_0002;

    /// An ntdll routine that takes one process handle and answers with an NTSTATUS.
    type NtProcess = unsafe extern "system" fn(*mut c_void) -> i32;

    #[repr(C)]
    struct Luid {
        low: u32,
        high: i32,
    }

    #[repr(C)]
    struct LuidAndAttributes {
        luid: Luid,
        attributes: u32,
    }

    #[repr(C)]
    struct TokenPrivileges {
        count: u32,
        privileges: [LuidAndAttributes; 1],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn GetLastError() -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut c_void;
        fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
        fn AdjustTokenPrivileges(
            token: *mut c_void,
            disable_all: i32,
            requested: *const TokenPrivileges,
            length: u32,
            previous: *mut TokenPrivileges,
            returned: *mut u32,
        ) -> i32;
        fn GetModuleHandleW(module: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const i8) -> *mut c_void;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut Luid) -> i32;
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// `NtSuspendProcess` and `NtResumeProcess` are exported by ntdll but not by any import
    /// library the toolchain ships reliably, so they are looked up at run time. ntdll is mapped
    /// into every process, so this needs no loading and leaves no module to free.
    fn ntdll(entry: &str) -> io::Result<NtProcess> {
        let module = unsafe { GetModuleHandleW(wide("ntdll.dll").as_ptr()) };
        if module.is_null() {
            return Err(io::Error::last_os_error());
        }
        let name: Vec<i8> = entry.bytes().map(|byte| byte as i8).chain([0]).collect();
        let address = unsafe { GetProcAddress(module, name.as_ptr()) };
        if address.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetProcAddress returned a non-null address for an ntdll export whose
        // signature is (HANDLE) -> NTSTATUS, and a function pointer is the same size as the
        // returned address.
        Ok(unsafe { std::mem::transmute::<*mut c_void, NtProcess>(address) })
    }

    /// One open handle per frozen process: `NtResumeProcess` is called through the very handle
    /// that froze it, so a process another caller suspended cannot be released by mistake.
    pub(super) struct Frozen {
        handles: Vec<*mut c_void>,
        targets: usize,
        resume_process: NtProcess,
        refused: Option<String>,
    }

    /// Freezes every target process so the reproduced state cannot drift while the console is
    /// read and the dumps are written.
    pub(super) fn suspend(list: &[processes::Process], log: &ErrorLog) -> io::Result<Frozen> {
        let suspend_process = ntdll("NtSuspendProcess")?;
        let resume_process = ntdll("NtResumeProcess")?;
        let targets = list.len();
        if list.is_empty() {
            return Ok(Frozen {
                handles: Vec::new(),
                targets,
                resume_process,
                refused: None,
            });
        }
        enable_debug_privilege(log);
        let mut handles = Vec::new();
        let mut refused = None;
        for process in list {
            // One call freezes the whole process, including threads created after this point.
            let handle = unsafe { OpenProcess(PROCESS_SUSPEND_RESUME, 0, process.pid) };
            let failure = if handle.is_null() {
                Some(format!(
                    "OpenProcess failed: {}",
                    io::Error::last_os_error()
                ))
            } else {
                let status = unsafe { suspend_process(handle) };
                if status == 0 {
                    handles.push(handle);
                    None
                } else {
                    unsafe { CloseHandle(handle) };
                    Some(format!("NtSuspendProcess failed: {status:#010x}"))
                }
            };
            if let Some(failure) = failure {
                log.error("FREEZE", process, &failure);
                refused = refused.or(Some(failure));
            }
        }
        Ok(Frozen {
            handles,
            targets,
            resume_process,
            refused,
        })
    }

    impl Frozen {
        pub(super) fn has_processes(&self) -> bool {
            !self.handles.is_empty()
        }

        /// Tells the user what stopped, because a frozen window looks exactly like a hung one.
        /// The reason is the one the system actually gave, never a guess.
        pub(super) fn announce(&self) {
            if self.targets == 0 {
                println!("没有正在运行的 Lertaro 进程，跳过冻结。");
            } else if self.handles.is_empty() {
                println!(
                    "未能冻结任何进程（{}），转储可能抓不到复现瞬间的状态。",
                    self.refused.as_deref().unwrap_or("原因见 run.log")
                );
            } else {
                println!("已冻结 {} 个进程。", self.handles.len());
                if self.handles.len() < self.targets {
                    println!(
                        "另有 {} 个进程没能冻结（{}）。",
                        self.targets - self.handles.len(),
                        self.refused.as_deref().unwrap_or("原因见 run.log")
                    );
                }
            }
        }

        /// Releases every process this run froze and reports the first refusal.
        pub(super) fn resume(&mut self) -> io::Result<()> {
            let mut refused = None;
            for handle in std::mem::take(&mut self.handles) {
                let status = unsafe { (self.resume_process)(handle) };
                if status != 0 && refused.is_none() {
                    refused = Some(io::Error::other(format!(
                        "NtResumeProcess failed: {status:#010x}"
                    )));
                }
                unsafe { CloseHandle(handle) };
            }
            match refused {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }

    impl Drop for Frozen {
        fn drop(&mut self) {
            // A failed dump or archive must not leave Lertaro frozen.
            let _ = self.resume();
        }
    }

    /// Opens the door to the session 0 service process, whose handle an elevated token may only
    /// open with the debug privilege — held by default, but disabled.
    fn enable_debug_privilege(log: &ErrorLog) {
        let name = wide("SeDebugPrivilege");
        let mut luid = Luid { low: 0, high: 0 };
        if unsafe { LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) } == 0 {
            log.error(
                "FREEZE",
                "LookupPrivilegeValueW",
                io::Error::last_os_error(),
            );
            return;
        }
        let mut token = std::ptr::null_mut();
        if unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            )
        } == 0
        {
            log.error("FREEZE", "OpenProcessToken", io::Error::last_os_error());
            return;
        }
        let requested = TokenPrivileges {
            count: 1,
            privileges: [LuidAndAttributes {
                luid,
                attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let assigned = unsafe {
            AdjustTokenPrivileges(
                token,
                0,
                &requested,
                std::mem::size_of::<TokenPrivileges>() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        // A token that does not hold the privilege reports ERROR_NOT_ALL_ASSIGNED here rather
        // than failing the call, so read it before any other API can overwrite it.
        let code = unsafe { GetLastError() };
        unsafe { CloseHandle(token) };
        if assigned == 0 {
            log.error(
                "FREEZE",
                "AdjustTokenPrivileges",
                io::Error::from_raw_os_error(code as i32),
            );
        } else if code != 0 {
            log.error(
                "FREEZE",
                "SeDebugPrivilege",
                io::Error::from_raw_os_error(code as i32),
            );
        }
    }
}

mod dump {
    use super::{ErrorLog, command, processes, status, tools};
    use std::{
        io,
        path::Path,
        process::{Command, Output},
    };

    const COMPRESSED: &str = "-mac";
    const UNCOMPRESSED: &str = "-ma";
    const MINI: &str = "-mm";

    /// Collects every target process, answering whether an uncompressed full dump landed.
    pub(super) fn collect(
        work_dir: &Path,
        processes: &[processes::Process],
        log: &ErrorLog,
        memory: bool,
    ) -> io::Result<bool> {
        let executable = tools::procdump()?;
        let mut uncompressed = false;
        for process in processes {
            let landed = status::run(&format!("正在转储 {process}"), || {
                create(&executable, work_dir, process, log, memory)
            });
            if log.report("DUMP", process, landed).unwrap_or_default() {
                uncompressed = true;
            }
        }
        Ok(uncompressed)
    }

    fn create(
        executable: &Path,
        work_dir: &Path,
        process: &processes::Process,
        log: &ErrorLog,
        memory: bool,
    ) -> io::Result<bool> {
        // Stacks, metadata and what the stacks reference are enough for crashes, hangs and
        // startup failures, and that is roughly 3% of a full dump.
        if !memory {
            return capture(executable, work_dir, process, MINI).map(|()| false);
        }
        // Some Windows builds refuse ProDump's compressed full dump outright. Falling back
        // costs a larger file, which is still better than shipping a bundle without a dump.
        match capture(executable, work_dir, process, COMPRESSED) {
            Err(error) if error.to_string().contains("Error writing dump file") => {
                // Record the refusal so the reader learns this machine cannot compress.
                log.error("DUMP", process, format!("compressed dump refused: {error}"));
                capture(executable, work_dir, process, UNCOMPRESSED).map(|()| true)
            }
            result => result.map(|()| false),
        }
    }

    fn capture(
        executable: &Path,
        work_dir: &Path,
        process: &processes::Process,
        dump_type: &str,
    ) -> io::Result<()> {
        let output = Command::new(executable)
            .args([
                "-accepteula",
                dump_type,
                "-o",
                &process.pid.to_string(),
                "PROCESSNAME_PID.dmp",
            ])
            .current_dir(work_dir)
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

    pub(super) fn collect(work_dir: &Path, log: &ErrorLog) {
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

    pub(super) fn collect(work_dir: &Path, log: &ErrorLog) {
        let _ = log.report(
            "SYSTEM",
            "fastfetch.txt",
            status::run("正在采集系统信息", || fastfetch(work_dir)),
        );
        let _ = log.report(
            "SYSTEM",
            "icacls.txt",
            status::run("正在读取目录权限", || permissions(work_dir, log)),
        );
        let _ = log.report(
            "SYSTEM",
            "account.txt",
            status::run("正在检查账户权限", || account(work_dir, log)),
        );
        let _ = log.report(
            "SYSTEM",
            "dotnet.txt",
            status::run("正在检查 .NET 环境", || dotnet(work_dir, log)),
        );
    }

    fn write_report(work_dir: &Path, name: &str, text: &str) -> io::Result<()> {
        let directory = work_dir.join("system");
        fs::create_dir_all(&directory)?;
        // A BOM lets Windows text editors recognize UTF-8 in any system locale.
        fs::write(directory.join(name), format!("\u{feff}{text}"))
    }

    fn fastfetch(work_dir: &Path) -> io::Result<()> {
        // --logo None keeps the report plain text instead of ASCII art.
        let text = command::text_utf8(tools::fastfetch()?, &["--logo", "None"])?;
        write_report(work_dir, "fastfetch.txt", &text)
    }

    fn permissions(work_dir: &Path, log: &ErrorLog) -> io::Result<()> {
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
        write_report(work_dir, "icacls.txt", &text)
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

    fn account(work_dir: &Path, log: &ErrorLog) -> io::Result<()> {
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
        write_report(work_dir, "account.txt", &text)
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

    fn dotnet(work_dir: &Path, log: &ErrorLog) -> io::Result<()> {
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

        write_report(work_dir, "dotnet.txt", &text)
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

    pub(super) fn collect(work_dir: &Path, log: &ErrorLog) {
        let _ = status::run("正在复制应用日志", || {
            logs::collect(work_dir, log);
            Ok(())
        });
        let _ = status::run("正在查询事件日志", || {
            for (channel, query) in [("Application", APPLICATION_QUERY), ("System", SYSTEM_QUERY)] {
                let _ = log.report("EVENTS", channel, collect_events(channel, query, work_dir));
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

    pub(super) fn create(work_dir: &Path, destination: &Path, level: &str) -> io::Result<PathBuf> {
        let executable = tools::zip()?;
        // Written beside the working directory: 7-Zip must not archive its own output.
        let pending = work_dir.with_extension("pending.zip");
        if pending.try_exists()? {
            fs::remove_file(&pending)?;
        }

        command::output(
            Command::new(executable)
                .args([
                    "a",
                    "-tzip",
                    level,
                    "-y",
                    "-sccUTF-8",
                    "-bb0",
                    "-bd",
                    "-sse",
                    "-ssw",
                ])
                .arg(&pending)
                .arg(".")
                .current_dir(work_dir),
            command::UTF8,
        )?;
        // Replace only after 7-Zip completed without warnings or errors.
        if fs::rename(&pending, destination).is_err() {
            // %TEMP% and the program folder can sit on different volumes, where a rename has no
            // choice but to copy.
            fs::copy(&pending, destination)?;
            fs::remove_file(&pending)?;
        }
        Ok(destination.to_path_buf())
    }
}
