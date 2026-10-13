//! Colours.
//!
//! By default every colour comes from the terminal's own theme: data colours
//! are its ANSI slots (red, green, yellow, blue, magenta, cyan, plus an
//! orange when a bright slot has one), and neutrals such as borders and bar
//! tracks are blended from its background and foreground. Roles the theme
//! cannot fill fall back to built-in 256-colour choices, which themes leave
//! alone. Text and background stay at the terminal's defaults.

use ratatui::style::Color;
use std::io::Write;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Light,
    Dark,
}

#[derive(Clone)]
pub struct Theme {
    /// Shown in the key hint: "terminal", "light" or "dark".
    pub name: &'static str,
    pub iowait: Color,
    /// Reclaimable page cache, stacked above used memory: a pale shade.
    pub mem_cache: Color,
    pub rx: Color,
    pub tx: Color,
    pub nfs_read: Color,
    pub nfs_write: Color,
    pub border: Color,
    pub title: Color,
    pub dim: Color,
    pub ok: Color,
    pub warn: Color,
    pub crit: Color,
    pub track: Color,
    /// One colour per CPU core, cycled.
    pub cores: Vec<Color>,
}

impl Theme {
    /// Built-in colours from the 256-colour palette.
    pub fn builtin(mode: Mode) -> Self {
        let i = Color::Indexed;
        let cores = (1..=6).map(Color::Indexed).collect();
        match mode {
            Mode::Light => Theme {
                name: "light",
                iowait: i(130),
                mem_cache: i(151),
                rx: i(31),
                tx: i(166),
                nfs_read: i(91),
                nfs_write: i(160),
                border: i(249),
                title: i(236),
                dim: i(242),
                ok: i(28),
                warn: i(136),
                crit: i(160),
                track: i(251),
                cores,
            },
            Mode::Dark => Theme {
                name: "dark",
                iowait: i(179),
                mem_cache: i(22),
                rx: i(80),
                tx: i(209),
                nfs_read: i(177),
                nfs_write: i(203),
                border: i(240),
                title: i(252),
                dim: i(245),
                ok: i(114),
                warn: i(221),
                crit: i(203),
                track: i(239),
                cores,
            },
        }
    }

    /// Colours taken from the terminal's theme. None when the terminal did
    /// not report its background.
    pub fn from_terminal(c: &TermColors, truecolor: bool) -> Option<Self> {
        let bg = c.background?;
        let mut th = Theme::builtin(c.mode()?);
        th.name = "terminal";

        let visible =
            |i: usize| c.ansi[i].filter(|&rgb| chroma(rgb) >= 0.15 && contrast(rgb, bg) >= 1.6);
        let slot = |i: usize, fallback: Color| {
            if visible(i).is_some() {
                Color::Indexed(i as u8)
            } else {
                fallback
            }
        };
        // An orange, if one of the bright slots holds a hue that none of the
        // six normal colours already covers.
        let orange = (9..=14)
            .filter_map(|i| Some((i, hue(visible(i)?))))
            .filter(|&(_, h)| {
                (1..=6).all(|n| visible(n).is_none_or(|rgb| hue_dist(hue(rgb), h) >= 20.0))
            })
            .filter(|&(_, h)| hue_dist(h, 30.0) <= 30.0)
            .min_by(|a, b| hue_dist(a.1, 30.0).total_cmp(&hue_dist(b.1, 30.0)))
            .map(|(i, _)| Color::Indexed(i as u8));

        th.crit = slot(1, th.crit);
        th.ok = slot(2, th.ok);
        th.warn = slot(3, th.warn);
        th.nfs_read = slot(4, th.nfs_read);
        th.nfs_write = slot(5, th.nfs_write);
        th.rx = slot(6, th.rx);
        th.tx = orange.unwrap_or(slot(1, th.tx));
        th.iowait = orange.unwrap_or(slot(3, th.iowait));

        let mut cores: Vec<Color> = (1..=6)
            .filter(|&i| visible(i).is_some())
            .map(|i| Color::Indexed(i as u8))
            .collect();
        cores.extend(orange);
        if cores.len() >= 3 {
            th.cores = cores;
        }

        // Neutrals have no slot of their own, so blend them from the theme.
        if truecolor && let Some(fg) = c.foreground {
            th.title = Color::Reset;
            th.border = rgb(mix(bg, fg, 0.35));
            th.dim = rgb(mix(bg, fg, 0.75));
            th.track = rgb(mix(bg, fg, 0.25));
            if let Some(green) = visible(2) {
                th.mem_cache = rgb(mix(bg, green, 0.35));
            }
        }
        Some(th)
    }

