use std::collections::VecDeque;

/// Fixed-length series of samples, newest last.
pub struct History {
    cap: usize,
    data: VecDeque<f64>,
}

impl History {
    pub fn new(cap: usize) -> Self {
        History {
            cap,
            data: VecDeque::with_capacity(cap),
        }
    }

    pub fn push(&mut self, v: f64) {
        if self.data.len() == self.cap {
            self.data.pop_front();
        }
        self.data.push_back(v);
    }

    /// Chart points, right-aligned so the newest sample sits at the right edge.
    pub fn points(&self) -> Vec<(f64, f64)> {
        let offset = self.cap - self.data.len();
        self.data
            .iter()
            .enumerate()
            .map(|(i, &v)| ((offset + i) as f64, v))
            .collect()
    }

    /// Value at chart position x (0 ..= cap-1), interpolated between samples.
    /// None left of the oldest sample.
    pub fn value_at(&self, x: f64) -> Option<f64> {
        let offset = (self.cap - self.data.len()) as f64;
        if self.data.is_empty() || x < offset {
            return None;
        }
        let i = x - offset;
        let (lo, frac) = (i.floor() as usize, i.fract());
        let a = *self.data.get(lo)?;
        let b = self.data.get(lo + 1).copied().unwrap_or(a);
        Some(a + (b - a) * frac)
    }

    pub fn max(&self) -> f64 {
        self.data.iter().copied().fold(0.0, f64::max)
    }

    pub fn cap(&self) -> usize {
        self.cap
    }
}
