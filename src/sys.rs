//! What macOS has instead of Linux's /proc and /etc/passwd: who runs nami, who a uid is, when a
//! process started. (On Linux nami reads those files, as it always has: profile.rs.)
//! A Mac's people aren't in /etc/passwd (Directory Services has them): getpwuid finds them.

#![cfg_attr(target_os = "linux", allow(dead_code))]

use std::ffi::{CStr, CString};
use std::path::PathBuf;

/// Someone with an account here.
pub struct Account {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    #[allow(dead_code)]
    pub home: PathBuf,
}

/// The effective uid.
pub fn euid() -> u32 {
    // SAFETY: no arguments, always succeeds.
    unsafe { libc::geteuid() }
}

fn account_of(pw: &libc::passwd) -> Option<Account> {
    // SAFETY: getpw*_r filled these in, pointing into our buffer.
    let name = unsafe { CStr::from_ptr(pw.pw_name) }.to_str().ok()?.to_string();
    let home = if pw.pw_dir.is_null() { PathBuf::new() } else { PathBuf::from(unsafe { CStr::from_ptr(pw.pw_dir) }.to_string_lossy().into_owned()) };
    Some(Account { name, uid: pw.pw_uid, gid: pw.pw_gid, home })
}

/// The account with uid `uid`.
pub fn account_by_uid(uid: u32) -> Option<Account> {
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 16384];
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: the buffer and struct outlive the call; `out` is null or `&mut pw`.
    let rc = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc != 0 || out.is_null() {
        return None;
    }
    account_of(&pw)
}

/// The account called `login`.
pub fn account_by_name(login: &str) -> Option<Account> {
    let c = CString::new(login).ok()?;
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 16384];
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: as above.
    let rc = unsafe { libc::getpwnam_r(c.as_ptr(), &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc != 0 || out.is_null() {
        return None;
    }
    account_of(&pw)
}

/// When process `pid` started (seconds and microseconds since 1970), so a pid used again
/// isn't taken for it. None: no such process.
#[cfg(target_os = "macos")]
pub fn process_start(pid: u32) -> Option<String> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: proc_pidinfo writes at most `size` bytes into `info`.
    let n = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
    (n == size).then(|| format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec))
}

#[cfg(not(target_os = "macos"))]
pub fn process_start(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // (After the name, which is in brackets and may have anything in it.)
    stat.rsplit_once(')')?.1.split_whitespace().nth(19).map(String::from)
}

/// This computer's name.
pub fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most buf.len() bytes.
    if unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    // A Mac says "ada-mbp.local": the computer's name is the first part.
    let name = name.strip_suffix(".local").map(str::to_string).unwrap_or(name);
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_root_and_this_process() {
        // (Accounts are a Mac's question: Linux reads /etc/passwd, which a build's sandbox may not have.)
        if cfg!(target_os = "macos") {
            let root = super::account_by_uid(0).expect("root");
            assert_eq!(root.name, "root");
            assert_eq!(super::account_by_name("root").map(|a| a.uid), Some(0));
            assert!(super::account_by_name("no-such-person-here").is_none());
        }
        let me = super::process_start(std::process::id()).expect("this process");
        assert_eq!(super::process_start(std::process::id()).as_deref(), Some(me.as_str()));
        assert!(super::hostname().is_some());
    }
}
