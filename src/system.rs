use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::error_log::ErrorLog;
use crate::i18n::t;
use crate::{command, processes, status, tools};

const PRODUCT: &str = "lertaro";
const SERVICE_NAME: &str = "LertaroService";
// An x64 process reads the 64-bit registry view, so the 32-bit and per-user
// uninstall roots have to be enumerated as well.
const UNINSTALL_ROOTS: [&str; 3] = [
    r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
];

const ADMINISTRATORS_SID: &str = "S-1-5-32-544";
const UAC_ROOT: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
const UAC_VALUES: [&str; 6] = [
    "EnableLUA",
    "ConsentPromptBehaviorAdmin",
    "ConsentPromptBehaviorUser",
    "PromptOnSecureDesktop",
    "FilterAdministratorToken",
    "LocalAccountTokenFilterPolicy",
];

const FRAMEWORK_KEYS: [&str; 2] = [
    r"HKLM\SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full",
    r"HKLM\SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Client",
];
// Microsoft documents 4.x as cumulative: the Release value names the newest build
// present, so every threshold below means "that version and its predecessors".
const FRAMEWORK_RELEASES: [(&str, u32); 11] = [
    ("4.5", 378_389),
    ("4.5.1", 379_893),
    ("4.5.2", 380_522),
    ("4.6", 393_297),
    ("4.6.1", 394_294),
    ("4.6.2", 394_802),
    ("4.7", 460_793),
    ("4.7.1", 461_308),
    ("4.7.2", 461_808),
    ("4.8", 528_040),
    ("4.8.1", 533_320),
];

// The versions the report calls out by name. Their thresholds come from FRAMEWORK_RELEASES,
// so no Release number is written down twice.
const FRAMEWORK_CHECKLIST: [&str; 2] = ["4.5", "4.8"];

const DOTNET_COMPONENTS: [&str; 3] = [
    "shared/Microsoft.NETCore.App",
    "shared/Microsoft.WindowsDesktop.App",
    "sdk",
];
const DOTNET_MAJORS: [u32; 4] = [7, 8, 9, 10];

pub(crate) fn collect(work_dir: &Path, log: &ErrorLog, origins: &[Option<PathBuf>]) {
    let _ = log.report(
        "SYSTEM",
        "fastfetch.txt",
        status::run(t("status.system"), || fastfetch(work_dir)),
    );
    let _ = log.report(
        "SYSTEM",
        "icacls.txt",
        status::run(t("status.permissions"), || {
            permissions(work_dir, log, origins)
        }),
    );
    let _ = log.report(
        "SYSTEM",
        "account.txt",
        status::run(t("status.account"), || account(work_dir, log)),
    );
    let _ = log.report(
        "SYSTEM",
        "dotnet.txt",
        status::run(t("status.dotnet"), || dotnet(work_dir, log)),
    );
}

fn write_report(work_dir: &Path, name: &str, text: &str) -> io::Result<()> {
    let directory = work_dir.join("system");
    fs::create_dir_all(&directory)?;
    // A BOM lets Windows text editors recognize UTF-8 in any system locale.
    fs::write(directory.join(name), format!("\u{feff}{text}"))
}

fn fastfetch(work_dir: &Path) -> io::Result<()> {
    // --logo None keeps the report plain text instead of ASCII art.
    let text = command::text_utf8(tools::fastfetch()?, &["--logo", "None"])?;
    write_report(work_dir, "fastfetch.txt", &text)
}

