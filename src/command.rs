use std::{
    ffi::OsStr,
    io,
    process::{Command, Output},
};

pub(crate) const UTF8: u32 = 65001;

/// Runs a program with the console's output code page and returns its stdout text.
pub(crate) fn text<C: AsRef<OsStr>>(program: C, arguments: &[&str]) -> io::Result<String> {
    text_with(program, arguments, console_code_page())
}

pub(crate) fn text_utf8<C: AsRef<OsStr>>(program: C, arguments: &[&str]) -> io::Result<String> {
    text_with(program, arguments, UTF8)
}

fn text_with<C: AsRef<OsStr>>(
    program: C,
    arguments: &[&str],
    code_page: u32,
) -> io::Result<String> {
    let captured = output(Command::new(program).args(arguments), code_page)?;
    Ok(decode(&captured.stdout, code_page))
}

pub(crate) fn output(command: &mut Command, code_page: u32) -> io::Result<Output> {
    check(command.output()?, code_page)
}

pub(crate) fn check(output: Output, code_page: u32) -> io::Result<Output> {
    if output.status.success() {
        return Ok(output);
    }

    let stderr = decode(&output.stderr, code_page);
    let stdout = decode(&output.stdout, code_page);
    let detail = if stderr.trim().is_empty() {
        &stdout
    } else {
        &stderr
    };
    // Prefer diagnostic lines over version banners and progress messages.
    let errors: Vec<_> = detail
        .lines()
        .map(str::trim)
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            [
                "error",
                "failed",
                "cannot",
                "denied",
                "unable",
                "not found",
                "can't",
            ]
            .iter()
            .any(|word| lower.contains(word))
                || ["错误", "失败", "拒绝", "无法", "找不到"]
                    .iter()
                    .any(|word| line.contains(word))
        })
        .collect();
    let reason = if errors.is_empty() {
        detail
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_owned()
    } else {
        errors.join("; ")
    };
    Err(io::Error::other(format!("{}: {reason}", output.status)))
}

pub(crate) fn decode(bytes: &[u8], code_page: u32) -> String {
    if let Some(bytes) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let (bytes, big_endian) = if let Some(bytes) = bytes.strip_prefix(b"\xFF\xFE") {
        (bytes, false)
    } else if let Some(bytes) = bytes.strip_prefix(b"\xFE\xFF") {
        (bytes, true)
    } else if bytes.len() >= 4 && bytes[1] == 0 && bytes[3] == 0 {
        (bytes, false)
    } else {
        return decode_code_page(bytes, code_page);
    };
    let wide: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            if big_endian {
                u16::from_be_bytes([pair[0], pair[1]])
            } else {
                u16::from_le_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&wide)
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetConsoleOutputCP() -> u32;
    fn MultiByteToWideChar(
        code_page: u32,
        flags: u32,
        input: *const u8,
        input_len: i32,
        output: *mut u16,
        output_len: i32,
    ) -> i32;
}

pub(crate) fn console_code_page() -> u32 {
    #[cfg(windows)]
    {
        // SAFETY: This Win32 query takes no pointers and has no preconditions.
        let code_page = unsafe { GetConsoleOutputCP() };
        if code_page == 0 { 1 } else { code_page } // CP_OEMCP when no console exists.
    }
    #[cfg(not(windows))]
    {
        UTF8
    }
}

pub(crate) fn decode_code_page(bytes: &[u8], code_page: u32) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if code_page == UTF8 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    #[cfg(windows)]
    if let Ok(length) = i32::try_from(bytes.len()) {
        let mut wide = vec![0u16; bytes.len()];
        // SAFETY: Both buffers remain alive, have the stated lengths, and do not
        // overlap. Win32 bounds its writes by output_len; zero indicates failure.
        let written = unsafe {
            MultiByteToWideChar(
                code_page,
                0,
                bytes.as_ptr(),
                length,
                wide.as_mut_ptr(),
                length,
            )
        };
        if written > 0 {
            return String::from_utf16_lossy(&wide[..written as usize]);
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}
