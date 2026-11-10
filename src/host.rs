//! Facts about this computer that loaders' requirements and the memory estimate need.

use std::process::Command;

/// The OS version as dotted numbers: "26.2" on macOS, the kernel release on Linux, the build on Windows.
pub fn os_version() -> String {
    let run = |program: &str, args: &[&str]| Command::new(program).args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty());
    if cfg!(target_os = "macos") {
        run("/usr/bin/sw_vers", &["-productVersion"]).unwrap_or_default()
    } else if cfg!(windows) {
        run("cmd", &["/c", "ver"]).and_then(|s| crate::manifest::first_version(&s)).unwrap_or_default()
    } else {
        run("uname", &["-r"]).and_then(|s| crate::manifest::first_version(&s)).unwrap_or_default()
    }
}

/// Installed memory in bytes.
pub fn total_memory() -> Option<u64> {
    if cfg!(target_os = "macos") {
        let out = Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    } else if cfg!(target_os = "linux") {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = text.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    } else {
        None
    }
}

/// What the GPU may keep resident. On Apple Silicon, Metal's recommended working set is about 74% of
/// the unified memory (17.8 GiB of 24 measured); elsewhere 8gent does not know yet.
pub fn gpu_budget(total: Option<u64>) -> Option<u64> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        total.map(|t| (t as f64 * 0.74) as u64)
    } else {
        None
    }
}

/// Memory that can be given to a model right now without pushing the computer into swap: what is
/// free plus what the system holds only as cache.
pub fn available_memory() -> Option<u64> {
    if cfg!(target_os = "macos") {
        let out = Command::new("/usr/bin/vm_stat").output().ok()?;
        parse_vm_stat(&String::from_utf8_lossy(&out.stdout))
    } else if cfg!(target_os = "linux") {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = text.lines().find(|l| l.starts_with("MemAvailable:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    } else {
        None
    }
}

/// `vm_stat`'s free, inactive, speculative and purgeable pages, in bytes.
pub fn parse_vm_stat(text: &str) -> Option<u64> {
    let page: u64 = text.lines().next()?.split("page size of ").nth(1)?.split_whitespace().next()?.parse().ok()?;
    let pages = |name: &str| -> u64 {
        text.lines().find(|l| l.starts_with(name)).and_then(|l| l.rsplit(':').next()).and_then(|v| v.trim().trim_end_matches('.').parse().ok()).unwrap_or(0)
    };
    Some((pages("Pages free") + pages("Pages inactive") + pages("Pages speculative") + pages("Pages purgeable")) * page)
}

#[cfg(test)]
mod tests {
    #[test]
    fn vm_stat_gives_what_is_free_or_only_cache() {
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               18028.\nPages active:                            345146.\nPages inactive:                          352698.\nPages speculative:                          957.\nPages wired down:                        217478.\nPages purgeable:                           1847.\n";
        assert_eq!(super::parse_vm_stat(text), Some((18028 + 352698 + 957 + 1847) * 16384));
        assert_eq!(super::parse_vm_stat("nonsense"), None);
    }
}
