//! CPU temperatures from hwmon: k10temp or zenpower on AMD, coretemp on Intel.

use std::fs;
use std::path::{Path, PathBuf};

/// Temperature to colour against when a sensor reports no critical value.
pub const DEFAULT_CRIT: f64 = 95.0;

#[derive(Default, Clone)]
pub struct CpuTemp {
    /// The package (or Tctl/Tdie) reading, in °C.
    pub package: Option<f64>,
    /// Per-chiplet (AMD) or per-core (Intel) readings.
    pub parts: Vec<f64>,
    /// What `parts` are: "ccd" or "core".
    pub part_kind: &'static str,
    pub crit: f64,
}

pub fn read_milli(p: &Path) -> Option<f64> {
    let v: f64 = fs::read_to_string(p).ok()?.trim().parse().ok()?;
    Some(v / 1000.0)
}

/// Each `tempN_label` in a hwmon directory with its `tempN_input` path.
pub fn labelled_temps(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(u32, String, PathBuf)> = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(n) = name
            .strip_prefix("temp")
            .and_then(|s| s.strip_suffix("_input"))
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let label = fs::read_to_string(dir.join(format!("temp{n}_label")))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| format!("temp{n}"));
        out.push((n, label, e.path()));
    }
    out.sort_by_key(|t| t.0);
    out.into_iter().map(|(_, l, p)| (l, p)).collect()
}

pub struct CpuSensors {
    package: Vec<PathBuf>,
    parts: Vec<PathBuf>,
    part_kind: &'static str,
    crit: f64,
}

impl CpuSensors {
    pub fn find() -> Option<Self> {
        let mut s = CpuSensors {
            package: Vec::new(),
            parts: Vec::new(),
            part_kind: "core",
            crit: DEFAULT_CRIT,
        };
        for e in fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
            let dir = e.path();
            let name = fs::read_to_string(dir.join("name")).unwrap_or_default();
            match name.trim() {
                // Tdie is Tctl without the offset some Ryzens add, so prefer it.
                "k10temp" | "zenpower" => {
                    let temps = labelled_temps(&dir);
                    let pick = temps
                        .iter()
                        .find(|t| t.0 == "Tdie")
                        .or_else(|| temps.iter().find(|t| t.0 == "Tctl"));
                    s.package.extend(pick.map(|t| t.1.clone()));
                    s.parts.extend(
                        temps
                            .iter()
                            .filter(|t| t.0.starts_with("Tccd"))
                            .map(|t| t.1.clone()),
                    );
                    s.part_kind = "ccd";
                }
                "coretemp" => {
                    for (label, path) in labelled_temps(&dir) {
                        if label.starts_with("Package") {
                            if let Some(c) = read_milli(&sibling(&path, "crit")) {
                                s.crit = c;
                            }
                            s.package.push(path);
                        } else if label.starts_with("Core") {
                            s.parts.push(path);
                        }
                    }
                }
                _ => {}
            }
        }
        (!s.package.is_empty() || !s.parts.is_empty()).then_some(s)
    }

    pub fn read(&self) -> CpuTemp {
        let max = |ps: &[PathBuf]| ps.iter().filter_map(|p| read_milli(p)).reduce(f64::max);
        CpuTemp {
            // Several packages (multi-socket): show the hottest.
            package: max(&self.package),
            parts: self.parts.iter().filter_map(|p| read_milli(p)).collect(),
            part_kind: self.part_kind,
            crit: self.crit,
        }
    }
}

/// `temp1_input` -> `temp1_<what>`.
pub fn sibling(input: &Path, what: &str) -> PathBuf {
    let name = input.file_name().unwrap_or_default().to_string_lossy();
    input.with_file_name(name.replace("_input", &format!("_{what}")))
}
