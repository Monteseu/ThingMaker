//! The Windows side of [`crate`]: profile, grants, loopback and launching.

use std::{ffi::c_void, path::Path, process::Command, ptr::null_mut};

use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, LocalFree, SetHandleInformation},
    Security::{Authorization::ConvertSidToStringSidW, FreeSid, PSID, SECURITY_CAPABILITIES},
    Security::Isolation::{CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName},
    System::{
        Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
        JobObjects::{AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject},
        Threading::{
            CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
        },
    },
    UI::{
        Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
        WindowsAndMessaging::SW_HIDE,
    },
};

use crate::PROFILE_NAME;

/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`.
const ALREADY_EXISTS: i32 = 0x800700B7u32 as i32;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The container's SID, owned and freed on drop.
struct Sid(PSID);

impl Drop for Sid {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { FreeSid(self.0) };
        }
    }
}

fn container_sid() -> Result<Sid, String> {
    let name = wide(PROFILE_NAME);
    let display = wide("ThingMaker Local Models");
    let description = wide("Local models in ThingMaker: reaches only this computer.");
    let mut sid: PSID = null_mut();
    let created = unsafe { CreateAppContainerProfile(name.as_ptr(), display.as_ptr(), description.as_ptr(), std::ptr::null(), 0, &mut sid) };
    if created == 0 {
        return Ok(Sid(sid));
    }
    if created != ALREADY_EXISTS {
        return Err(format!("could not make the AppContainer (0x{created:08X})"));
    }
    let derived = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
    if derived != 0 {
        return Err(format!("could not find the AppContainer (0x{derived:08X})"));
    }
    Ok(Sid(sid))
}

fn sid_string(sid: &Sid) -> Result<String, String> {
    let mut text: *mut u16 = null_mut();
    if unsafe { ConvertSidToStringSidW(sid.0, &mut text) } == 0 {
        return Err(format!("could not read the AppContainer's SID ({})", unsafe { GetLastError() }));
    }
    let mut len = 0usize;
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    unsafe { LocalFree(text as *mut c_void) };
    Ok(value)
}

/// The container's SID as text (`S-1-15-2-…`), making the profile the first time.
pub fn ensure_profile() -> Result<String, String> {
    sid_string(&container_sid()?)
}

/// Lets the container read (and, with `write`, change) `path` and what is
/// under it.
pub fn grant(path: &Path, sid: &str, write: bool) -> Result<(), String> {
    let rights = if write { "(OI)(CI)M" } else { "(OI)(CI)RX" };
    let output = Command::new("icacls")
        .arg(path)
        .args(["/grant", &format!("*{sid}:{rights}"), "/Q"])
        .output()
        .map_err(|error| format!("icacls could not run: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("could not let local agents use {}: {}", path.display(), String::from_utf8_lossy(&output.stderr).trim()))
    }
}

/// Whether the container may reach `127.0.0.1` (Ollama).
pub fn loopback_allowed(sid: &str) -> bool {
    Command::new("CheckNetIsolation.exe")
        .args(["LoopbackExempt", "-s"])
        .output()
        .map(|output| crate::listed_in(&String::from_utf8_lossy(&output.stdout), sid))
        .unwrap_or(false)
}

/// Asks, with Windows' own administrator prompt, to let the container reach
/// `127.0.0.1`. True once it is allowed.
pub fn request_loopback(sid: &str) -> Result<bool, String> {
    let verb = wide("runas");
    let file = wide("CheckNetIsolation.exe");
    let parameters = wide(&format!("LoopbackExempt -a -p={sid}"));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = SW_HIDE;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // Declining the prompt lands here too.
        return Err("Windows did not allow it".into());
    }
    if !info.hProcess.is_null() {
        unsafe {
            WaitForSingleObject(info.hProcess, 60_000);
            CloseHandle(info.hProcess);
        }
    }
    Ok(loopback_allowed(sid))
}

/// Starts `program args…` inside the container with this process's standard
/// handles, waits for it, and returns its exit code. The agent is in a job
/// that ends it when this launcher ends, so stopping a session stops it.
pub fn run_inside(program: &str, args: &[String]) -> Result<u32, String> {
    let sid = container_sid()?;
    let mut capabilities = SECURITY_CAPABILITIES { AppContainerSid: sid.0, Capabilities: null_mut(), CapabilityCount: 0, Reserved: 0 };
    let mut handles: Vec<HANDLE> = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].iter().map(|which| unsafe { GetStdHandle(*which) }).filter(|handle| !handle.is_null()).collect();
    handles.dedup();
    for handle in &handles {
        unsafe { SetHandleInformation(*handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
    }

    let mut size = 0usize;
    unsafe { InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut size) };
    let mut buffer = vec![0u8; size];
    let list = buffer.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    if unsafe { InitializeProcThreadAttributeList(list, 2, 0, &mut size) } == 0 {
        return Err(format!("could not prepare the AppContainer launch ({})", unsafe { GetLastError() }));
    }
    let ok = unsafe {
        UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize, &mut capabilities as *mut _ as *const c_void, std::mem::size_of::<SECURITY_CAPABILITIES>(), null_mut(), std::ptr::null())
    } != 0 && unsafe {
        // Only the standard handles are handed to the agent.
        UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, handles.as_ptr() as *const c_void, std::mem::size_of_val(handles.as_slice()), null_mut(), std::ptr::null())
    } != 0;
    if !ok {
        unsafe { DeleteProcThreadAttributeList(list) };
        return Err(format!("could not prepare the AppContainer launch ({})", unsafe { GetLastError() }));
    }

    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    startup.StartupInfo.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    startup.StartupInfo.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    startup.lpAttributeList = list;

    let mut command = wide(&crate::command_line(program, args));
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            std::ptr::null(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW | CREATE_SUSPENDED,
            std::ptr::null(),
            std::ptr::null(),
            &startup.StartupInfo,
            &mut process,
        )
    };
    let error = unsafe { GetLastError() };
    unsafe { DeleteProcThreadAttributeList(list) };
    if created == 0 {
        return Err(format!("could not start {program} in the AppContainer ({error})"));
    }

    // In the job before it runs a single instruction, so nothing it starts
    // escapes it.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if !job.is_null() {
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits as *const _ as *const c_void, std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
            AssignProcessToJobObject(job, process.hProcess);
        }
    }
    unsafe { ResumeThread(process.hThread) };
    unsafe { WaitForSingleObject(process.hProcess, INFINITE) };
    let mut code = 1u32;
    unsafe {
        GetExitCodeProcess(process.hProcess, &mut code);
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        if !job.is_null() {
            CloseHandle(job);
        }
    }
    Ok(code)
}
