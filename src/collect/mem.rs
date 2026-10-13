//! Memory and swap from /proc/meminfo. All values in bytes.

use std::fs;

#[derive(Default, Clone)]
pub struct Mem {
    pub total: u64,
    pub available: u64,
    pub free: u64,
    pub cached: u64,
    pub buffers: u64,
    pub dirty: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

impl Mem {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
    /// Memory counted as available that currently holds cache the kernel
    /// will drop on demand. used + reclaimable + free = total.
    pub fn reclaimable(&self) -> u64 {
        self.available.saturating_sub(self.free)
    }
    pub fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

pub fn read() -> Mem {
    let mut m = Mem::default();
    let Ok(text) = fs::read_to_string("/proc/meminfo") else {
        return m;
    };
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(key), Some(val)) = (it.next(), it.next()) else {
            continue;
        };
        let kb: u64 = val.parse().unwrap_or(0) * 1024;
        match key {
            "MemTotal:" => m.total = kb,
            "MemAvailable:" => m.available = kb,
            "MemFree:" => m.free = kb,
            "Cached:" => m.cached = kb,
            "Buffers:" => m.buffers = kb,
            "Dirty:" => m.dirty = kb,
            "SwapTotal:" => m.swap_total = kb,
            "SwapFree:" => m.swap_free = kb,
            _ => {}
        }
    }
    m
}
