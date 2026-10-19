//! Every question the user answers with a mouse instead of the keyboard.
//!
//! One dialog per question: the text is never printed to the console as well, and each
//! dialog carries its own localized title rather than a generic one.

use std::{fmt::Display, path::Path};

use win_msgbox_timeout::{info_msgbox, quest_msgbox_yesno, warn_msgbox};

use crate::i18n::{t, tr};
use crate::transfer;

/// Win32 `MessageBox` results this program acts on.
const IDOK: i32 = 1;
const IDYES: i32 = 6;

/// Waits until the reproduction is on screen, because a dump of a state the user has
/// already navigated away from cannot be taken again.
pub(crate) fn ready() -> bool {
    quest_msgbox_yesno(t("prompt.ready.body"), t("prompt.ready.title"), 0) == IDOK
}

/// Decides the dump type, so it is only asked while a target process is running.
pub(crate) fn memory() -> bool {
    quest_msgbox_yesno(t("prompt.memory.body"), t("prompt.memory.title"), 0) == IDYES
}

pub(crate) fn upload(zip: &Path) -> bool {
    quest_msgbox_yesno(
        tr("prompt.upload.body", &[("path", &zip.display())]),
        t("prompt.upload.title"),
        0,
    ) == IDYES
}

pub(crate) fn upload_instructions(zip: &Path) {
    info_msgbox(
        tr(
            "prompt.uploaded.body",
            &[("site", &transfer::UPLOAD_SITE), ("path", &zip.display())],
        ),
        t("prompt.uploaded.title"),
        0,
    );
}

pub(crate) fn browser_blocked(error: impl Display, zip: &Path) {
    warn_msgbox(
        tr(
            "prompt.browser.body",
            &[
                ("error", &error),
                ("site", &transfer::UPLOAD_SITE),
                ("path", &zip.display()),
            ],
        ),
        t("prompt.uploaded.title"),
        0,
    );
}

/// Holds a double-clicked run's window open until the user has read it, without
/// asking for a keystroke.
pub(crate) fn finish() {
    info_msgbox(t("prompt.finish.body"), t("prompt.finish.title"), 0);
}
