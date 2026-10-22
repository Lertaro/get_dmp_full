//! Language detection and message catalogs.
//!
//! The catalogs are nested hash maps: `language code -> key -> text`. Adding a language
//! means registering one more table in [`catalogs`] and teaching `code_of_tag` and
//! `code_of_langid` which languages select it; no call site changes.

use std::{collections::HashMap, fmt::Display, sync::OnceLock};

type Key = &'static str;
type Table = HashMap<Key, &'static str>;

const ZH: &str = "zh-Hans";
const EN: &str = "en";

static LANGUAGE: OnceLock<&'static str> = OnceLock::new();
static CATALOGS: OnceLock<HashMap<&'static str, Table>> = OnceLock::new();

/// Text for `key` in the detected language, with `{name}` markers filled from `args`.
pub(crate) fn tr(key: Key, args: &[(&str, &dyn Display)]) -> String {
    let mut text = String::from(t(key));
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), &value.to_string());
    }
    // A marker left behind means a call site forgot an argument, and the dialog would show
    // `{path}` to the user. Debug builds stop on that instead of shipping it.
    debug_assert!(!text.contains('{'), "unfilled placeholder in {key}: {text}");
    text
}

/// Text for `key` in the detected language, falling back to English and then to the key.
pub(crate) fn t(key: Key) -> &'static str {
    let catalogs = catalogs();
    // A key no catalog defines is a typo at the call site rather than a message, so the debug
    // build fails loudly instead of showing `prompt.ready.titel` in a dialog.
    debug_assert!(
        catalogs.values().any(|table| table.contains_key(key)),
        "{key} is in no catalog"
    );
    let found = |code: &str| catalogs.get(code).and_then(|table| table.get(key));
    // A half-translated catalog still answers in English rather than in raw keys.
    let text = found(language()).or_else(|| found(EN));
    text.copied().unwrap_or(key)
}

fn language() -> &'static str {
    LANGUAGE.get_or_init(select)
}

fn catalogs() -> &'static HashMap<&'static str, Table> {
    CATALOGS.get_or_init(|| [(ZH, table(ZH_ENTRIES)), (EN, table(EN_ENTRIES))].into())
}

fn table(entries: &[(Key, &'static str)]) -> Table {
    entries.iter().copied().collect()
}

fn select() -> &'static str {
    resolve(&detect::preferred(), detect::installed_ui_language())
}

/// The first preference with a catalog, English otherwise.
///
/// The installed UI language only speaks up when the preference list itself is missing: a
/// reader who picked a language this program cannot show should get English, not the language
/// their operating system happened to be installed with.
fn resolve(preferred: &[String], installed: u16) -> &'static str {
    if preferred.is_empty() {
        return code_of_langid(installed).unwrap_or(EN);
    }
    preferred
        .iter()
        .find_map(|tag| code_of_tag(tag))
        .unwrap_or(EN)
}

/// BCP-47 display-language tags, as this API reports them by default.
fn code_of_tag(tag: &str) -> Option<&'static str> {
    let mut parts = tag.split('-').map(|part| part.to_ascii_lowercase());
    match parts.next().as_deref() {
        Some("en") => Some(EN),
        Some("zh") => {
            // A tag without a Simplified marker is either Traditional (zh-Hant, zh-TW,
            // zh-HK, zh-MO) or unspecified, and none of them has a catalog here.
            let simplified = ["hans", "cn", "sg"];
            parts
                .any(|part| simplified.contains(&part.as_str()))
                .then_some(ZH)
        }
        _ => None,
    }
}

fn code_of_langid(langid: u16) -> Option<&'static str> {
    const LANG_CHINESE: u16 = 0x04;
    const LANG_ENGLISH: u16 = 0x09;
    const SUBLANG_CHINESE_SIMPLIFIED: u16 = 0x02;
    const SUBLANG_CHINESE_SINGAPORE: u16 = 0x04;

    match langid & 0x3FF {
        LANG_CHINESE
            if matches!(
                langid >> 10,
                SUBLANG_CHINESE_SIMPLIFIED | SUBLANG_CHINESE_SINGAPORE
            ) =>
        {
            Some(ZH)
        }
        LANG_ENGLISH => Some(EN),
        _ => None,
    }
}