fn permissions(work_dir: &Path, log: &ErrorLog, origins: &[Option<PathBuf>]) -> io::Result<()> {
    let mut targets: Vec<(&'static str, PathBuf)> = Vec::new();
    for (label, path) in install_dirs(log) {
        push(&mut targets, label, path);
    }
    for (label, path) in running_targets(origins, log) {
        push(&mut targets, label, path);
    }
    for (label, variable) in [
        ("config-user", "LOCALAPPDATA"),
        ("config-machine", "ProgramData"),
    ] {
        let Some(base) = log.environment_path("PERMS", variable) else {
            continue;
        };
        push(&mut targets, label, base.join("Lertaro"));
    }

    let mut text = String::new();
    for (label, path) in &targets {
        text.push_str(&format!("[{label}] {}\r\n", path.display()));
        text.push_str(&match describe(path) {
            // A failure is a finding: keep it in the report next to its directory.
            Ok(output) => output,
            Err(error) => format!("({error})\r\n\r\n"),
        });
    }
    if targets.is_empty() {
        text.push_str("(no directory to inspect)\r\n");
    }
    write_report(work_dir, "icacls.txt", &text)
}

fn describe(path: &Path) -> io::Result<String> {
    if !path.try_exists()? {
        return Ok("(directory does not exist)\r\n\r\n".to_owned());
    }
    let mut text = command::text("icacls.exe", &[&path.to_string_lossy()])?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("\r\n");
    Ok(text)
}

fn account(work_dir: &Path, log: &ErrorLog) -> io::Result<()> {
    let mut text = String::new();
    let mut groups = String::new();
    for argument in ["/user", "/groups", "/priv"] {
        let section = format!("whoami {argument}");
        match log.report(
            "ACCOUNT",
            &section,
            command::text("whoami.exe", &[argument]),
        ) {
            Some(output) => {
                if argument == "/groups" {
                    groups = output.clone();
                }
                text.push_str(&format!("[{section}]\r\n{output}\r\n"));
            }
            None => text.push_str(&format!("[{section}]\r\n(unavailable)\r\n")),
        }
    }

    text.push_str(&format!("\r\n[UAC policy] {UAC_ROOT}\r\n"));
    if let Some(values) = log.report("ACCOUNT", UAC_ROOT, registry_values(UAC_ROOT)) {
        for name in UAC_VALUES {
            let value = registry_value(&values, name).unwrap_or("(absent)");
            text.push_str(&format!("{name} = {value}\r\n"));
        }
    }

    // Both an elevated and a UAC-filtered token carry the Administrators SID, so its
    // presence is what proves the account holds local administrator rights.
    let admin = groups.contains(ADMINISTRATORS_SID);
    let integrity = integrity(&groups);
    text.push_str(&format!(
        "\r\n[summary]\r\nlocal administrator: {}\r\nintegrity: {integrity}\r\nalready elevated: {}\r\n",
        yes_no(admin),
        yes_no(integrity.starts_with("High")),
    ));
    write_report(work_dir, "account.txt", &text)
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn integrity(groups: &str) -> String {
    let sid = groups
        .split_whitespace()
        .find(|token| token.starts_with("S-1-16-"))
        .unwrap_or("unknown");
    let level = match sid {
        "S-1-16-4096" => "Low",
        "S-1-16-8192" => "Medium",
        "S-1-16-12288" => "High",
        "S-1-16-16384" => "System",
        _ => "Unrecognized",
    };
    format!("{level} ({sid})")
}

fn dotnet(work_dir: &Path, log: &ErrorLog) -> io::Result<()> {
    let mut text = String::new();
    let mut release = None;
    let mut installed = false;
    text.push_str("[.NET Framework]\r\n");
    for key in FRAMEWORK_KEYS {
        match log.report("DOTNET", key, registry_values(key)) {
            Some(values) => {
                text.push_str(&format!("{key}\r\n"));
                for name in ["Install", "Release", "Version"] {
                    if let Some(value) = registry_value(&values, name) {
                        text.push_str(&format!("    {name} = {value}\r\n"));
                    }
                }
                installed = true;
                if release.is_none() {
                    release = registry_value(&values, "Release").and_then(parse_release);
                }
            }
            None => text.push_str(&format!("{key} = (absent)\r\n")),
        }
    }
    text.push_str("\r\n[framework checklist]\r\n");
    text.push_str(&format!("4: {}\r\n", yes_no(installed)));
    for label in FRAMEWORK_CHECKLIST {
        text.push_str(&format!(
            "{label}: {}\r\n",
            match release {
                Some(value) => yes_no(release_at_least(value, label)),
                None => "unknown",
            }
        ));
    }
    if let Some(value) = release {
        text.push_str(&format!("at least: {}\r\n", framework_name(value)));
    }

    text.push_str("\r\n[.NET runtime and SDK]\r\n");
    let mut versions: Vec<(&'static str, &'static str, String)> = Vec::new();
    for (label, variable) in [("x64", "ProgramFiles"), ("x86", "ProgramFiles(x86)")] {
        let Some(base) = log.environment_path("DOTNET", variable) else {
            continue;
        };
        let root = base.join("dotnet");
        // One unreadable folder is written into the report and skipped; aborting here would
        // throw away the runtimes that were already listed.
        let exists = match root.try_exists() {
            Ok(exists) => exists,
            Err(error) => {
                log.error("DOTNET", root.display(), error);
                text.push_str(&format!("{} ({label}) = (unreadable)\r\n", root.display()));
                continue;
            }
        };
        if !exists {
            text.push_str(&format!("{} ({label}) = (absent)\r\n", root.display()));
            continue;
        }
        text.push_str(&format!("{} ({label})\r\n", root.display()));
        for component in DOTNET_COMPONENTS {
            let path = root.join(component);
            match log.report("DOTNET", path.display(), installed_versions(&path)) {
                Some(found) => {
                    text.push_str(&format!(
                        "    {component}: {}\r\n",
                        if found.is_empty() {
                            String::from("(none)")
                        } else {
                            found.join(", ")
                        }
                    ));
                    versions.extend(found.into_iter().map(|version| (label, component, version)));
                }
                None => text.push_str(&format!("    {component}: (unreadable)\r\n")),
            }
        }
    }
    text.push_str("\r\n[dotnet checklist]\r\n");
    for major in DOTNET_MAJORS {
        let found: Vec<String> = versions
            .iter()
            .filter(|(_, _, version)| version_major(version) == Some(major))
            .map(|(label, component, version)| {
                let leaf = component.rsplit('/').next().unwrap_or(component);
                format!("{leaf} {version} ({label})")
            })
            .collect();
        text.push_str(&format!(
            "{major}: {}\r\n",
            if found.is_empty() {
                String::from("not installed")
            } else {
                found.join("; ")
            }
        ));
    }

    write_report(work_dir, "dotnet.txt", &text)
}

fn installed_versions(directory: &Path) -> io::Result<Vec<String>> {
    if !directory.try_exists()? {
        return Ok(Vec::new());
    }
    let mut versions: Vec<String> = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    versions.sort_by_key(|version| version_key(version));
    Ok(versions)
}

fn version_key(version: &str) -> (u32, u32, u32) {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or_default());
    (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    )
}

fn version_major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

fn parse_release(value: &str) -> Option<u32> {
    u32::from_str_radix(value.trim().strip_prefix("0x")?, 16).ok()
}

fn framework_name(release: u32) -> &'static str {
    let mut name = "4.0";
    for (label, minimum) in FRAMEWORK_RELEASES {
        if release >= minimum {
            name = label;
        }
    }
    name
}

