use std::ffi::c_void;
use std::io;

use crate::error_log::ErrorLog;
use crate::i18n::{t, tr};
use crate::processes;

/// The single right `NtSuspendProcess` needs on the process handle.
const PROCESS_SUSPEND_RESUME: u32 = 0x0800;
const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
const TOKEN_QUERY: u32 = 0x0001;
const SE_PRIVILEGE_ENABLED: u32 = 0x0000_0002;

/// An ntdll routine that takes one process handle and answers with an NTSTATUS.
type NtProcess = unsafe extern "system" fn(*mut c_void) -> i32;

#[repr(C)]
struct Luid {
    low: u32,
    high: i32,
}

#[repr(C)]
struct LuidAndAttributes {
    luid: Luid,
    attributes: u32,
}

#[repr(C)]
struct TokenPrivileges {
    count: u32,
    privileges: [LuidAndAttributes; 1],
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn GetLastError() -> u32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut c_void;
    fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
    fn AdjustTokenPrivileges(
        token: *mut c_void,
        disable_all: i32,
        requested: *const TokenPrivileges,
        length: u32,
        previous: *mut TokenPrivileges,
        returned: *mut u32,
    ) -> i32;
    fn GetModuleHandleW(module: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const i8) -> *mut c_void;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut Luid) -> i32;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `NtSuspendProcess` and `NtResumeProcess` are exported by ntdll but not by any import
/// library the toolchain ships reliably, so they are looked up at run time. ntdll is mapped
/// into every process, so this needs no loading and leaves no module to free.
fn ntdll(entry: &str) -> io::Result<NtProcess> {
    let module = unsafe { GetModuleHandleW(wide("ntdll.dll").as_ptr()) };
    if module.is_null() {
        return Err(io::Error::last_os_error());
    }
    let name: Vec<i8> = entry.bytes().map(|byte| byte as i8).chain([0]).collect();
    let address = unsafe { GetProcAddress(module, name.as_ptr()) };
    if address.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: GetProcAddress returned a non-null address for an ntdll export whose
    // signature is (HANDLE) -> NTSTATUS, and a function pointer is the same size as the
    // returned address.
    Ok(unsafe { std::mem::transmute::<*mut c_void, NtProcess>(address) })
}

/// One open handle per frozen process: `NtResumeProcess` is called through the very handle
/// that froze it, so a process another caller suspended cannot be released by mistake.
pub(crate) struct Frozen {
    handles: Vec<*mut c_void>,
    targets: usize,
    resume_process: NtProcess,
    refused: Option<String>,
}

/// Freezes every target process so the reproduced state cannot drift while the dialogs are
/// answered and the dumps are written.
pub(crate) fn suspend(list: &[processes::Process], log: &ErrorLog) -> io::Result<Frozen> {
    let suspend_process = ntdll("NtSuspendProcess")?;
    let resume_process = ntdll("NtResumeProcess")?;
    let targets = list.len();
    if list.is_empty() {
        return Ok(Frozen {
            handles: Vec::new(),
            targets,
            resume_process,
            refused: None,
        });
    }
    enable_debug_privilege(log);
    let mut handles = Vec::new();
    let mut refused = None;
    for process in list {
        // One call freezes the whole process, including threads created after this point.
        let handle = unsafe { OpenProcess(PROCESS_SUSPEND_RESUME, 0, process.pid) };
        let failure = if handle.is_null() {
            Some(format!(
                "OpenProcess failed: {}",
                io::Error::last_os_error()
            ))
        } else {
            let status = unsafe { suspend_process(handle) };
            if status == 0 {
                handles.push(handle);
                None
            } else {
                unsafe { CloseHandle(handle) };
                Some(format!("NtSuspendProcess failed: {status:#010x}"))
            }
        };
        if let Some(failure) = failure {
            log.error("FREEZE", process, &failure);
            refused = refused.or(Some(failure));
        }
    }
    Ok(Frozen {
        handles,
        targets,
        resume_process,
        refused,
    })
}

impl Frozen {
    pub(crate) fn has_processes(&self) -> bool {
        !self.handles.is_empty()
    }

    /// Tells the user what stopped, because a frozen window looks exactly like a hung one.
    /// The reason is the one the system actually gave, never a guess.
    pub(crate) fn announce(&self) {
        let unknown = t("freeze.unknown");
        if self.targets == 0 {
            println!("{}", t("freeze.none"));
        } else if self.handles.is_empty() {
            let reason = self.refused.as_deref().unwrap_or(unknown);
            println!("{}", tr("freeze.failed", &[("reason", &reason)]));
        } else {
            let count = self.handles.len();
            println!("{}", tr("freeze.done", &[("count", &count)]));
            if count < self.targets {
                let refused = self.targets - count;
                let reason = self.refused.as_deref().unwrap_or(unknown);
                println!(
                    "{}",
                    tr(
                        "freeze.partial",
                        &[("count", &refused), ("reason", &reason)]
                    )
                );
            }
        }
    }

    /// Releases every process this run froze and reports the first refusal.
    pub(crate) fn resume(&mut self) -> io::Result<()> {
        let mut refused = None;
        for handle in std::mem::take(&mut self.handles) {
            let status = unsafe { (self.resume_process)(handle) };
            if status != 0 && refused.is_none() {
                refused = Some(io::Error::other(format!(
                    "NtResumeProcess failed: {status:#010x}"
                )));
            }
            unsafe { CloseHandle(handle) };
        }
        match refused {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for Frozen {
    fn drop(&mut self) {
        // A failed dump or archive must not leave Lertaro frozen.
        let _ = self.resume();
    }
}

/// Opens the door to the session 0 service process, whose handle an elevated token may only
/// open with the debug privilege — held by default, but disabled.
fn enable_debug_privilege(log: &ErrorLog) {
    let name = wide("SeDebugPrivilege");
    let mut luid = Luid { low: 0, high: 0 };
    if unsafe { LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) } == 0 {
        log.error(
            "FREEZE",
            "LookupPrivilegeValueW",
            io::Error::last_os_error(),
        );
        return;
    }
    let mut token = std::ptr::null_mut();
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    } == 0
    {
        log.error("FREEZE", "OpenProcessToken", io::Error::last_os_error());
        return;
    }
    let requested = TokenPrivileges {
        count: 1,
        privileges: [LuidAndAttributes {
            luid,
            attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    let assigned = unsafe {
        AdjustTokenPrivileges(
            token,
            0,
            &requested,
            std::mem::size_of::<TokenPrivileges>() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    // A token that does not hold the privilege reports ERROR_NOT_ALL_ASSIGNED here rather
    // than failing the call, so read it before any other API can overwrite it.
    let code = unsafe { GetLastError() };
    unsafe { CloseHandle(token) };
    if assigned == 0 {
        log.error(
            "FREEZE",
            "AdjustTokenPrivileges",
            io::Error::from_raw_os_error(code as i32),
        );
    } else if code != 0 {
        log.error(
            "FREEZE",
            "SeDebugPrivilege",
            io::Error::from_raw_os_error(code as i32),
        );
    }
}
