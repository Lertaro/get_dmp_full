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

pub(crate) fn procdump() -> io::Result<PathBuf> {
    extract(&[("procdump.exe", include_bytes!("../res/procdump.exe"))])
}

pub(crate) fn fastfetch() -> io::Result<PathBuf> {
    extract(&[("fastfetch.exe", include_bytes!("../res/fastfetch.exe"))])
}

pub(crate) fn zip() -> io::Result<PathBuf> {
    extract(&[
        ("7z.exe", include_bytes!("../res/7z.exe")),
        ("7z.dll", include_bytes!("../res/7z.dll")),
    ])
}
