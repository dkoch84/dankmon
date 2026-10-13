//! NFS client statistics from /proc/self/mountstats.
//!
//! Reading this file never touches the server, so it stays responsive even
//! when a mount is hung.

use std::collections::HashMap;
use std::fs;

/// Per-operation counters, as the kernel reports them.
#[derive(Clone, Copy, Default)]
struct Op {
    ops: u64,
    trans: u64,
    timeouts: u64,
    rtt_ms: u64,
    exec_ms: u64,
    errors: u64,
}

#[derive(Clone, Default)]
struct Raw {
    /// normal read, normal write, direct read, direct write,
    /// server read, server write, pages read, pages written
    bytes: [u64; 8],
    ops: HashMap<String, Op>,
}

pub struct Mount {
    pub device: String,
    pub mountpoint: String,
    pub vers: String,
    pub proto: String,
    pub connections: usize,
    pub age_secs: u64,
    /// Bytes per second actually moved over the wire.
    pub wire_read: f64,
    pub wire_write: f64,
    /// Bytes per second applications asked for (read()/write()).
    pub app_read: f64,
    pub app_write: f64,
    pub ops_rate: f64,
    /// Average round trip and total execution time per op in this interval.
    pub rtt_ms: Option<f64>,
    pub exec_ms: Option<f64>,
    pub read_exec_ms: Option<f64>,
    pub write_exec_ms: Option<f64>,
    pub retrans: u64,
    pub timeouts: u64,
    pub errors: u64,
    /// Busiest operations this interval, as (name, ops per second).
    pub top_ops: Vec<(String, f64)>,
    pub total_read: u64,
    pub total_write: u64,
}

#[derive(Default)]
pub struct Nfs {
    prev: HashMap<String, Raw>,
}

fn unescape(s: &str) -> String {
    s.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn opt<'a>(opts: &'a str, key: &str) -> Option<&'a str> {
    opts.split(',')
        .find_map(|o| o.strip_prefix(key)?.strip_prefix('='))
}

struct Parsed {
    device: String,
    mountpoint: String,
    opts: String,
    age: u64,
    xprts: usize,
    raw: Raw,
}

fn parse(text: &str) -> Vec<Parsed> {
    let mut out: Vec<Parsed> = Vec::new();
    let mut in_ops = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("device ") {
            in_ops = false;
            let Some((dev, rest)) = rest.split_once(" mounted on ") else {
                continue;
            };
            let Some((mp, rest)) = rest.split_once(" with fstype ") else {
                continue;
            };
            let fstype = rest.split_whitespace().next().unwrap_or("");
            if fstype == "nfs" || fstype == "nfs4" {
                out.push(Parsed {
                    device: unescape(dev),
                    mountpoint: unescape(mp),
                    opts: String::new(),
                    age: 0,
                    xprts: 0,
                    raw: Raw::default(),
                });
            } else {
                // Mark that we are inside a non-NFS block.
                out.push(Parsed {
                    device: String::new(),
                    mountpoint: String::new(),
                    opts: String::new(),
                    age: 0,
                    xprts: 0,
                    raw: Raw::default(),
                });
            }
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        if cur.device.is_empty() {
            continue;
        }
        let t = line.trim();
        if let Some(v) = t.strip_prefix("opts:") {
            cur.opts = v.trim().to_string();
        } else if let Some(v) = t.strip_prefix("age:") {
            cur.age = v.trim().parse().unwrap_or(0);
        } else if t.starts_with("xprt:") {
            cur.xprts += 1;
        } else if let Some(v) = t.strip_prefix("bytes:") {
            for (i, n) in v.split_whitespace().take(8).enumerate() {
                cur.raw.bytes[i] = n.parse().unwrap_or(0);
            }
        } else if t == "per-op statistics" {
            in_ops = true;
        } else if in_ops {
            let Some((name, vals)) = t.split_once(':') else {
                continue;
            };
            let f: Vec<u64> = vals
                .split_whitespace()
                .map(|v| v.parse().unwrap_or(0))
                .collect();
            let g = |i: usize| f.get(i).copied().unwrap_or(0);
            cur.raw.ops.insert(
                name.to_string(),
                Op {
                    ops: g(0),
                    trans: g(1),
                    timeouts: g(2),
                    rtt_ms: g(6),
                    exec_ms: g(7),
                    errors: g(8),
                },
            );
        }
    }
    out.retain(|p| !p.device.is_empty());
    out
}

impl Nfs {
    pub fn sample(&mut self, dt: f64) -> Vec<Mount> {
        let text = fs::read_to_string("/proc/self/mountstats").unwrap_or_default();
        let parsed = parse(&text);
        let mut next = HashMap::new();
        let mut out = Vec::new();
        for p in parsed {
            let key = format!("{}\0{}", p.device, p.mountpoint);
            let prev = self
                .prev
                .get(&key)
                .cloned()
                .unwrap_or_else(|| p.raw.clone());
            let rate = |d: u64| if dt > 0.0 { d as f64 / dt } else { 0.0 };
            let db = |i: usize| p.raw.bytes[i].saturating_sub(prev.bytes[i]);

            let mut d_ops = 0u64;
            let mut d_rtt = 0u64;
            let mut d_exec = 0u64;
            let mut retrans = 0u64;
            let mut timeouts = 0u64;
            let mut errors = 0u64;
            let mut per_op: Vec<(String, u64)> = Vec::new();
            let op_exec = |name: &str| -> Option<f64> {
                let (now, before) = (
                    p.raw.ops.get(name)?,
                    prev.ops.get(name).copied().unwrap_or_default(),
                );
                let n = now.ops.saturating_sub(before.ops);
                (n > 0).then(|| now.exec_ms.saturating_sub(before.exec_ms) as f64 / n as f64)
            };
            let read_exec_ms = op_exec("READ");
            let write_exec_ms = op_exec("WRITE");
            for (name, now) in &p.raw.ops {
                let before = prev.ops.get(name).copied().unwrap_or_default();
                let n = now.ops.saturating_sub(before.ops);
                d_ops += n;
                d_rtt += now.rtt_ms.saturating_sub(before.rtt_ms);
                d_exec += now.exec_ms.saturating_sub(before.exec_ms);
                let tr = now.trans.saturating_sub(before.trans);
                retrans += tr.saturating_sub(n);
                timeouts += now.timeouts.saturating_sub(before.timeouts);
                errors += now.errors.saturating_sub(before.errors);
                if n > 0 {
                    per_op.push((name.clone(), n));
                }
            }
            per_op.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let avg = |total: u64| (d_ops > 0).then(|| total as f64 / d_ops as f64);

            out.push(Mount {
                device: p.device.clone(),
                mountpoint: p.mountpoint.clone(),
                vers: opt(&p.opts, "vers").unwrap_or("?").to_string(),
                proto: opt(&p.opts, "proto").unwrap_or("?").to_string(),
                connections: p.xprts,
                age_secs: p.age,
                wire_read: rate(db(4)),
                wire_write: rate(db(5)),
                app_read: rate(db(0) + db(2)),
                app_write: rate(db(1) + db(3)),
                ops_rate: rate(d_ops),
                rtt_ms: avg(d_rtt),
                exec_ms: avg(d_exec),
                read_exec_ms,
                write_exec_ms,
                retrans,
                timeouts,
                errors,
                top_ops: per_op
                    .into_iter()
                    .take(4)
                    .map(|(n, c)| (n, rate(c)))
                    .collect(),
                total_read: p.raw.bytes[4],
                total_write: p.raw.bytes[5],
            });
            next.insert(key, p.raw);
        }
        self.prev = next;
        out
    }
}
