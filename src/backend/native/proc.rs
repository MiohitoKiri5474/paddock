//! What the operating system says of a process (its name and directory),
//! and a random number: the native server's platform seam. macOS asks
//! `libproc`, Linux reads `/proc`.

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("paddock's native server runs on macOS and Linux only");

/// Process `pid`'s name: its executable's, as macOS's `ps -c` shows it.
#[cfg(target_os = "macos")]
pub fn name(pid: i32) -> Option<String> {
    let mut buf = [0u8; 256];
    let size = u32::try_from(buf.len()).ok()?;
    // SAFETY: proc_name writes at most `size` bytes to the buffer.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), size) };
    let n = usize::try_from(n).ok().filter(|&n| n > 0)?;
    Some(String::from_utf8_lossy(&buf[..n]).into_owned())
}

/// Process `pid`'s current directory.
#[cfg(target_os = "macos")]
pub fn cwd(pid: i32) -> Option<String> {
    use std::ffi::CStr;
    // SAFETY: an all-zero proc_vnodepathinfo is a valid value.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = i32::try_from(std::mem::size_of::<libc::proc_vnodepathinfo>()).ok()?;
    // SAFETY: proc_pidinfo writes at most `size` bytes to `info`.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            std::ptr::from_mut(&mut info).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // SAFETY: the kernel writes the path NUL-terminated within the array.
    let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
    Some(path.to_string_lossy().into_owned()).filter(|p| !p.is_empty())
}

/// A random number.
#[cfg(target_os = "macos")]
pub fn random() -> u32 {
    // SAFETY: arc4random has no preconditions.
    unsafe { libc::arc4random() }
}

/// Process `pid`'s name: its executable's (`comm`, as `ps -o comm`
/// shows it), cut to 15 bytes by the kernel.
#[cfg(target_os = "linux")]
pub fn name(pid: i32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim_end_matches('\n').to_string()).filter(|n| !n.is_empty())
}

/// Process `pid`'s current directory (`None` once it is deleted).
#[cfg(target_os = "linux")]
pub fn cwd(pid: i32) -> Option<String> {
    let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    let path = path.to_string_lossy().into_owned();
    (!path.is_empty() && !path.ends_with(" (deleted)")).then_some(path)
}

/// A random number.
#[cfg(target_os = "linux")]
pub fn random() -> u32 {
    let mut buf = [0u8; 4];
    // SAFETY: getrandom writes at most `buf.len()` bytes to the buffer.
    let n = unsafe { libc::getrandom(buf.as_mut_ptr().cast(), buf.len(), 0) };
    if usize::try_from(n).ok() != Some(buf.len()) {
        // Never short for 4 bytes once the pool is ready; fall back to
        // the clock rather than fail a server's start.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        return now.subsec_nanos() ^ std::process::id();
    }
    u32::from_ne_bytes(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> i32 {
        i32::try_from(std::process::id()).unwrap()
    }

    #[test]
    fn a_process_directory_is_where_it_runs() {
        let here = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        let read = cwd(me()).expect("own directory");
        assert_eq!(std::fs::canonicalize(read).unwrap(), here);
    }

    #[test]
    fn a_process_is_named_after_its_executable() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        let read = name(pid);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(read.as_deref(), Some("sleep"));
    }

    #[test]
    fn a_process_that_is_gone_has_neither() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        child.wait().unwrap();
        assert_eq!(name(pid), None);
        assert_eq!(cwd(pid), None);
    }

    #[test]
    fn random_numbers_differ() {
        let draws: std::collections::HashSet<u32> = (0..8).map(|_| random()).collect();
        assert!(draws.len() > 1);
    }
}
