//! Is pid N still the process we started? Checks existence plus the kernel's
//! start time, so a recycled pid doesn't pass for a live run.

const SZOMB: u32 = 5;

/// Deeper than any real process tree; only there so a walk can't go on forever.
const MAX_LINEAGE: usize = 256;

struct Info {
    status: u32,
    start_secs: u64,
    parent: i32,
}

fn info(pid: i32) -> Option<Info> {
    let mut raw: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let written = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut raw).cast(), size) };
    (written == size).then_some(Info {
        status: raw.pbi_status,
        start_secs: raw.pbi_start_tvsec,
        parent: raw.pbi_ppid as i32,
    })
}

/// `pid` and every process above it, nearest first, short of launchd.
pub fn lineage(pid: i32) -> Vec<i32> {
    std::iter::successors(Some(pid), |pid| parent(*pid))
        .take(MAX_LINEAGE)
        .collect()
}

/// The process that started `pid`, unless that's launchd (an orphan).
pub fn parent(pid: i32) -> Option<i32> {
    info(pid).map(|info| info.parent).filter(|parent| *parent > 1)
}

pub fn start_time(pid: i32) -> Option<u64> {
    info(pid).map(|info| info.start_secs)
}

pub fn alive(pid: i32, recorded_start: Option<u64>) -> bool {
    if pid <= 0 {
        return false;
    }
    let exists =
        unsafe { libc::kill(pid, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    match (exists, info(pid), recorded_start) {
        (false, _, _) => false,
        (true, Some(info), _) if info.status == SZOMB => false,
        (true, Some(info), Some(start)) => info.start_secs == start,
        (true, _, _) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_is_alive() {
        let pid = std::process::id() as i32;
        let start = start_time(pid);
        assert!(start.is_some());
        assert!(alive(pid, start));
        assert!(!alive(pid, start.map(|secs| secs + 100)));
    }

    #[test]
    fn lineage_climbs_from_self() {
        let pid = std::process::id() as i32;
        let lineage = lineage(pid);
        assert_eq!(lineage.first(), Some(&pid));
        let parent = unsafe { libc::getppid() };
        assert_eq!(lineage.get(1).copied(), (parent > 1).then_some(parent));
        assert!(!lineage.contains(&1));
    }

    #[test]
    fn bogus_pid_is_dead() {
        assert!(!alive(0, None));
        assert!(!alive(999_999, None));
    }
}
