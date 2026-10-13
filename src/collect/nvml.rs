//! Nvidia GPUs through NVML (libnvidia-ml.so.1, shipped with the proprietary
//! driver). The library is loaded at runtime, so dankmon neither links
//! against it nor needs it installed.

use super::gpu::Gpu;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};

type Device = *mut c_void;
type Ret = c_int;
const SUCCESS: Ret = 0;
const TEMPERATURE_GPU: c_int = 0;
const CLOCK_GRAPHICS: c_int = 0;
/// Temperature to colour against: NVML's slowdown threshold is not exposed
/// by every card, and most throttle in the high 80s.
const CRIT: f64 = 90.0;

#[repr(C)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

#[repr(C)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}

struct Api {
    count: unsafe extern "C" fn(*mut c_uint) -> Ret,
    handle: unsafe extern "C" fn(c_uint, *mut Device) -> Ret,
    name: unsafe extern "C" fn(Device, *mut c_char, c_uint) -> Ret,
    util: unsafe extern "C" fn(Device, *mut Utilization) -> Ret,
    memory: unsafe extern "C" fn(Device, *mut Memory) -> Ret,
    temp: unsafe extern "C" fn(Device, c_int, *mut c_uint) -> Ret,
    power: unsafe extern "C" fn(Device, *mut c_uint) -> Ret,
    power_limit: unsafe extern "C" fn(Device, *mut c_uint) -> Ret,
    fan: unsafe extern "C" fn(Device, *mut c_uint) -> Ret,
    clock: unsafe extern "C" fn(Device, c_int, *mut c_uint) -> Ret,
}

pub struct Nvml {
    api: Api,
    devices: Vec<(Device, String)>,
}

unsafe fn sym<T: Copy>(lib: *mut c_void, name: &CStr) -> Option<T> {
    let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
    // SAFETY: T is always an extern "C" fn pointer with NVML's signature.
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy(&p) })
}

impl Nvml {
    pub fn load() -> Option<Nvml> {
        unsafe {
            let lib = libc::dlopen(c"libnvidia-ml.so.1".as_ptr(), libc::RTLD_NOW);
            if lib.is_null() {
                return None;
            }
            let init: unsafe extern "C" fn() -> Ret = sym(lib, c"nvmlInit_v2")?;
            if init() != SUCCESS {
                return None;
            }
            let api = Api {
                count: sym(lib, c"nvmlDeviceGetCount_v2")?,
                handle: sym(lib, c"nvmlDeviceGetHandleByIndex_v2")?,
                name: sym(lib, c"nvmlDeviceGetName")?,
                util: sym(lib, c"nvmlDeviceGetUtilizationRates")?,
                memory: sym(lib, c"nvmlDeviceGetMemoryInfo")?,
                temp: sym(lib, c"nvmlDeviceGetTemperature")?,
                power: sym(lib, c"nvmlDeviceGetPowerUsage")?,
                power_limit: sym(lib, c"nvmlDeviceGetEnforcedPowerLimit")?,
                fan: sym(lib, c"nvmlDeviceGetFanSpeed")?,
                clock: sym(lib, c"nvmlDeviceGetClockInfo")?,
            };
            let mut n: c_uint = 0;
            if (api.count)(&mut n) != SUCCESS || n == 0 {
                return None;
            }
            let mut devices = Vec::new();
            for i in 0..n {
                let mut d: Device = std::ptr::null_mut();
                if (api.handle)(i, &mut d) != SUCCESS {
                    continue;
                }
                let mut buf = [0 as c_char; 96];
                let name = if (api.name)(d, buf.as_mut_ptr(), buf.len() as c_uint) == SUCCESS {
                    CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
                } else {
                    "GPU".into()
                };
                let name = name.strip_prefix("NVIDIA ").unwrap_or(&name).to_string();
                devices.push((d, format!("Nvidia {name}")));
            }
            Some(Nvml { api, devices })
        }
    }

    pub fn sample(&self) -> Vec<Gpu> {
        let a = &self.api;
        self.devices
            .iter()
            .map(|&(d, ref name)| unsafe {
                let get = |f: unsafe extern "C" fn(Device, *mut c_uint) -> Ret| {
                    let mut v: c_uint = 0;
                    (f(d, &mut v) == SUCCESS).then_some(v)
                };
                let mut u = Utilization { gpu: 0, memory: 0 };
                let mut m = Memory { total: 0, free: 0, used: 0 };
                let mut temp: c_uint = 0;
                let mut clock: c_uint = 0;
                Gpu {
                    name: name.clone(),
                    driver: "nvidia".into(),
                    busy: ((a.util)(d, &mut u) == SUCCESS).then_some(u.gpu as f64),
                    busy_is_awake: false,
                    vram: ((a.memory)(d, &mut m) == SUCCESS).then_some((m.used, m.total)),
                    temps: ((a.temp)(d, TEMPERATURE_GPU, &mut temp) == SUCCESS)
                        .then(|| ("gpu".to_string(), temp as f64, CRIT))
                        .into_iter()
                        .collect(),
                    power_w: get(a.power).map(|mw| mw as f64 / 1000.0),
                    power_cap_w: get(a.power_limit).map(|mw| mw as f64 / 1000.0),
                    fan: get(a.fan).map(|p| format!("{p}%")),
                    clock_mhz: ((a.clock)(d, CLOCK_GRAPHICS, &mut clock) == SUCCESS)
                        .then_some(clock as f64),
                }
            })
            .collect()
    }
}
