//! The bundle's own index: what was collected, how big it is, and what the collector promised not to
//! read.
//!
//! The ZIP is handed to a stranger, so the inventory has to come from the bundle rather than from the
//! collector's intentions: this walks the finished working directory and lists what is actually there.
//! The privacy note is spelled out for the same reason -- a reader can check a claim against the byte
//! counts instead of trusting the README.

use std::{fs, io, path::Path};

use crate::error_log::local_timestamp;

const MANIFEST: &str = "manifest.json";

/// Writes `manifest.json` into the working directory. The archive step that follows picks it up like
/// any other file, so the inventory is everything that ships, minus this file itself.
///
/// `purpose` is what the run was told to look for, which a reader needs to judge the absence of
/// things: a ranking bundle holds no dump and no crash evidence on purpose.
pub(crate) fn write(work_dir: &Path, purpose: &str, dump: &str) -> io::Result<()> {
    let mut files = Vec::new();
    inventory(work_dir, work_dir, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"tool\": \"get_dmp_full\",\n");
    json.push_str(&format!(
        "  \"version\": \"{}\",\n",
        env!("CARGO_PKG_VERSION")
    ));
    json.push_str(&format!("  \"purpose\": {},\n", quote(purpose)));
    json.push_str(&format!(
        "  \"collectedAt\": {},\n",
        quote(&local_timestamp())
    ));
    json.push_str(&format!("  \"dump\": {},\n", quote(dump)));
    json.push_str(&format!("  \"fileCount\": {},\n", files.len()));

    json.push_str("  \"files\": [\n");
    for (index, (name, bytes)) in files.iter().enumerate() {
        json.push_str(&format!(
            "    {{\"path\": {}, \"bytes\": {bytes}}}{}\n",
            quote(name),
            if index + 1 == files.len() { "" } else { "," }
        ));
    }
    json.push_str("  ],\n");

    json.push_str("  \"privacy\": {\n");
    json.push_str("    \"rankingLog\": \"ranking-events*.jsonl holds a 12-character truncated SHA-256 of \
         the query and of the path; every other field is a number (section, score, type rank, index-rule \
         priority, history tier, match shape, name length, depth, extension class). No query text, path \
         or file name is in it.\",\n");
    json.push_str(
        "    \"settings\": \"user-settings.json and plugin-settings.json are copied verbatim: \
         they hold the paths, rules and folders you configured.\",\n",
    );
    json.push_str(
        "    \"notCollected\": \"search history, index cache, clipboard, browser data, \
         credentials, other applications' files.\",\n",
    );
    json.push_str(
        "    \"dump\": \"when dump is mini or full, the dump is a raw memory snapshot and can \
         contain what was on screen, the clipboard and live tokens.\"\n",
    );
    json.push_str("  }\n");
    json.push_str("}\n");

    fs::write(work_dir.join(MANIFEST), json)
}

/// Every file below `root`, as a ZIP-style relative path and its size. Directories are not listed:
/// an empty one carries no evidence. The walk keeps the top directory separate from the one it is
/// reading, or a nested file would be named relative to its own folder instead of the bundle.
fn inventory(root: &Path, directory: &Path, files: &mut Vec<(String, u64)>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            inventory(root, &path, files)?;
        } else if kind.is_file() {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if name == MANIFEST {
                continue;
            }
            files.push((name, entry.metadata()?.len()));
        }
    }
    Ok(())
}

/// JSON string escaping. Paths cannot contain a quote on NTFS, but a localized folder name can carry
/// anything a string can, and one unescaped character would make the whole file unreadable.
fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_is_relative_sorted_and_skips_the_manifest() {
        let root = std::env::temp_dir().join(format!("dmp-manifest-{}", std::process::id()));
        let nested = root.join("state/user");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("run.log"), "x").unwrap();
        fs::write(nested.join("user-settings.json"), "{}").unwrap();
        fs::write(root.join(MANIFEST), "{}").unwrap();

        let mut files = Vec::new();
        inventory(&root, &root, &mut files).unwrap();
        files.sort_by(|left, right| left.0.cmp(&right.0));

        assert_eq!(
            vec![
                ("run.log".to_owned(), 1),
                ("state/user/user-settings.json".to_owned(), 2)
            ],
            files
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_quote_and_a_backslash_survive_escaping() {
        assert_eq!("\"a\\\\b\\\"c\"", quote("a\\b\"c"));
    }
}
