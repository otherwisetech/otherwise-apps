//! What the computer is like: memory pressure, network, processes. libc only, no frameworks
//! (Low Power Mode is read by the macOS shell, `mac::low_power`).
use crate::sched::Pressure;
use std::path::Path;

/// macOS: `kern.memorystatus_vm_pressure_level` (1 normal, 2 warn, 4 critical), what
/// `memory_pressure` prints. Linux: PSI `some avg10` from `/proc/pressure/memory`.
pub fn memory_pressure() -> Pressure {
    #[cfg(target_os = "macos")]
    {
        let mut level: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>();
        // SAFETY: a valid NUL-terminated name and an int-sized out buffer.
        let rc = unsafe { libc::sysctlbyname(c"kern.memorystatus_vm_pressure_level".as_ptr(), (&mut level as *mut libc::c_int).cast(), &mut len, std::ptr::null_mut(), 0) };
        if rc != 0 {
            return Pressure::Normal;
        }
        return match level {
            4.. => Pressure::Critical,
            2 | 3 => Pressure::Warn,
            _ => Pressure::Normal,
        };
    }
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/pressure/memory").unwrap_or_default();
        let avg = s.lines().find(|l| l.starts_with("some")).and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("avg10="))).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        return if avg > 40.0 {
            Pressure::Critical
        } else if avg > 10.0 {
            Pressure::Warn
        } else {
            Pressure::Normal
        };
    }
    #[allow(unreachable_code)]
    Pressure::Normal
}

/// Interface names that are up without meaning the internet is reachable (VPN tunnels, AirDrop,
/// bridges), so they don't count as "online".
fn virtual_iface(name: &str) -> bool {
    ["lo", "utun", "llw", "awdl", "bridge", "anpi", "gif", "stf", "ap", "docker", "veth", "virbr", "tailscale", "zt"].iter().any(|p| name.starts_with(p))
}

/// Is any real interface up with a routable address? Cheap (no packets sent); a captive portal
/// still counts as online, the job then fails and backs off.
pub fn online() -> bool {
    #[cfg(unix)]
    {
        let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: getifaddrs fills a list we free with freeifaddrs.
        if unsafe { libc::getifaddrs(&mut addrs) } != 0 {
            return true;
        }
        let mut found = false;
        let mut cur = addrs;
        while !cur.is_null() {
            // SAFETY: cur walks the list getifaddrs returned.
            let ifa = unsafe { &*cur };
            cur = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
                continue;
            }
            let flags = ifa.ifa_flags as libc::c_int;
            if flags & libc::IFF_UP == 0 || flags & libc::IFF_RUNNING == 0 || flags & libc::IFF_LOOPBACK != 0 {
                continue;
            }
            // SAFETY: a NUL-terminated name from the kernel.
            let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) }.to_string_lossy();
            if virtual_iface(&name) {
                continue;
            }
            // SAFETY: ifa_addr is non-null; sa_family tells the real type.
            let fam = unsafe { (*ifa.ifa_addr).sa_family } as libc::c_int;
            if fam == libc::AF_INET {
                // SAFETY: AF_INET → sockaddr_in.
                let a = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                let ip = std::net::Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr));
                if !ip.is_link_local() && !ip.is_unspecified() {
                    found = true;
                    break;
                }
            } else if fam == libc::AF_INET6 {
                // SAFETY: AF_INET6 → sockaddr_in6.
                let a = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                let ip = std::net::Ipv6Addr::from(a.sin6_addr.s6_addr);
                if (ip.segments()[0] & 0xe000) == 0x2000 {
                    found = true;
                    break;
                }
            }
        }
        // SAFETY: the list from getifaddrs above.
        unsafe { libc::freeifaddrs(addrs) };
        return found;
    }
    #[allow(unreachable_code)]
    true
}

/// Resident memory of a process, in bytes.
pub fn rss(pid: u32) -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        // SAFETY: PROC_PIDTASKINFO fills a proc_taskinfo of `size` bytes.
        let n = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTASKINFO, 0, (&mut info as *mut libc::proc_taskinfo).cast(), size) };
        return (n == size).then_some(info.pti_resident_size);
    }
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        return Some(pages * page);
    }
    #[allow(unreachable_code)]
    {
        let _ = pid;
        None
    }
}

/// Is a process running from this executable (the app's window process)? macOS: every pid's
/// path; Linux: `/proc/*/exe`.
pub fn running(exe: &Path) -> bool {
    let Ok(want) = std::fs::canonicalize(exe) else { return false };
    let me = std::process::id();
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a null buffer asks for the count.
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if n <= 0 {
            return false;
        }
        let mut pids = vec![0 as libc::c_int; n as usize + 32];
        // SAFETY: the buffer holds pids.len() ints.
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int) };
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        for &pid in pids.iter().take(n.max(0) as usize) {
            if pid <= 0 || pid as u32 == me {
                continue;
            }
            // SAFETY: buf has PROC_PIDPATHINFO_MAXSIZE bytes.
            let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
            if len > 0 && Path::new(std::str::from_utf8(&buf[..len as usize]).unwrap_or("")) == want {
                return true;
            }
        }
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(rd) = std::fs::read_dir("/proc") else { return false };
        return rd.flatten().any(|e| e.file_name().to_string_lossy().parse::<u32>().is_ok_and(|p| p != me) && std::fs::read_link(e.path().join("exe")).is_ok_and(|p| p == want));
    }
    #[allow(unreachable_code)]
    {
        let _ = (want, me);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_answer() {
        let _ = memory_pressure();
        let _ = online();
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(rss(std::process::id()).unwrap_or(0) > 0);
        }
        assert!(!running(Path::new("/definitely/not/here")));
        assert!(virtual_iface("utun4") && !virtual_iface("en0"));
    }
}
