//! CPU usage from /proc/stat.

use std::fs;

#[derive(Clone, Copy, Default)]
struct Times {
    total: u64,
    idle: u64,
    iowait: u64,
}

#[derive(Default)]
pub struct Cpu {
    prev: Vec<Times>,
}

pub struct CpuSample {
    /// Overall busy percentage.
    pub total: f64,
    /// Share of time spent waiting on I/O (NFS stalls show up here).
    pub iowait: f64,
    /// Busy percentage per logical core.
    pub cores: Vec<f64>,
}

fn read() -> Vec<Times> {
    let Ok(text) = fs::read_to_string("/proc/stat") else {
        return Vec::new();
    };
    text.lines()
        .take_while(|l| l.starts_with("cpu"))
        .map(|l| {
            // user nice system idle iowait irq softirq steal (guest is already in user)
            let f: Vec<u64> = l
                .split_whitespace()
                .skip(1)
                .take(8)
                .map(|v| v.parse().unwrap_or(0))
                .collect();
            let get = |i: usize| f.get(i).copied().unwrap_or(0);
            Times {
                total: f.iter().sum(),
                idle: get(3) + get(4),
                iowait: get(4),
            }
        })
        .collect()
}

impl Cpu {
    pub fn sample(&mut self) -> CpuSample {
        let now = read();
        let pct = |i: usize, pick: fn(&Times) -> u64, busy: bool| -> f64 {
            let (Some(a), Some(b)) = (self.prev.get(i), now.get(i)) else {
                return 0.0;
            };
            let dt = b.total.saturating_sub(a.total);
            if dt == 0 {
                return 0.0;
            }
            let d = pick(b).saturating_sub(pick(a)) as f64 / dt as f64 * 100.0;
            if busy { 100.0 - d } else { d }
        };
        let s = CpuSample {
            total: pct(0, |t| t.idle, true),
            iowait: pct(0, |t| t.iowait, false),
            cores: (1..now.len()).map(|i| pct(i, |t| t.idle, true)).collect(),
        };
        self.prev = now;
        s
    }
}
