//! The local time, in the time zone the computer's in now: changed a moment ago, every clock
//! follows at once. (localtime_r goes on with the zone it first read; tzset looks at
//! /etc/localtime again, a stat when it's the same.)

/// That moment, here.
pub fn local(at: i64) -> libc::tm {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        tzset();
        libc::localtime_r(&at, &mut tm);
    }
    tm
}

/// The moment it is now, here.
pub fn local_now() -> libc::tm {
    local(unsafe { libc::time(std::ptr::null_mut()) })
}

unsafe extern "C" {
    fn tzset();
}
