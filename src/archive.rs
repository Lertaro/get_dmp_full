use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{command, tools};

pub(crate) fn create(work_dir: &Path, destination: &Path, level: &str) -> io::Result<PathBuf> {
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
