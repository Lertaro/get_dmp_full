use std::{
    io,
    path::Path,
    process::{Command, Output},
};

use crate::error_log::ErrorLog;
use crate::i18n::tr;
use crate::processes;
use crate::{command, status, tools};

const COMPRESSED: &str = "-mac";
const UNCOMPRESSED: &str = "-ma";
const MINI: &str = "-mm";

/// Collects every target process, answering whether an uncompressed full dump landed.
pub(crate) fn collect(
    work_dir: &Path,
    processes: &[processes::Process],
    log: &ErrorLog,
    memory: bool,
) -> io::Result<bool> {
    let executable = tools::procdump()?;
    let mut uncompressed = false;
    for process in processes {
        let label = tr("status.dump", &[("process", &process)]);
        let landed = status::run(&label, || {
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
        Err(error) if is_write_refusal(&error) => {
            // Record the refusal so the reader learns this machine cannot compress.
            log.error("DUMP", process, format!("compressed dump refused: {error}"));
            capture(executable, work_dir, process, UNCOMPRESSED).map(|()| true)
        }
        result => result.map(|()| false),
    }
}

/// Whether ProDump failed because the system refused the dump file rather than because it never
/// reached the process. Only a refused write can be cured by dropping the compression, so
/// `Error opening` (no permission, no such process) is never retried.
fn is_write_refusal(error: &io::Error) -> bool {
    error.to_string().contains("Error writing dump file")
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

fn check(output: Output, code_page: u32) -> io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_dump_file_is_worth_retrying_uncompressed() {
        let refused = io::Error::other("ProcDump: Error writing dump file: 0x80070057");
        assert!(is_write_refusal(&refused));
    }

    #[test]
    fn a_process_that_was_never_reached_is_not_retried() {
        for reason in [
            "ProcDump: Error opening executable: file not found -系统找不到指定的文件。 (2)",
            "ProcDump: Error opening process",
        ] {
            assert!(!is_write_refusal(&io::Error::other(reason)), "{reason}");
        }
    }
}
