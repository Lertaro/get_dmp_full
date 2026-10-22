//! Every question the user answers with a mouse instead of the keyboard.
//!
//! One dialog per question: the text is never printed to the console as well, and each
//! dialog carries its own localized title rather than a generic one.

use std::{fmt::Display, path::Path};

use win_msgbox_timeout::{info_msgbox, quest_msgbox_okcancel, quest_msgbox_yesno, warn_msgbox};

use crate::i18n::{t, tr};

/// The two button sets this program asks with, each paired with the answer that means yes.
/// Pairing them here is what keeps an OK/Cancel box from being judged by a Yes/No result:
/// `MessageBox` never returns `IDOK` for a Yes/No box, so a mismatched pair can only ever be
/// read as "no" and silently switches its step off.
enum Buttons {
    OkCancel,
    YesNo,
}

/// Win32 `MessageBox` results this program acts on.
const IDOK: i32 = 1;
const IDYES: i32 = 6;

fn ask(buttons: Buttons, body: impl Into<String>, title: &str) -> bool {
    let body = body.into();
    match buttons {
        Buttons::OkCancel => quest_msgbox_okcancel(body, title, 0) == IDOK,
        Buttons::YesNo => quest_msgbox_yesno(body, title, 0) == IDYES,
    }
}

/// Waits until the reproduction is on screen, because a dump of a state the user has
/// already navigated away from cannot be taken again.
pub(crate) fn ready() -> bool {
    ask(
        Buttons::OkCancel,
        t("prompt.ready.body"),
        t("prompt.ready.title"),
    )
}

/// Asks which kind of problem this is, because the two answers collect almost nothing in common: a
/// wrong order is answered by the ranking log and the settings, and a crash by the dump and the crash
/// artifacts. Asked second, after the reproduction is confirmed, so a person who cancelled never sees
/// it.
pub(crate) fn ranking() -> bool {
    ask(
        Buttons::YesNo,
        t("prompt.ranking.body"),
        t("prompt.ranking.title"),
    )
}

/// Decides the dump type, so it is only asked while a target process is running.
pub(crate) fn memory() -> bool {
    ask(
        Buttons::YesNo,
        t("prompt.memory.body"),
        t("prompt.memory.title"),
    )
}

/// Warns before a full dump, which reads every page of the process and stalls the machine for
/// minutes. The mini dump path never sees this dialog: it takes a second and stutters nothing.
///
/// Both full-dump types are covered because the compressed one is what is attempted first; a
/// machine that refuses it falls back to the uncompressed one, which stalls the machine just as
/// hard. The refusal is only known once the capture has already started.
pub(crate) fn full_dump_warning() {
    warn_msgbox(t("prompt.full_dump.body"), t("prompt.full_dump.title"), 0);
}

pub(crate) fn upload(zip: &Path) -> bool {
    ask(
        Buttons::YesNo,
        tr("prompt.upload.body", &[("path", &zip.display())]),
        t("prompt.upload.title"),
    )
}

pub(crate) fn upload_instructions(zip: &Path, site: &str) {
    info_msgbox(
        tr(
            "prompt.uploaded.body",
            &[("site", &site), ("path", &zip.display())],
        ),
        t("prompt.uploaded.title"),
        0,
    );
}

pub(crate) fn browser_blocked(error: impl Display, zip: &Path, site: &str) {
    warn_msgbox(
        tr(
            "prompt.browser.body",
            &[("error", &error), ("site", &site), ("path", &zip.display())],
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