/// Whether a Release value means the named version is present. 4.x is cumulative, so this is
/// the same walk `framework_name` does, stopped at one label.
fn release_at_least(release: u32, label: &str) -> bool {
    FRAMEWORK_RELEASES
        .iter()
        .find(|(name, _)| *name == label)
        .is_some_and(|(_, minimum)| release >= *minimum)
}

/// The folders of the copies that are actually running. Neither of them is in `install_dirs` or in
/// the AppData pair when a portable copy is unpacked somewhere else: its program folder is not
/// registered, and its state is not in AppData, so a permission problem there would go unreported.
fn running_targets(origins: &[Option<PathBuf>], log: &ErrorLog) -> Vec<(&'static str, PathBuf)> {
    let mut targets: Vec<(&'static str, PathBuf)> = Vec::new();
    for origin in origins.iter().flatten() {
        push(&mut targets, "install-running", origin.clone());
        if let Some(data) = processes::portable_data(origin, log) {
            push(&mut targets, "config-portable", data);
        }
    }
    targets
}

fn install_dirs(log: &ErrorLog) -> Vec<(&'static str, PathBuf)> {
    let mut dirs: Vec<(&'static str, PathBuf)> = Vec::new();
    for root in UNINSTALL_ROOTS {
        let Some(text) = log.report("INSTALL", root, registry_query(root)) else {
            continue;
        };
        parse_uninstall(&text, &mut dirs);
    }
    // The service image path is where lertaro.service.exe really runs from, which
    // outranks the installer's record after a manual move or a development build.
    if let Some(path) = log.report("INSTALL", SERVICE_NAME, service_dir()).flatten() {
        push(&mut dirs, "install-service", path);
    }
    dirs
}

fn parse_uninstall(text: &str, dirs: &mut Vec<(&'static str, PathBuf)>) {
    let mut values: Vec<(&str, &str)> = Vec::new();
    for line in text.lines() {
        if line.starts_with("HKEY_") {
            if let Some(path) = install_location(&values) {
                push(dirs, "install-registry", path);
            }
            values.clear();
        } else if let Some(value) = parse_value(line) {
            values.push(value);
        }
    }
    if let Some(path) = install_location(&values) {
        push(dirs, "install-registry", path);
    }
}

fn registry_query(root: &str) -> io::Result<String> {
    command::text("reg.exe", &["query", root, "/s"])
}

fn registry_values(key: &str) -> io::Result<String> {
    command::text("reg.exe", &["query", key])
}

fn registry_value<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(parse_value)
        .find(|(found, _)| found.eq_ignore_ascii_case(name))
        .map(|(_, data)| data)
}

fn parse_value(line: &str) -> Option<(&str, &str)> {
    // reg writes every value as `name    REG_TYPE    data`, indented by four spaces.
    let mut fields = line.trim_start().splitn(3, "    ");
    let name = fields.next()?.trim_end();
    let _ = fields.next()?;
    Some((name, fields.next()?.trim()))
}

fn install_location(values: &[(&str, &str)]) -> Option<PathBuf> {
    if !values.iter().any(|(name, data)| {
        name.eq_ignore_ascii_case("DisplayName") && data.to_ascii_lowercase().contains(PRODUCT)
    }) {
        return None;
    }
    ["InstallLocation", "Inno Setup: App Path"]
        .iter()
        .find_map(|key| {
            values
                .iter()
                .find(|(name, data)| name.eq_ignore_ascii_case(key) && !data.is_empty())
                .map(|(_, data)| PathBuf::from(data))
        })
}

fn service_dir() -> io::Result<Option<PathBuf>> {
    let text = command::text("sc.exe", &["qc", SERVICE_NAME])?;
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with("BINARY_PATH_NAME"))
        .ok_or_else(|| io::Error::other("BINARY_PATH_NAME is missing"))?;
    let value = line
        .split_once(':')
        .map(|(_, value)| value.trim())
        .ok_or_else(|| io::Error::other("unexpected service configuration format"))?;
    let executable = value
        .strip_prefix('"')
        .and_then(|quoted| quoted.split('"').next())
        .unwrap_or(value)
        .split_whitespace()
        .next()
        .unwrap_or_default();
    Ok(Path::new(executable)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf))
}

