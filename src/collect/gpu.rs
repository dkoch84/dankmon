//! GPUs from /sys/class/drm (amdgpu, i915, xe, and whatever hwmon offers for
//! others), plus Nvidia through NVML when its library is installed.

use super::nvml::Nvml;
use super::temp::{DEFAULT_CRIT, labelled_temps, read_milli, sibling};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Default)]
pub struct Gpu {
    pub name: String,
    pub driver: String,
    /// Utilisation in percent.
    pub busy: Option<f64>,
    /// True when `busy` is "not in RC6" (Intel) rather than a real busy
    /// counter: the GPU is awake, though not necessarily fully loaded.
    pub busy_is_awake: bool,
    /// (used, total) bytes of dedicated memory.
    pub vram: Option<(u64, u64)>,
    /// Labelled readings in °C, with the critical value to colour against.
    pub temps: Vec<(String, f64, f64)>,
    pub power_w: Option<f64>,
    pub power_cap_w: Option<f64>,
    pub fan: Option<String>,
    pub clock_mhz: Option<f64>,
}

fn read_u64(p: &Path) -> Option<u64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn read_f64(p: &Path) -> Option<f64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn first_existing(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|p| p.exists()).cloned()
}

/// "DG2 [Arc A750]" -> "Arc A750"; "Navi 31 [Radeon RX 7900 XT/7900 XTX/...]"
/// -> "Navi 31", since the bracket lists several cards and would guess wrong.
fn short_name(entry: &str) -> String {
    match (entry.find('['), entry.rfind(']')) {
        (Some(a), Some(b)) if b > a => {
            let inner = &entry[a + 1..b];
            if inner.contains('/') {
                entry[..a].trim().to_string()
            } else {
                inner.trim().to_string()
            }
        }
        _ => entry.trim().to_string(),
    }
}

/// Device name from the pci.ids database, if hwdata is installed.
fn pci_name(vendor: u16, device: u16) -> Option<String> {
    let text = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"]
        .iter()
        .find_map(|p| fs::read_to_string(p).ok())?;
    let v = format!("{vendor:04x}  ");
    let d = format!("\t{device:04x}  ");
    let mut in_vendor = false;
    for line in text.lines() {
        if !line.starts_with('\t') && !line.starts_with('#') && !line.is_empty() {
            if in_vendor {
                return None;
            }
            in_vendor = line.starts_with(&v);
        } else if in_vendor && let Some(rest) = line.strip_prefix(&d) {
            return Some(short_name(rest));
        }
    }
    None
}

fn vendor_name(vendor: u16) -> &'static str {
    match vendor {
        0x1002 => "AMD",
        0x8086 => "Intel",
        0x10de => "Nvidia",
        _ => "GPU",
    }
}

enum Kind {
    Amd,
    /// i915 or xe: utilisation from RC6 residency, power from energy.
    Intel { rc6: Option<PathBuf> },
    Other,
}

struct Dev {
    name: String,
    driver: String,
    dev: PathBuf,
    hwmon: Option<PathBuf>,
    kind: Kind,
    freq: Option<PathBuf>,
    temps: Vec<(String, PathBuf, f64)>,
    prev_rc6: Option<(f64, Instant)>,
    prev_energy: Option<(f64, Instant)>,
}

impl Dev {
    fn open(card: &Path) -> Option<Dev> {
        let dev = card.join("device");
        let hex = |f: &str| {
            let s = fs::read_to_string(dev.join(f)).ok()?;
            u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
        };
        let (vendor, device) = (hex("vendor")?, hex("device")?);
        let driver = fs::read_link(dev.join("driver"))
            .ok()?
            .file_name()?
            .to_string_lossy()
            .into_owned();
        let model = pci_name(vendor, device).unwrap_or_else(|| format!("{device:04x}"));
        let hwmon = fs::read_dir(dev.join("hwmon"))
            .ok()
            .and_then(|mut rd| rd.next())
            .and_then(|e| e.ok())
            .map(|e| e.path());

        let gt = card.join("gt/gt0");
        let xe_gt = dev.join("tile0/gt0");
        let (kind, freq) = match driver.as_str() {
            "amdgpu" => (Kind::Amd, None),
            "i915" => (
                Kind::Intel {
                    rc6: first_existing(&[
                        gt.join("rc6_residency_ms"),
                        card.join("power/rc6_residency_ms"),
                    ]),
                },
                first_existing(&[gt.join("rps_act_freq_mhz"), card.join("gt_act_freq_mhz")]),
            ),
            "xe" => (
                Kind::Intel {
                    rc6: first_existing(&[xe_gt.join("gtidle/idle_residency_ms")]),
                },
                first_existing(&[xe_gt.join("freq0/act_freq")]),
            ),
            _ => (Kind::Other, None),
        };

        let temps = hwmon
            .as_deref()
            .map(|h| {
                labelled_temps(h)
                    .into_iter()
                    .map(|(label, path)| {
                        let crit = read_milli(&sibling(&path, "crit"))
                            .or_else(|| read_milli(&sibling(&path, "emergency")))
                            .filter(|c| *c > 0.0 && *c < 200.0)
                            .unwrap_or(DEFAULT_CRIT);
                        (short_temp_label(&label), path, crit)
                    })
                    .collect()
            })
            .unwrap_or_default();

        Some(Dev {
            name: format!("{} {model}", vendor_name(vendor)),
            driver,
            dev,
            hwmon,
            kind,
            freq,
            temps,
            prev_rc6: None,
            prev_energy: None,
        })
    }

