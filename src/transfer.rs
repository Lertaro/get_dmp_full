use std::{io, path::Path};

use crate::i18n::t;
use crate::{command, prompt};

const UPLOAD_SITE: &str = "https://app.dashbeam.net";

/// Opens the folder holding the ZIP so the user never has to reconstruct the path.
pub(crate) fn open_folder(zip: &Path) -> io::Result<()> {
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

/// Offers to hand the bundle to the developer. The program itself never goes online.
pub(crate) fn offer(zip: &Path) {
    if prompt::upload(zip) {
        // rundll32 hands the URL to the default browser without a shell, so it needs no quoting.
        match command::text(
            "rundll32.exe",
            &["url.dll,FileProtocolHandler", UPLOAD_SITE],
        ) {
            Ok(_) => prompt::upload_instructions(zip, UPLOAD_SITE),
            Err(error) => prompt::browser_blocked(error, zip, UPLOAD_SITE),
        }
    }
    println!("{}", t("prompt.drive"));
}
