mod archive;
mod cli;
mod command;
mod diagnostics;
mod dump;
mod error_log;
mod i18n;
mod logs;
mod manifest;
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
        cli::Invocation::Collect {
            zip,
            no_dump,
            ranking,
        } => collect(zip, no_dump, ranking),
    }
}

/// Collects everything that describes the failure and packs it.
fn collect(zip_override: Option<PathBuf>, no_dump: bool, ranking: bool) -> io::Result<()> {
    // Nothing is read or written until the reproduction is confirmed on screen.
    if !prompt::ready() {
        println!("{}", t("run.cancelled"));
        return Ok(());
    }
    // Which report this is decides most of the run, so it is asked before anything is read. Given on
    // the command line, it is not asked again.
    let ranking = ranking || prompt::ranking();

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

    // The bundle lands in the working directory unless --zip said otherwise, so a double-clicked
    // run writes it beside the program.
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

    // The running copies decide which log folders hold the failure and whether a dump is even
    // possible, so the list is read before anything is collected.
    let processes = status::run(t("status.processes"), processes::running);
    // Where each running copy keeps its state is asked by the permission report and by the log
    // collection, and answering it costs a query per process, so it is read once here.
    let origins = processes::origins(processes.as_deref().unwrap_or_default(), &log);

    // Machine, permission and runtime details belong in every bundle, dump or no dump.
    system::collect(&work_dir, &log, &origins);
    let mut dump = "none";
    let mut uncompressed = false;
    if ranking {
        // A wrong order is answered by the settings, the ranking log and the window that is on
        // screen. The event log, the error reports, the dumps already on disk and a fresh dump
        // describe a failure, so a ranking run reads none of them and asks about none of them.
        diagnostics::collect_state(&work_dir, &log, &origins);
        dump = "skipped";
    } else {
        // Logs and previously written crash artifacts describe the failure just as well while
        // the app is still running, so they are collected before the dump decision.
        diagnostics::collect(&work_dir, &log, &origins);
        // A dump of the heap answers "why did it grow", not "why is this row third". A wrong result or
        // a wrong order is a state question, so --no-dump trades a few hundred megabytes and minutes of
        // stalling for the settings and the ranking log, which is what the answer is in.
        let memory =
            !no_dump && matches!(&processes, Ok(list) if !list.is_empty()) && prompt::memory();
        // An unsuccessful query is not evidence that all target processes have stopped.
        if no_dump {
            dump = "skipped";
            let _ = log.report("DUMP", "skipped by --no-dump", Ok::<(), io::Error>(()));
        } else if let Some(processes) = log.report("PROCESSES", "query", processes)
            && !processes.is_empty()
        {
            if memory {
                prompt::full_dump_warning();
            }
            if let Some(captured) = log.report(
                "DUMP",
                "collect",
                dump::collect(&work_dir, &processes, &log, memory),
            ) {
                uncompressed = captured;
                dump = if uncompressed { "full" } else { "mini" };
            }
        }
    }
    // Written after the collection so the inventory describes what actually landed, and before the
    // archive so it ships with it.
    status::step(t("status.manifest"), || {
        let purpose = if ranking { "ranking" } else { "crash" };
        let _ = log.report(
            "MANIFEST",
            "write",
            manifest::write(&work_dir, purpose, dump),
        );
    });
    // A full dump is raw memory, so deflating earns its keep. A mini dump was once stored whole on
    // the theory that it is already small; 25 MB of one, with a real state tree beside it, came out
    // at 7.5 MB in 0.4 s instead of 25 MB in 0.1 s, and the same pass is what shrinks the settings
    // and the ranking log. The transfer to the developer is the part that costs real time.
    let level = if uncompressed { "-mx=5" } else { "-mx=1" };
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
