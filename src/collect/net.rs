//! Interface throughput from /proc/net/dev.

use std::collections::HashMap;
use std::fs;

pub struct Iface {
    pub name: String,
    /// Virtual interfaces (bridges, veths, tunnels) carry traffic that also
    /// crosses a physical NIC, so they are left out of the totals.
    pub virt: bool,
    pub rx_rate: f64,
    pub tx_rate: f64,
    pub rx_total: u64,
    pub tx_total: u64,
}

#[derive(Default)]
pub struct Net {
    prev: HashMap<String, (u64, u64)>,
}

fn is_virtual(name: &str) -> bool {
    fs::read_link(format!("/sys/class/net/{name}"))
        .map(|p| p.to_string_lossy().contains("/virtual/"))
        .unwrap_or(true)
}

impl Net {
    pub fn sample(&mut self, dt: f64) -> Vec<Iface> {
        let Ok(text) = fs::read_to_string("/proc/net/dev") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut next = HashMap::new();
        for line in text.lines().skip(2) {
            let Some((name, rest)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if name == "lo" {
                continue;
            }
            let f: Vec<u64> = rest
                .split_whitespace()
                .map(|v| v.parse().unwrap_or(0))
                .collect();
            let (rx, tx) = (
                f.first().copied().unwrap_or(0),
                f.get(8).copied().unwrap_or(0),
            );
            let (rx_rate, tx_rate) = match self.prev.get(name) {
                Some(&(prx, ptx)) if dt > 0.0 => (
                    rx.saturating_sub(prx) as f64 / dt,
                    tx.saturating_sub(ptx) as f64 / dt,
                ),
                _ => (0.0, 0.0),
            };
            next.insert(name.to_string(), (rx, tx));
            out.push(Iface {
                name: name.to_string(),
                virt: is_virtual(name),
                rx_rate,
                tx_rate,
                rx_total: rx,
                tx_total: tx,
            });
        }
        self.prev = next;
        out
    }
}
