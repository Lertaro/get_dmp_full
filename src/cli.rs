use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

use crate::i18n::{t, tr};

pub(crate) const REPOSITORY: &str = "https://github.com/Lertaro/get_dmp_full";

/// What the command line asked for. Any argument other than a complete `--zip <path>` is
/// treated as a request for help, so a mistyped option never starts a capture.
pub(crate) enum Invocation {
    Help,
    Collect { zip: Option<PathBuf> },
}

pub(crate) fn resolve(arguments: &[OsString]) -> io::Result<Invocation> {
    let mut zip = None;
    let mut other = false;
    let mut iterator = arguments.iter();
    while let Some(argument) = iterator.next() {
        if argument == "--zip" {
            let path = iterator
                .next()
                .ok_or_else(|| io::Error::other("missing path after --zip"))?;
            zip = Some(PathBuf::from(path));
        } else {
            other = true;
        }
    }
    Ok(if other {
        Invocation::Help
    } else {
        Invocation::Collect { zip }
    })
}

pub(crate) fn show_help() {
    println!();
    println!("{}", t("help.title"));
    println!();
    println!("{}", t("help.summary"));
    println!();
    println!("{}", t("help.usage"));
    println!("{}", t("help.location"));
    println!();
    println!("{}", tr("help.repository", &[("url", &REPOSITORY)]));
}

/// `--zip` may name a folder, which keeps the timestamped file name, or a file, which is used
/// exactly as given.
pub(crate) fn destination(folder: &Path, session: &str) -> PathBuf {
    if folder
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        folder.to_path_buf()
    } else {
        folder.join(format!("{session}.zip"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_a_bare_or_zip_command_line_collects() {
        assert!(matches!(
            resolve(&arguments(&[])).unwrap(),
            Invocation::Collect { zip: None }
        ));
        assert!(matches!(
            resolve(&arguments(&["--zip", "D:\\out"])).unwrap(),
            Invocation::Collect { zip: Some(_) }
        ));
    }

    #[test]
    fn every_other_argument_only_shows_help() {
        for argument in ["--help", "-h", "help", "zipp"] {
            assert!(
                matches!(resolve(&arguments(&[argument])).unwrap(), Invocation::Help),
                "{argument} must not start a capture"
            );
        }
    }

    #[test]
    fn a_lone_zip_option_is_an_error() {
        assert!(resolve(&arguments(&["--zip"])).is_err());
    }

    #[test]
    fn a_zip_named_destination_is_used_verbatim() {
        assert_eq!(
            destination(Path::new("D:\\out\\a.zip"), "session"),
            PathBuf::from("D:\\out\\a.zip")
        );
        assert_eq!(
            destination(Path::new("D:\\out"), "session"),
            PathBuf::from("D:\\out\\session.zip")
        );
    }
}