    fn sample(&mut self) -> Gpu {
        let now = Instant::now();
        let mut g = Gpu {
            name: self.name.clone(),
            driver: self.driver.clone(),
            temps: self
                .temps
                .iter()
                .filter_map(|(l, p, c)| Some((l.clone(), read_milli(p)?, *c)))
                .collect(),
            ..Default::default()
        };
        let hw = |f: &str| self.hwmon.as_ref().map(|h| h.join(f));

        g.power_w = hw("power1_average")
            .and_then(|p| read_f64(&p))
            .or_else(|| hw("power1_input").and_then(|p| read_f64(&p)))
            .map(|uw| uw / 1e6);
        if g.power_w.is_none()
            && let Some(uj) = hw("energy1_input").and_then(|p| read_f64(&p))
        {
            if let Some((prev, at)) = self.prev_energy {
                let dt = now.duration_since(at).as_secs_f64();
                if dt > 0.0 && uj >= prev {
                    g.power_w = Some((uj - prev) / 1e6 / dt);
                }
            }
            self.prev_energy = Some((uj, now));
        }
        g.power_cap_w = ["power1_cap", "power1_max"]
            .iter()
            .find_map(|f| hw(f).and_then(|p| read_f64(&p)))
            .filter(|w| *w > 0.0)
            .map(|uw| uw / 1e6);
        g.fan = hw("fan1_input")
            .and_then(|p| read_u64(&p))
            .map(|rpm| format!("{rpm}rpm"));

        match &self.kind {
            Kind::Amd => {
                g.busy = read_f64(&self.dev.join("gpu_busy_percent"));
                if let (Some(u), Some(t)) = (
                    read_u64(&self.dev.join("mem_info_vram_used")),
                    read_u64(&self.dev.join("mem_info_vram_total")),
                ) {
                    g.vram = Some((u, t));
                }
                g.clock_mhz = hw("freq1_input")
                    .and_then(|p| read_f64(&p))
                    .map(|hz| hz / 1e6);
            }
            Kind::Intel { rc6 } => {
                if let Some(ms) = rc6.as_ref().and_then(|p| read_f64(p)) {
                    if let Some((prev, at)) = self.prev_rc6 {
                        let wall = now.duration_since(at).as_secs_f64() * 1000.0;
                        if wall > 0.0 {
                            let idle = ((ms - prev) / wall).clamp(0.0, 1.0);
                            g.busy = Some((1.0 - idle) * 100.0);
                            g.busy_is_awake = true;
                        }
                    }
                    self.prev_rc6 = Some((ms, now));
                }
                g.clock_mhz = self.freq.as_ref().and_then(|p| read_f64(p));
            }
            Kind::Other => {}
        }
        g
    }
}

/// hwmon labels as they read in a title: amdgpu's "junction" is long.
fn short_temp_label(l: &str) -> String {
    match l {
        "junction" => "hot".into(),
        l if l.starts_with("temp") => "temp".into(),
        l => l.to_string(),
    }
}

pub struct Gpus {
    devs: Vec<Dev>,
    nvml: Option<Nvml>,
}

impl Gpus {
    pub fn find() -> Self {
        let nvml = Nvml::load();
        let mut cards: Vec<PathBuf> = fs::read_dir("/sys/class/drm")
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        let n = p.file_name().unwrap_or_default().to_string_lossy();
                        n.starts_with("card") && !n.contains('-')
                    })
                    .collect()
            })
            .unwrap_or_default();
        cards.sort();
        let devs = cards
            .iter()
            .filter_map(|c| Dev::open(c))
            // NVML reports Nvidia cards far better than sysfs does.
            .filter(|d| !(nvml.is_some() && d.name.starts_with("Nvidia")))
            .collect();
        Gpus { devs, nvml }
    }

    pub fn sample(&mut self) -> Vec<Gpu> {
        let mut out: Vec<Gpu> = self.devs.iter_mut().map(Dev::sample).collect();
        if let Some(n) = &self.nvml {
            out.extend(n.sample());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names() {
        assert_eq!(short_name("DG2 [Arc A750]"), "Arc A750");
        assert_eq!(short_name("Raphael [Radeon 610M]"), "Radeon 610M");
        assert_eq!(
            short_name("Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]"),
            "Navi 31"
        );
        assert_eq!(short_name("GA102"), "GA102");
    }
}
