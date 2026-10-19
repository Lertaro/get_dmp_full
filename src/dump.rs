use std::{
    io,
    path::Path,
    process::{Command, Output},
};

use crate::error_log::ErrorLog;
use crate::i18n::tr;
use crate::processes;
use crate::{command, status, tools};

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
    let dump_type = dump_type(memory);
    let mut uncompressed = false;
    for process in processes {
        let label = tr("status.dump", &[("process", &process)]);
        let landed = status::run(&label, || {
            capture(&executable, work_dir, process, dump_type)
        });
        if memory && log.report("DUMP", process, landed).is_some() {
            uncompressed = true;
        }
    }
    Ok(uncompressed)
}

/// Stacks, metadata and what the stacks reference are enough for crashes, hangs and startup
/// failures, and that is roughly 3% of a full dump. Only `-ma` carries a heap, and `-mac` is not
/// attempted: it reads the same memory, some machines refuse it outright, so trying it first
/// would only spend a failed pass before the capture that works.
fn dump_type(memory: bool) -> &'static str {
    if memory { UNCOMPRESSED } else { MINI }
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
    fn only_a_memory_question_answered_yes_writes_a_full_dump() {
        assert_eq!(dump_type(true), UNCOMPRESSED);
        assert_eq!(dump_type(false), MINI);
    }
}
