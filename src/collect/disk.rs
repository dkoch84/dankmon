//! Filesystem usage, matching what `df` shows.
//!
//! statvfs() on a hard NFS mount blocks for as long as the server is
//! unreachable, so this runs on its own thread and the UI shows the last
//! result it got.

use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Fs {
    pub source: String,
    pub fstype: String,
    pub mountpoint: String,
    pub size: u64,
    pub used: u64,
    pub avail: u64,
    pub inodes: u64,
    pub inodes_used: u64,
}

impl Fs {
    /// Use% as df computes it: used / (used + avail), rounded up.
    pub fn use_pct(&self) -> f64 {
        let denom = self.used + self.avail;
        if denom == 0 {
            0.0
        } else {
            (self.used as f64 * 100.0 / denom as f64).ceil()
        }
    }
    pub fn is_local_disk(&self) -> bool {
        self.source.starts_with("/dev/")
    }
    pub fn is_network(&self) -> bool {
        matches!(
            self.fstype.as_str(),
            "nfs" | "nfs4" | "cifs" | "smb3" | "smbfs"
        ) || self.fstype.starts_with("fuse.sshfs")
    }
}

pub struct Snapshot {
    pub at: Instant,
    pub list: Vec<Fs>,
}

/// Filesystems df leaves out unless given -a.
const DUMMY: &[&str] = &[
    "autofs",
    "proc",
    "sysfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "securityfs",
    "pstore",
    "bpf",
    "mqueue",
    "debugfs",
    "tracefs",
    "configfs",
    "fusectl",
    "hugetlbfs",
    "binfmt_misc",
    "nsfs",
    "rpc_pipefs",
    "selinuxfs",
    "ramfs",
    "fuse.portal",
    "fuse.gvfsd-fuse",
    "rootfs",
];

fn unescape(s: &str) -> String {
    s.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn statvfs(path: &str) -> Option<libc::statvfs> {
    let c = CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0).then_some(st)
}

fn collect() -> Vec<Fs> {
    let text = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mut seen_dev = HashSet::new();
    let mut out: Vec<Fs> = Vec::new();
    for line in text.lines() {
        // id parent major:minor root mountpoint opts [optional...] - fstype source superopts
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let l: Vec<&str> = left.split(' ').collect();
        let r: Vec<&str> = right.split(' ').collect();
        if l.len() < 5 || r.len() < 2 {
            continue;
        }
        let (dev, mountpoint, fstype, source) = (l[2], unescape(l[4]), r[0], unescape(r[1]));
        if DUMMY.contains(&fstype) {
            continue;
        }
        // df shows each device once; a later mount of the same device
        // replaces the earlier one only if its path is shorter.
        if !seen_dev.insert(dev.to_string()) {
            if let Some(i) = out
                .iter()
                .position(|f| f.source == source && f.fstype == fstype)
                && mountpoint.len() < out[i].mountpoint.len()
            {
                out[i].mountpoint = mountpoint;
            }
            continue;
        }
        let Some(st) = statvfs(&mountpoint) else {
            continue;
        };
        if st.f_blocks == 0 {
            continue;
        }
        let bs = st.f_frsize;
        let size = st.f_blocks * bs;
        let free = st.f_bfree * bs;
        out.push(Fs {
            source,
            fstype: fstype.to_string(),
            mountpoint,
            size,
            used: size.saturating_sub(free),
            avail: st.f_bavail * bs,
            inodes: st.f_files,
            inodes_used: st.f_files.saturating_sub(st.f_ffree),
        });
    }
    out
}

pub fn spawn(every: Duration) -> Receiver<Snapshot> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        loop {
            let list = collect();
            if tx
                .send(Snapshot {
                    at: Instant::now(),
                    list,
                })
                .is_err()
            {
                return;
            }
            thread::sleep(every);
        }
    });
    rx
}

/// (used, avail, size) summed once per backing device or export.
pub fn totals<'a>(list: impl Iterator<Item = &'a Fs>) -> (u64, u64, u64) {
    let mut seen = HashSet::new();
    list.filter(|f| seen.insert(&f.source))
        .fold((0, 0, 0), |(u, a, s), f| {
            (u + f.used, a + f.avail, s + f.size)
        })
}
