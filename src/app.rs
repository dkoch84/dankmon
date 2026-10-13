use crate::collect::{cpu, disk, gpu, mem, net, nfs, temp};
use crate::history::History;
use crate::theme::Theme;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

pub struct App {
    pub theme: Theme,
    pub interval: Duration,

    cpu: cpu::Cpu,
    pub cpu_total: History,
    pub cpu_iowait: History,
    pub cores: Vec<f64>,
    pub core_hist: Vec<History>,
    window: usize,
    pub load: String,
    cpu_sensors: Option<temp::CpuSensors>,
    pub cpu_temp: Option<temp::CpuTemp>,

    pub mem: mem::Mem,
    pub mem_hist: History,
    pub cache_hist: History,

    net: net::Net,
    pub ifaces: Vec<net::Iface>,
    pub rx_hist: History,
    pub tx_hist: History,
    pub session_rx: u64,
    pub session_tx: u64,

    nfs: nfs::Nfs,
    pub mounts: Vec<nfs::Mount>,
    pub nfs_read_hist: History,
    pub nfs_write_hist: History,

    gpus: gpu::Gpus,
    pub gpu: Vec<gpu::Gpu>,
    pub gpu_hist: Vec<History>,

    disk_rx: Receiver<disk::Snapshot>,
    pub disks: Option<disk::Snapshot>,

    last: Instant,
    pub started: Instant,
}

impl App {
    pub fn new(theme: Theme, interval: Duration, window: usize) -> Self {
        let mut app = App {
            theme,
            interval,
            cpu: cpu::Cpu::default(),
            cpu_total: History::new(window),
            cpu_iowait: History::new(window),
            cores: Vec::new(),
            core_hist: Vec::new(),
            window,
            load: String::new(),
            cpu_sensors: temp::CpuSensors::find(),
            cpu_temp: None,
            mem: mem::Mem::default(),
            mem_hist: History::new(window),
            cache_hist: History::new(window),
            net: net::Net::default(),
            ifaces: Vec::new(),
            rx_hist: History::new(window),
            tx_hist: History::new(window),
            session_rx: 0,
            session_tx: 0,
            nfs: nfs::Nfs::default(),
            mounts: Vec::new(),
            nfs_read_hist: History::new(window),
            nfs_write_hist: History::new(window),
            gpus: gpu::Gpus::find(),
            gpu: Vec::new(),
            gpu_hist: Vec::new(),
            disk_rx: disk::spawn(Duration::from_secs(2)),
            disks: None,
            last: Instant::now(),
            started: Instant::now(),
        };
        // Prime the counters so the first real tick has something to diff against.
        app.cpu.sample();
        app.net.sample(0.0);
        app.nfs.sample(0.0);
        app.mem = mem::read();
        app.gpu = app.gpus.sample();
        app
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        self.last = now;

        let c = self.cpu.sample();
        self.cpu_total.push(c.total);
        self.cpu_iowait.push(c.iowait);
        self.cores = c.cores;
        if self.core_hist.len() != self.cores.len() {
            self.core_hist = (0..self.cores.len())
                .map(|_| History::new(self.window))
                .collect();
        }
        for (h, &p) in self.core_hist.iter_mut().zip(&self.cores) {
            h.push(p);
        }
        self.load = std::fs::read_to_string("/proc/loadavg")
            .map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
            .unwrap_or_default();

        self.cpu_temp = self.cpu_sensors.as_ref().map(temp::CpuSensors::read);

        self.gpu = self.gpus.sample();
        if self.gpu_hist.len() != self.gpu.len() {
            self.gpu_hist = (0..self.gpu.len())
                .map(|_| History::new(self.window))
                .collect();
        }
        for (h, g) in self.gpu_hist.iter_mut().zip(&self.gpu) {
            h.push(g.busy.unwrap_or(0.0));
        }

        self.mem = mem::read();
        let pct = |used: u64, total: u64| {
            if total == 0 {
                0.0
            } else {
                used as f64 * 100.0 / total as f64
            }
        };
        self.mem_hist.push(pct(self.mem.used(), self.mem.total));
        self.cache_hist
            .push(pct(self.mem.reclaimable(), self.mem.total));

        self.ifaces = self.net.sample(dt);
        let (rx, tx) = self
            .ifaces
            .iter()
            .filter(|i| !i.virt)
            .fold((0.0, 0.0), |(r, t), i| (r + i.rx_rate, t + i.tx_rate));
        self.rx_hist.push(rx);
        self.tx_hist.push(tx);
        self.session_rx += (rx * dt) as u64;
        self.session_tx += (tx * dt) as u64;

        self.mounts = self.nfs.sample(dt);
        let (r, w) = self
            .mounts
            .iter()
            .fold((0.0, 0.0), |(r, w), m| (r + m.wire_read, w + m.wire_write));
        self.nfs_read_hist.push(r);
        self.nfs_write_hist.push(w);

        while let Ok(snap) = self.disk_rx.try_recv() {
            self.disks = Some(snap);
        }
    }

    /// True when the df thread has not reported for a while, which means
    /// statvfs() is stuck on an unreachable network mount.
    pub fn disks_stale(&self) -> bool {
        let at = self.disks.as_ref().map(|d| d.at).unwrap_or(self.started);
        at.elapsed() > Duration::from_secs(10)
    }
}
