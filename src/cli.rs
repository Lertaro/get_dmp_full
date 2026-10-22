use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

use crate::i18n::{t, tr};

pub(crate) const REPOSITORY: &str = "https://github.com/Lertaro/get_dmp_full";

/// What the command line asked for. Any argument other than the complete options below is treated as a
/// request for help, so a mistyped option never starts a capture.
pub(crate) enum Invocation {
    Help,
    Collect {
        zip: Option<PathBuf>,
        no_dump: bool,
        /// The ranking questions were already answered, so the run neither asks them nor collects
        /// the crash evidence around a dump.
        ranking: bool,
    },
}

pub(crate) fn resolve(arguments: &[OsString]) -> io::Result<Invocation> {
    let mut zip = None;
    let mut no_dump = false;
    let mut ranking = false;
    let mut other = false;
    let mut iterator = arguments.iter();
    while let Some(argument) = iterator.next() {
        if argument == "--zip" {
            let path = iterator
                .next()
                .ok_or_else(|| io::Error::other("missing path after --zip"))?;
            zip = Some(PathBuf::from(path));
        } else if argument == "--no-dump" {
            no_dump = true;
        } else if argument == "--ranking" {
            // A ranking bundle holds no dump by definition, so this turns the dump off rather than
            // leaving a second switch for the caller to keep in step.
            ranking = true;
            no_dump = true;
        } else {
            other = true;
        }
    }
    Ok(if other {
        Invocation::Help
    } else {
        Invocation::Collect {
            zip,
            no_dump,
            ranking,
        }
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
    fn only_the_known_options_collect() {
        assert!(matches!(
            resolve(&arguments(&[])).unwrap(),
            Invocation::Collect {
                zip: None,
                no_dump: false,
                ranking: false
            }
        ));
        assert!(matches!(
            resolve(&arguments(&["--zip", "D:\\out"])).unwrap(),
            Invocation::Collect {
                zip: Some(_),
                no_dump: false,
                ranking: false
            }
        ));
        assert!(matches!(
            resolve(&arguments(&["--no-dump", "--zip", "D:\\out.zip"])).unwrap(),
            Invocation::Collect {
                zip: Some(_),
                no_dump: true,
                ranking: false
            }
        ));
    }

    /// The ranking option is asked to imply the dump is off, so a caller cannot ask for a ranking
    /// bundle that then stalls the machine for minutes.
    #[test]
    fn ranking_turns_the_dump_off_with_it() {
        assert!(matches!(
            resolve(&arguments(&["--ranking"])).unwrap(),
            Invocation::Collect {
                zip: None,
                no_dump: true,
                ranking: true
            }
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
