mod archive;
mod cli;
mod command;
mod diagnostics;
mod dump;
mod error_log;
mod i18n;
mod logs;
mod processes;
mod prompt;
mod status;
mod system;
mod tools;
mod transfer;

use std::{ffi::OsString, fs, io, path::PathBuf};

use crate::error_log::ErrorLog;
use crate::i18n::{t, tr};

fn main() {
    println!("Version: {}", env!("CARGO_PKG_VERSION"));
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if let Err(error) = run(&arguments) {
        eprintln!("{}", error_log::format_record("RUN", error));
    }
    if arguments.is_empty() {
        prompt::finish();
    }
}

fn run(arguments: &[OsString]) -> io::Result<()> {
    match cli::resolve(arguments)? {
        cli::Invocation::Help => {
            cli::show_help();
            Ok(())
        }
        cli::Invocation::Collect { zip } => collect(zip),
    }
}

/// Freezes the target processes, collects everything that describes the failure, and packs it.
fn collect(zip_override: Option<PathBuf>) -> io::Result<()> {
    // Nothing is read or written until the reproduction is confirmed on screen.
    if !prompt::ready() {
        println!("{}", t("run.cancelled"));
        return Ok(());
    }

    // The finished ZIP is the only deliverable of the program folder. A release build collects
    // into %TEMP%; a debug build keeps the scratch session next to the working directory.
    let work_root = if cfg!(debug_assertions) {
        std::env::current_dir()?
    } else {
        std::env::temp_dir().join(env!("CARGO_PKG_NAME"))
    };
    fs::create_dir_all(&work_root)?;
    // Keep previous runs untouched and exclude their contents from this run's ZIP.
    let session = format!(
        "{}-{}",
        error_log::local_timestamp()
            .replace([':', '.'], "-")
            .replace(' ', "_"),
        std::process::id(),
    );
    let work_dir = work_root.join(&session);
    fs::create_dir_all(&work_dir)?;
    let log = ErrorLog::new(&work_dir)?;

    // The bundle belongs next to the program unless --zip said otherwise. Its folder is made
    // here and nowhere else, so a run that redirects the ZIP leaves no empty dump_file behind.
    let zip_path = match &zip_override {
        Some(destination) => cli::destination(destination, &session),
        None => std::env::current_dir()?.join(format!("{session}.zip")),
    };
    // `--zip name.zip` names a file in the working directory, which has no parent to create.
    if let Some(parent) = zip_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    // The reproduction has to survive reading the dialogs, so the processes are frozen before
    // anything is asked or collected and released as soon as the dumps have landed.
    let processes = status::run(t("status.processes"), processes::running);

    // Machine, permission and runtime details belong in every bundle, dump or no dump.
    system::collect(&work_dir, &log);
    // Logs and previously written crash artifacts describe the failure just as well while
    // the app is still running, so they are collected before the dump decision.
    diagnostics::collect(&work_dir, &log);

    let memory = matches!(&processes, Ok(list) if !list.is_empty()) && prompt::memory();
    // An unsuccessful query is not evidence that all target processes have stopped.
    let mut uncompressed = false;
    if let Some(processes) = log.report("PROCESSES", "query", processes)
        && !processes.is_empty()
    {
        if memory {
            prompt::full_dump_warning();
        }
        uncompressed = log
            .report(
                "DUMP",
                "collect",
                dump::collect(&work_dir, &processes, &log, memory),
            )
            .unwrap_or_default();
    }
    // A full dump is raw memory, so deflating earns its keep. Mini dumps are already small,
    // where compressing only costs time.
    let level = if uncompressed { "-mx=5" } else { "-mx=0" };
    let archived = status::run(t("status.archive"), || {
        archive::create(&work_dir, &zip_path, level)
    });
    if let Some(zip) = log.report("ZIP", "archive", archived) {
        println!("{}", t("run.done"));
        println!("{}", tr("run.bundle", &[("path", &zip.display())]));
        let _ = log.report("FOLDER", "explorer", transfer::open_folder(&zip));
        transfer::offer(&zip);
    }
    Ok(())
}