    pub fn core(&self, i: usize) -> Color {
        self.cores[i % self.cores.len()]
    }

    /// Green / amber / red by how full something is.
    pub fn level(&self, pct: f64) -> Color {
        if pct >= 90.0 {
            self.crit
        } else if pct >= 75.0 {
            self.warn
        } else {
            self.ok
        }
    }
}

type Rgb = (f64, f64, f64);

/// What the terminal told us about its colours.
#[derive(Default)]
pub struct TermColors {
    pub background: Option<Rgb>,
    pub foreground: Option<Rgb>,
    /// The 16 ANSI colours, as the active theme defines them.
    pub ansi: [Option<Rgb>; 16],
}

impl TermColors {
    pub fn mode(&self) -> Option<Mode> {
        let bg = self.background?;
        Some(if luminance(bg) < 0.18 {
            Mode::Dark
        } else {
            Mode::Light
        })
    }
}

/// True when the terminal says it takes 24-bit colour.
pub fn truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit")
}

fn rgb((r, g, b): Rgb) -> Color {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::Rgb(c(r), c(g), c(b))
}

/// `t` of the way from `a` to `b`.
fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

/// Hue in degrees, 0 = red.
fn hue((r, g, b): Rgb) -> f64 {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    if d == 0.0 {
        return 0.0;
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h * 60.0
}

fn hue_dist(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    d.min(360.0 - d)
}

/// WCAG relative luminance.
fn luminance((r, g, b): Rgb) -> f64 {
    let lin = |c: f64| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// Spread between the strongest and weakest channel: near 0 for greys.
fn chroma((r, g, b): Rgb) -> f64 {
    r.max(g).max(b) - r.min(g).min(b)
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Parse "rgb:RRRR/GGGG/BBBB" (1 to 4 hex digits per channel).
fn parse_rgb(s: &str) -> Option<Rgb> {
    let mut parts = s.strip_prefix("rgb:")?.split('/').map(|p| {
        let hex: String = p.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        let max = (1u64 << (4 * hex.len().clamp(1, 4))) - 1;
        u64::from_str_radix(&hex, 16)
            .ok()
            .map(|v| v as f64 / max as f64)
    });
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Ask the terminal for its ANSI palette (OSC 4), foreground (OSC 10) and
/// background (OSC 11).
/// Must be called with the terminal in raw mode. Terminals answer in order,
/// so the background reply, asked for last, means everything has arrived.
pub fn query() -> TermColors {
    let mut colors = TermColors::default();
    if unsafe { libc::isatty(0) } == 0 {
        return colors;
    }
    let mut req = String::new();
    for i in 0..16 {
        req.push_str(&format!("\x1b]4;{i};?\x1b\\"));
    }
    req.push_str("\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
    let mut out = std::io::stdout();
    if out
        .write_all(req.as_bytes())
        .and_then(|_| out.flush())
        .is_err()
    {
        return colors;
    }

    let deadline = Instant::now() + Duration::from_millis(250);
    let mut buf = Vec::new();
    while !String::from_utf8_lossy(&buf).contains("]11;")
        || !(buf.ends_with(b"\x07") || buf.ends_with(b"\x1b\\"))
    {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut pfd, 1, left.as_millis() as i32) } <= 0 {
            break;
        }
        let mut chunk = [0u8; 256];
        let n = unsafe { libc::read(0, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
    }

    parse_replies(&mut colors, &String::from_utf8_lossy(&buf));
    colors
}

fn parse_replies(colors: &mut TermColors, text: &str) {
    for reply in text.split("\x1b]").skip(1) {
        let reply = reply.trim_end_matches(['\x07', '\x1b', '\\']);
        if let Some(rest) = reply.strip_prefix("11;") {
            colors.background = parse_rgb(rest);
        } else if let Some(rest) = reply.strip_prefix("10;") {
            colors.foreground = parse_rgb(rest);
        } else if let Some(rest) = reply.strip_prefix("4;")
            && let Some((idx, rgb)) = rest.split_once(';')
            && let Ok(i) = idx.parse::<usize>()
            && i < 16
        {
            colors.ansi[i] = parse_rgb(rgb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb_of(h: &str) -> String {
        let c = |i: usize| format!("{0}{0}", &h[i..i + 2]);
        format!("rgb:{}/{}/{}", c(0), c(2), c(4))
    }

    fn reply(ansi: &[&str], fg: &str, bg: &str) -> String {
        let mut s: String = ansi
            .iter()
            .enumerate()
            .map(|(i, h)| format!("\x1b]4;{i};{}\x1b\\", rgb_of(h)))
            .collect();
        s.push_str(&format!("\x1b]10;{}\x1b\\", rgb_of(fg)));
        s.push_str(&format!("\x1b]11;{}\x1b\\", rgb_of(bg)));
        s
    }

    fn colors(ansi: &[&str], fg: &str, bg: &str) -> TermColors {
        let mut c = TermColors::default();
        parse_replies(&mut c, &reply(ansi, fg, bg));
        c
    }

    /// A light theme whose "black" is grey, "white" near-black, bright red
    /// an orange, and most other bright slots greys.
    const LIGHT: [&str; 16] = [
        "6d767d", "d6769e", "43a838", "b0913a", "469bd4", "a980e8", "37a2a6", "0b141a", "0b141a",
        "d67e47", "61707a", "6d767d", "787e82", "d6769e", "808487", "0b141a",
    ];

    #[test]
    fn light_theme_roles_come_from_its_slots() {
        let c = colors(&LIGHT, "6d767d", "f7f8f3");
        assert_eq!(c.mode(), Some(Mode::Light));
        let t = Theme::from_terminal(&c, true).unwrap();
        let i = Color::Indexed;
        assert_eq!((t.crit, t.ok, t.warn), (i(1), i(2), i(3)));
        assert_eq!((t.rx, t.tx, t.iowait), (i(6), i(9), i(9)));
        assert_eq!((t.nfs_read, t.nfs_write), (i(4), i(5)));
        // Six chromatic normals plus the orange; greys and near-black left out.
        assert_eq!(t.cores, [1, 2, 3, 4, 5, 6, 9].map(i));
        // Neutrals blended from background and foreground.
        assert!(matches!(t.border, Color::Rgb(..)));
        assert!(matches!(t.mem_cache, Color::Rgb(..)));
    }

    #[test]
    fn no_truecolor_keeps_builtin_neutrals() {
        let c = colors(&LIGHT, "6d767d", "f7f8f3");
        let t = Theme::from_terminal(&c, false).unwrap();
        assert_eq!(t.border, Theme::builtin(Mode::Light).border);
    }

    #[test]
    fn dark_theme_without_orange() {
        // Bright slots repeat the normal hues, so no orange is found.
        let ansi = [
            "1d1f21", "cc6666", "b5bd68", "f0c674", "81a2be", "b294bb", "8abeb7", "c5c8c6",
            "969896", "cc6666", "b5bd68", "f0c674", "81a2be", "b294bb", "8abeb7", "ffffff",
        ];
        let c = colors(&ansi, "c5c8c6", "1d1f21");
        assert_eq!(c.mode(), Some(Mode::Dark));
        let t = Theme::from_terminal(&c, true).unwrap();
        assert_eq!(t.cores, (1..=6).map(Color::Indexed).collect::<Vec<_>>());
        assert_eq!(t.tx, Color::Indexed(1));
    }

    #[test]
    fn no_answer_means_builtin() {
        assert!(Theme::from_terminal(&TermColors::default(), true).is_none());
    }
}