mod detect {
    use std::ptr;

    /// Ask for BCP-47 tags. With MUI_LANGUAGE_ID the buffer holds four-character strings
    /// such as "0804" instead of numbers, which would need parsing before mapping.
    const MUI_LANGUAGE_NAME: u32 = 0x8;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserPreferredUILanguages(
            flags: u32,
            languages: *mut u32,
            buffer: *mut u16,
            characters: *mut u32,
        ) -> i32;
        fn GetUserDefaultUILanguage() -> u16;
    }

    /// The user's ordered display-language tags. Empty when the API is unavailable.
    pub(super) fn preferred() -> Vec<String> {
        let mut languages = 0;
        let mut characters = 0;
        // SAFETY: A null buffer is the documented way to ask for the required size, both
        // counts are live exclusively borrowed locals, and the return value is checked.
        if unsafe {
            GetUserPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &mut languages,
                ptr::null_mut(),
                &mut characters,
            )
        } == 0
        {
            return Vec::new();
        }
        let mut buffer = vec![0u16; characters as usize];
        // SAFETY: `characters` still holds the buffer's capacity in characters, which the API
        // bounds its writes by; the count it reports back is clamped before anything reads it.
        if unsafe {
            GetUserPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &mut languages,
                buffer.as_mut_ptr(),
                &mut characters,
            )
        } == 0
        {
            return Vec::new();
        }
        let written = (characters as usize).min(buffer.len());
        split_tags(&buffer[..written])
    }

    /// The language the operating system was installed with, as a single LANGID.
    pub(super) fn installed_ui_language() -> u16 {
        // SAFETY: This Win32 query takes no pointers and has no preconditions.
        unsafe { GetUserDefaultUILanguage() }
    }

    fn split_tags(buffer: &[u16]) -> Vec<String> {
        buffer
            .split(|unit| *unit == 0)
            .filter(|tag| !tag.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}

const ZH_ENTRIES: &[(Key, &str)] = &[
    ("help.title", "Lertaro 诊断包收集工具"),
    (
        "help.summary",
        "把 Lertaro 崩溃、闪退或功能异常后排查所需的资料一次性收集到一个 ZIP，交给开发者分析。\n\
         全程在本机完成：程序不联网、不上传任何数据。",
    ),
    (
        "help.usage",
        "用法：\n  get_dump_full.exe                直接运行，按提示完成采集与打包\n  \
         get_dump_full.exe --zip <路径>   指定诊断包落点：以 .zip 结尾按该名字写，否则当作目录\n  \
         get_dump_full.exe --no-dump      不抓内存转储：结果不对、界面异常这类问题用这个\n  \
         get_dump_full.exe --ranking      排序问题专用：不查崩溃证据也不抓转储，弹窗里的那一问省了",
    ),
    (
        "help.location",
        "不带 --zip 时，诊断包以 <时间戳>-<进程号>.zip 写在当前工作目录；双击运行时，这个目录就是程序所在目录。",
    ),
    ("help.repository", "源码与使用说明：{url}"),
    ("status.line", "{label}（{elapsed}）"),
    ("status.processes", "正在检查 Lertaro 进程"),
    ("status.archive", "正在压缩诊断文件"),
    ("status.system", "正在采集系统信息"),
    ("status.permissions", "正在读取目录权限"),
    ("status.account", "正在检查账户权限"),
    ("status.dotnet", "正在检查 .NET 环境"),
    ("status.logs", "正在复制应用日志"),
    ("status.manifest", "正在生成诊断清单"),
    ("status.events", "正在查询事件日志"),
    ("status.wer", "正在收集崩溃报告"),
    ("status.crashdumps", "正在收集崩溃转储"),
    ("status.dump", "正在转储 {process}"),
    ("run.done", "抓取与打包已完成。"),
    ("run.bundle", "诊断包已生成：{path}"),
    ("run.cancelled", "已取消，本次没有收集任何文件。"),
    ("prompt.ready.title", "Lertaro 诊断 · 准备确认"),
    (
        "prompt.ready.body",
        "你是否已准备完毕，当前复现了 bug？\n\
         如果是搜索相关的 bug，请将复现问题的 Lertaro 窗口置顶（鼠标中键点击搜索框的 Lertaro 图标）。\n\
         点“确定”开始抓取，点“取消”结束本次运行。",
    ),
    ("prompt.ranking.title", "Lertaro 诊断 · 问题类型"),
    (
        "prompt.ranking.body",
        "这次的问题是不是搜索结果排序不对（该靠前的排到了后面、类型顺序不对、结果少了一条）？\n\
         选“是”只收排运用得上的东西：设置、排名校准记录、应用日志和系统信息，不查崩溃事件，\
         也不抓内存转储，几秒钟就完。\n\
         选“否”按崩溃流程继续，之后会再问一次转储类型。",
    ),
    ("prompt.memory.title", "Lertaro 诊断 · 转储类型"),
    (
        "prompt.memory.body",
        "本次问题是否与内存有关（卡住、占用暴涨、图标资源故障）？\n\
         选“是”抓取包含堆内存的完整转储，包体积大约大一个数量级；选“否”只抓迷你转储。",
    ),
    ("prompt.full_dump.title", "Lertaro 诊断 · 即将抓取完整转储"),
    (
        "prompt.full_dump.body",
        "接下来抓取的是包含堆内存的完整转储。\n\
         抓取过程中电脑可能卡顿 3-5 分钟，请耐心等待，期间尽量不要操作鼠标和键盘。",
    ),
    ("prompt.upload.title", "Lertaro 诊断 · 上传引导"),
    (
        "prompt.upload.body",
        "是否需要现在打开上传站点，把这个文件交给开发者？\n诊断包：{path}",
    ),
    ("prompt.uploaded.title", "Lertaro 诊断 · 上传站点"),
    (
        "prompt.uploaded.body",
        "已打开 {site}：把诊断包拖进页面上传，再把页面给出的一次性传输票据发给开发者。\n\
         开发者接收完成之前请不要关闭这个浏览器窗口。该方式不限速、无需登录。\n\
         诊断包位置：{path}",
    ),
    (
        "prompt.browser.body",
        "打开浏览器失败（{error}），请手动访问 {site}\n诊断包位置：{path}",
    ),
    (
        "prompt.drive",
        "请不要使用百度网盘等限速网盘传输诊断包：下载会被限速，开发者往往拿不到完整文件。",
    ),
    ("prompt.finish.title", "Lertaro 诊断 · 已结束"),
    ("prompt.finish.body", "按“确定”关闭这个窗口。"),
];

const EN_ENTRIES: &[(Key, &str)] = &[
    ("help.title", "Lertaro diagnostic bundle collector"),
    (
        "help.summary",
        "Collects everything needed to diagnose a Lertaro crash, sudden exit or malfunction into \
         one ZIP for the developer.\nEverything runs on this machine: the program never uploads anything.",
    ),
    (
        "help.usage",
        "Usage:\n  get_dump_full.exe                run it and follow the prompts\n  \
         get_dump_full.exe --zip <path>   where the bundle goes: a name ending in .zip, or a folder\n  \
         get_dump_full.exe --no-dump      skip the memory dump: for wrong results or UI issues\n  \
         get_dump_full.exe --ranking      a ranking report: no crash evidence, no dump, and no extra question",
    ),
    (
        "help.location",
        "Without --zip the bundle is written to <timestamp>-<pid>.zip in the working directory, \
         which for a double-clicked run is the program folder.",
    ),
    ("help.repository", "Source and documentation: {url}"),
    ("status.line", "{label} ({elapsed})"),
    ("status.processes", "Checking Lertaro processes"),
    ("status.archive", "Compressing diagnostic files"),
    ("status.system", "Collecting system information"),
    ("status.permissions", "Reading directory permissions"),
    ("status.account", "Checking account privileges"),
    ("status.dotnet", "Checking the .NET environment"),
    ("status.logs", "Copying application logs"),
    ("status.manifest", "Writing the bundle manifest"),
    ("status.events", "Querying event logs"),
    ("status.wer", "Collecting crash reports"),
    ("status.crashdumps", "Collecting crash dumps"),
    ("status.dump", "Dumping {process}"),
    ("run.done", "Collection and packaging finished."),
    ("run.bundle", "Diagnostic bundle: {path}"),
    ("run.cancelled", "Cancelled. Nothing was collected."),
    ("prompt.ready.title", "Lertaro Diagnostics · Ready check"),
    (
        "prompt.ready.body",
        "Are you ready - is the bug reproduced on screen?\n\
         For a search related bug, bring the Lertaro window showing the problem to the front \
         (middle-click the Lertaro icon in the search box).\n\
         Choose OK to start the capture, or Cancel to end this run.",
    ),
    ("prompt.ranking.title", "Lertaro Diagnostics · Problem type"),
    (
        "prompt.ranking.body",
        "Is this a wrong search result order - a row that belongs higher sits lower, the type order \
         is wrong, or a result is missing?\n\
         Yes collects only what a ranking report uses: the settings, the ranking calibration log, the \
         application logs and the system information. No crash evidence and no memory dump, done in \
         seconds.\n\
         No continues as a failure report and asks about the dump type next.",
    ),
    ("prompt.memory.title", "Lertaro Diagnostics · Dump type"),
    (
        "prompt.memory.body",
        "Is this problem memory related (Freezing, High resource usage, Icon failure)?\n\
         Yes captures a full dump including the heap, roughly an order of magnitude larger; \
         No captures a mini dump.",
    ),
    (
        "prompt.full_dump.title",
        "Lertaro Diagnostics · Full dump next",
    ),
    (
        "prompt.full_dump.body",
        "The next capture is a full dump including the heap.\n\
         The computer may stutter for 3-5 minutes while it runs. Please wait, and leave the \
         mouse and keyboard alone meanwhile.",
    ),
    ("prompt.upload.title", "Lertaro Diagnostics · Upload"),
    (
        "prompt.upload.body",
        "Open the upload site now so you can hand this file to the developer?\nBundle: {path}",
    ),
    ("prompt.uploaded.title", "Lertaro Diagnostics · Upload site"),
    (
        "prompt.uploaded.body",
        "{site} is open: drag the bundle into the page to upload it, then send the developer the \
         one-time transfer ticket the page shows.\nKeep that browser window open until the developer \
         has finished receiving it. The transfer is not rate limited and needs no sign-in.\n\
         Bundle location: {path}",
    ),
    (
        "prompt.browser.body",
        "The browser could not be opened ({error}); please visit {site} manually.\n\
         Bundle location: {path}",
    ),
    (
        "prompt.drive",
        "Do not send the bundle through a rate limited cloud drive such as Baidu Netdisk: the \
         download is throttled and the developer often cannot get the complete file.",
    ),
    ("prompt.finish.title", "Lertaro Diagnostics · Finished"),
    ("prompt.finish.body", "Choose OK to close this window."),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_catalogs_cover_the_same_keys() {
        let zh = table(ZH_ENTRIES);
        let en = table(EN_ENTRIES);
        assert_eq!(zh.len(), en.len());
        for key in zh.keys() {
            assert!(en.contains_key(key), "en is missing {key}");
        }
        assert!(
            zh.values().chain(en.values()).all(|text| !text.is_empty()),
            "an empty message would print as a blank line"
        );
    }

    #[test]
    fn placeholders_match_between_catalogs() {
        let zh = table(ZH_ENTRIES);
        let en = table(EN_ENTRIES);
        for key in zh.keys() {
            let (found, expected) = (sorted_markers(zh[key]), sorted_markers(en[key]));
            assert_eq!(found, expected, "placeholders differ for {key}");
        }
    }

    #[test]
    fn tr_fills_every_marker() {
        let rendered = tr(
            "prompt.browser.body",
            &[
                ("error", &"os error 5"),
                ("site", &"example.site"),
                ("path", &"D:\\a.zip"),
            ],
        );
        assert!(!rendered.contains('{'), "{rendered} still holds a marker");
        for value in ["os error 5", "example.site", "D:\\a.zip"] {
            assert!(rendered.contains(value), "{value} was not substituted");
        }
    }

    /// A key in no catalog is a typo at the call site. The debug build panics on it so the
    /// mistake cannot reach the screen as raw text, and the release build still degrades to
    /// showing the key.
    #[test]
    #[should_panic(expected = "is in no catalog")]
    fn an_unknown_key_is_a_programmer_error() {
        t("prompt.ready.titel");
    }

    #[test]
    fn tags_select_only_the_shipped_catalogs() {
        assert_eq!(code_of_tag("zh-CN"), Some(ZH));
        assert_eq!(code_of_tag("zh-Hans"), Some(ZH));
        assert_eq!(code_of_tag("zh-Hans-CN"), Some(ZH));
        assert_eq!(code_of_tag("zh-SG"), Some(ZH));
        assert_eq!(code_of_tag("zh-TW"), None);
        assert_eq!(code_of_tag("zh-Hant-HK"), None);
        assert_eq!(code_of_tag("zh"), None);
        assert_eq!(code_of_tag("en-US"), Some(EN));
        assert_eq!(code_of_tag("en-GB"), Some(EN));
        assert_eq!(code_of_tag("ja-JP"), None);
        assert_eq!(code_of_tag("fr-CA"), None);
    }

    #[test]
    fn langids_select_only_the_shipped_catalogs() {
        assert_eq!(code_of_langid(0x0804), Some(ZH)); // zh-CN
        assert_eq!(code_of_langid(0x1004), Some(ZH)); // zh-SG
        assert_eq!(code_of_langid(0x0404), None); // zh-TW
        assert_eq!(code_of_langid(0x0C04), None); // zh-HK
        assert_eq!(code_of_langid(0x0409), Some(EN)); // en-US
        assert_eq!(code_of_langid(0x0809), Some(EN)); // en-GB
        assert_eq!(code_of_langid(0x0411), None); // ja-JP
    }

    #[test]
    fn the_installed_language_only_answers_for_a_missing_list() {
        let list = |tags: &[&str]| tags.iter().map(|tag| (*tag).to_owned()).collect::<Vec<_>>();

        // zh-CN installed, but the reader picked Japanese: English, not Chinese.
        assert_eq!(resolve(&list(&["ja-JP"]), 0x0804), EN);
        assert_eq!(resolve(&list(&["zh-TW", "en-US"]), 0x0804), EN);
        assert_eq!(resolve(&list(&["ja-JP", "zh-CN"]), 0x0409), ZH);
        assert_eq!(resolve(&list(&["zh-CN"]), 0x0409), ZH);

        // No list at all: the installed UI language is the only evidence left.
        assert_eq!(resolve(&[], 0x0804), ZH);
        assert_eq!(resolve(&[], 0x0409), EN);
        assert_eq!(resolve(&[], 0x0411), EN);
    }

    #[test]
    fn the_detected_language_always_has_a_catalog() {
        assert!(catalogs().contains_key(language()));
    }

    #[test]
    fn the_detected_language_comes_from_winapi() {
        // A wrong FFI signature dies here rather than in the release build, so the call
        // itself is part of what this test covers.
        let tags = detect::preferred();
        assert!(
            tags.iter()
                .all(|tag| !tag.is_empty() && !tag.contains('\0')),
            "{tags:?} is not a clean tag list"
        );
        assert!(
            !tags.is_empty(),
            "the display-language list came back empty"
        );
    }

    fn sorted_markers(text: &str) -> Vec<&str> {
        let mut markers: Vec<_> = text
            .split('{')
            .skip(1)
            .filter_map(|part| part.split('}').next())
            .collect();
        markers.sort();
        markers
    }
}