// Registry values, service image paths and environment variables describe the same
// directory differently, so compare them as trimmed, case-folded text.
fn push(targets: &mut Vec<(&'static str, PathBuf)>, label: &'static str, path: PathBuf) {
    let text = path
        .to_string_lossy()
        .trim_matches(['"', ' ', '/', '\\'])
        .to_owned();
    if text.is_empty()
        || targets
            .iter()
            .any(|(_, existing)| existing.to_string_lossy().eq_ignore_ascii_case(&text))
    {
        return;
    }
    targets.push((label, PathBuf::from(text)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checklist_labels_have_thresholds_in_the_shared_table() {
        for label in FRAMEWORK_CHECKLIST {
            assert!(
                FRAMEWORK_RELEASES.iter().any(|(name, _)| *name == label),
                "{label} has no Release threshold to compare against"
            );
        }
    }

    #[test]
    fn framework_release_thresholds_are_cumulative() {
        assert!(release_at_least(533_320, "4.5"));
        assert!(release_at_least(533_320, "4.8"));
        assert!(release_at_least(394_802, "4.5"));
        assert!(!release_at_least(394_802, "4.8"));
        assert_eq!(framework_name(394_802), "4.6.2");
        assert_eq!(framework_name(1), "4.0");
    }

    #[test]
    fn dotnet_version_names_sort_and_split_by_major() {
        assert!(version_key("8.0.2") < version_key("8.0.10"));
        assert_eq!(version_major("10.0.0-preview.1"), Some(10));
        assert_eq!(version_major(""), None);
    }

    #[test]
    fn registry_release_values_are_hex_with_their_prefix() {
        assert_eq!(parse_release("0x80231"), Some(524_849));
        assert_eq!(parse_release("533320"), None);
    }

    /// A portable copy's folder is neither in the uninstall registry nor in the AppData pair, so
    /// the report has to name it and its `Data` from the running copy itself.
    #[test]
    fn a_running_copy_adds_its_own_folder_and_its_data() {
        let portable = std::env::temp_dir().join("get_dump_full-perms-portable");
        let installed = std::env::temp_dir().join("get_dump_full-perms-installed");
        for directory in [&portable, &installed] {
            let _ = fs::remove_dir_all(directory);
        }
        fs::create_dir_all(portable.join("Data")).unwrap();
        fs::create_dir_all(&installed).unwrap();
        let log = ErrorLog::new(&installed).unwrap();

        let targets = running_targets(
            &[
                Some(portable.clone()),
                Some(portable.clone()),
                Some(installed.clone()),
                None,
            ],
            &log,
        );
        let labels: Vec<&str> = targets.iter().map(|(label, _)| *label).collect();
        assert_eq!(
            labels,
            ["install-running", "config-portable", "install-running"]
        );
        assert_eq!(targets[1].1, portable.join("Data"));
        assert_eq!(targets[2].1, installed);
    }

    /// The same chain the run uses: the folders of a running copy reach `icacls.exe`, and its
    /// output for them lands in the report under their labels. The fixture stands in for a copy
    /// nobody asked to start, and the assertions stay free of report wording because `icacls`
    /// answers in the console language.
    #[test]
    fn a_running_copy_is_inspected_by_icacls_in_the_report() {
        let copy = std::env::temp_dir().join("get_dump_full-perms-report");
        let _ = fs::remove_dir_all(&copy);
        fs::create_dir_all(copy.join("Data")).unwrap();
        let work = copy.join("bundle");
        fs::create_dir_all(&work).unwrap();
        let log = ErrorLog::new(&work).unwrap();

        permissions(&work, &log, &[Some(copy.clone())]).unwrap();
        let text = fs::read_to_string(work.join("system/icacls.txt")).unwrap();
        let _ = fs::remove_dir_all(&copy);

        for (label, path) in [
            ("install-running", copy.clone()),
            ("config-portable", copy.join("Data")),
        ] {
            let header = format!("[{label}] {}\r\n", path.display());
            let section = text
                .split_once(&header)
                .unwrap_or_else(|| panic!("{label} is missing from the report:\n{text}"))
                .1
                .split("\r\n\r\n")
                .next()
                .unwrap_or_default();
            assert!(
                section.contains(":("),
                "{label} has no access entry, so icacls did not read it:\n{section}"
            );
        }
    }
}
