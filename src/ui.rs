use crate::app::App;
use crate::collect::disk;
use crate::history::History;
use crate::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use ratatui::widgets::{Block, BorderType, Cell, Paragraph, Row, Table};

/// df -h style: powers of 1024, rounded up, one decimal below 10.
pub fn human(bytes: f64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    if bytes < 1.0 {
        return "0".into();
    }
    let mut v = bytes;
    let mut u = 0;
    while u < UNITS.len() - 1 && (v >= 1024.0 || (u > 0 && v.ceil() >= 1024.0)) {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{:.0}B", v.ceil())
    } else if v < 10.0 && (v * 10.0).ceil() < 100.0 {
        format!("{:.1}{}", (v * 10.0).ceil() / 10.0, UNITS[u])
    } else {
        format!("{:.0}{}", v.ceil(), UNITS[u])
    }
}

fn rate(bps: f64) -> String {
    format!("{}/s", human(bps))
}

/// Round up to 1, 2 or 5 times a power of ten, counted in the same
/// 1024-based unit `human` prints, so axis labels come out as round numbers.
fn nice_ceiling(v: f64) -> f64 {
    let v = v.max(1024.0);
    let mut unit = 1.0;
    while v / unit >= 1024.0 {
        unit *= 1024.0;
    }
    let x = v / unit;
    let mag = 10f64.powf(x.log10().floor());
    let m = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .find(|m| x <= m * mag)
        .unwrap_or(10.0);
    m * mag * unit
}

fn panel<'a>(t: &Theme, title: Vec<Span<'a>>) -> Block<'a> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(title);
    spans.push(Span::raw(" "));
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.border))
        .title(Line::from(spans))
}

fn heading(t: &Theme, s: &str) -> Span<'static> {
    Span::styled(
        s.to_string(),
        Style::new().fg(t.title).add_modifier(Modifier::BOLD),
    )
}

fn key(color: Color, s: String) -> Span<'static> {
    Span::styled(format!("  ━ {s}"), Style::new().fg(color))
}

fn dim(t: &Theme, s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), Style::new().fg(t.dim))
}

/// How many steps the y axis is divided into: fifths when there is room.
fn label_steps(height: u16) -> usize {
    if height >= 11 { 5 } else { 2 }
}

/// Percent axis labels every 20 when there is room for them, else every 50.
fn pct_labels(height: u16) -> Vec<String> {
    let n = label_steps(height);
    (0..=n).map(|k| (k * 100 / n).to_string()).collect()
}

type Series<'a> = (&'a [(f64, f64)], Color);

/// Draws y-axis labels and the axis line on the left of `area` and returns
/// what is left for the plot.
fn y_axis(f: &mut Frame, t: &Theme, area: Rect, labels: &[String]) -> Rect {
    let label_w = labels.iter().map(|l| l.len()).max().unwrap_or(0) as u16;
    let [label_area, axis_area, plot] = Layout::horizontal([
        Constraint::Length(label_w),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);

    // Labels spread evenly from the bottom row (first label) to the top row (last).
    let h = label_area.height as usize;
    let mut rows = vec![Line::raw(""); h];
    if h > 0 && labels.len() > 1 {
        for (k, l) in labels.iter().enumerate() {
            let row = h - 1 - (k * (h - 1) + (labels.len() - 1) / 2) / (labels.len() - 1);
            rows[row] = Line::from(dim(t, format!("{l:<w$}", w = label_w as usize)));
        }
    }
    f.render_widget(Paragraph::new(rows), label_area);
    let axis: Vec<Line> = (0..axis_area.height)
        .map(|_| Line::styled("│", Style::new().fg(t.border)))
        .collect();
    f.render_widget(Paragraph::new(axis), axis_area);

    plot
}

/// Line graph with a labelled y axis. All series share one braille canvas
/// layer, so where lines cross the dots combine (ratatui's Chart gives each
/// dataset its own layer, and a later layer's cell wipes out the dots below).
fn graph(
    f: &mut Frame,
    t: &Theme,
    area: Rect,
    series: &[Series],
    cap: usize,
    y_max: f64,
    labels: &[String],
) {
    let plot = y_axis(f, t, area, labels);
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, (cap - 1) as f64])
        .y_bounds([0.0, y_max])
        .paint(|ctx| {
            for (points, color) in series {
                for w in points.windows(2) {
                    ctx.draw(&CanvasLine {
                        x1: w[0].0,
                        y1: w[0].1,
                        x2: w[1].0,
                        y2: w[1].1,
                        color: *color,
                    });
                }
            }
        });
    f.render_widget(canvas, plot);
}

/// Braille dot bits, indexed [column][row from the top].
const DOT: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

fn braille(bits: u32) -> char {
    char::from_u32(0x2800 + bits).unwrap_or(' ')
}

/// Stacked area graph in percent, in braille dots: `low` filled from the
/// bottom (coloured by `low_color` of its value), `high` stacked on top in
/// `high_color`, the rest left empty. Each character is 2 dots wide and 4
/// tall; one carries one colour, so a cell holding both bands takes the
/// colour of whichever has more dots in it.
fn stacked_area(
    f: &mut Frame,
    t: &Theme,
    area: Rect,
    low: &History,
    high: &History,
    low_color: impl Fn(f64) -> Color,
    high_color: Color,
) {
    let plot = y_axis(f, t, area, &pct_labels(area.height));
    let (w, h) = (plot.width as usize, plot.height as usize);
    if w == 0 || h == 0 {
        return;
    }
    let span = (low.cap() - 1) as f64;
    let dots_w = w * 2;
    let to_dots = |pct: f64| ((pct.clamp(0.0, 100.0) / 100.0) * (h * 4) as f64).round() as usize;
    // Per dot column: (low value, low height, low+high height) in dots.
    let cols: Vec<Option<(f64, usize, usize)>> = (0..dots_w)
        .map(|d| {
            let x = if dots_w == 1 {
                span
            } else {
                d as f64 * span / (dots_w - 1) as f64
            };
            let lo = low.value_at(x)?;
            let hi = high.value_at(x).unwrap_or(0.0);
            Some((lo, to_dots(lo), to_dots(lo + hi)))
        })
        .collect();

    let buf = f.buffer_mut();
    for c in 0..w {
        let pair = [cols[c * 2], cols[c * 2 + 1]];
        let lo_max = pair.iter().flatten().map(|p| p.0).fold(f64::NAN, f64::max);
        if lo_max.is_nan() {
            continue;
        }
        let lc = low_color(lo_max);
        for r in 0..h {
            let (mut bits, mut n_low, mut n_high) = (0u32, 0, 0);
            for (dc, col) in pair.iter().enumerate() {
                let Some((_, lo_d, top_d)) = *col else {
                    continue;
                };
                for k in 0..4 {
                    let dot = r * 4 + k;
                    if dot < lo_d {
                        n_low += 1;
                    } else if dot < top_d {
                        n_high += 1;
                    } else {
                        continue;
                    }
                    bits |= DOT[dc][3 - k];
                }
            }
            if bits == 0 {
                continue;
            }
            let color = if n_low >= n_high { lc } else { high_color };
            buf[(plot.x + c as u16, plot.y + (h - 1 - r) as u16)]
                .set_char(braille(bits))
                .set_fg(color);
        }
    }
}

fn rate_graph(f: &mut Frame, t: &Theme, area: Rect, series: &[Series], cap: usize, max: f64) {
    let top = nice_ceiling(max);
    graph(
        f,
        t,
        area,
        series,
        cap,
        top,
        &rate_labels(top, label_steps(area.height)),
    );
}

fn rate_labels(top: f64, steps: usize) -> Vec<String> {
    (0..=steps)
        .map(|k| human(top * k as f64 / steps as f64))
        .collect()
}

/// A horizontal bar of braille dots, 4 dots tall and 2 per character, on a
/// track of faint dots.
fn bar(t: &Theme, pct: f64, width: usize, color: Color) -> Vec<Span<'static>> {
    let dots = ((pct.clamp(0.0, 100.0) / 100.0) * (width * 2) as f64).round() as usize;
    let (full, half) = (dots / 2, dots % 2 == 1);
    let mut spans = vec![Span::styled("⣿".repeat(full), Style::new().fg(color))];
    let mut used = full;
    if half && used < width {
        spans.push(Span::styled("⡇", Style::new().fg(color)));
        used += 1;
    }
    if used < width {
        spans.push(Span::styled(
            "⣿".repeat(width - used),
            Style::new().fg(t.track),
        ));
    }
    spans
}

pub fn draw(f: &mut Frame, app: &App) {
    let [top, mid, bottom] = Layout::vertical([
        Constraint::Percentage(30),
        Constraint::Percentage(30),
        Constraint::Fill(1),
    ])
    .areas(f.area());
    let [mem_area, net_area] = Layout::horizontal([Constraint::Percentage(50); 2]).areas(mid);
    let [nfs_area, disk_area] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(bottom);

    draw_cpu(f, app, top);
    draw_mem(f, app, mem_area);
    draw_net(f, app, net_area);
    draw_nfs(f, app, nfs_area);
    draw_disks(f, app, disk_area);
}

fn last(h: &History) -> f64 {
    h.points().last().map(|p| p.1).unwrap_or(0.0)
}

fn draw_cpu(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let mut title = vec![
        heading(t, "CPU"),
        Span::raw(format!("  {:.0}%", last(&app.cpu_total))),
        key(t.iowait, format!("iowait {:.1}%", last(&app.cpu_iowait))),
        dim(t, format!("   load {}", app.load)),
    ];
    title.push(dim(
        t,
        format!(
            "   {}s · t colours: {} · q quit",
            app.interval.as_secs_f64(),
            t.name
        ),
    ));
    let block = panel(t, title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Legend on the right: each core's name and load in its line's colour,
    // in as many columns as the height requires.
    let rows = inner.height.max(1) as usize;
    let n = app.cores.len();
    let cols = n.div_ceil(rows).max(1);
    const CELL: usize = 12;
    let legend_w = (cols * CELL) as u16;
    let [chart_area, _, legend_area] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(2),
        Constraint::Length(legend_w.min(inner.width / 2)),
    ])
    .areas(inner);

    let points: Vec<_> = app.core_hist.iter().map(History::points).collect();
    let series: Vec<Series> = points
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_slice(), t.core(i)))
        .collect();
    graph(
        f,
        t,
        chart_area,
        &series,
        app.cpu_total.cap(),
        100.0,
        &pct_labels(chart_area.height),
    );

    let lines: Vec<Line> = (0..rows)
        .map(|r| {
            let spans: Vec<Span> = (0..cols)
                .filter_map(|c| {
                    let i = c * rows + r;
                    let p = app.cores.get(i)?;
                    Some(Span::styled(
                        format!("{:<5}{:>5}  ", format!("cpu{i}"), format!("{p:.0}%")),
                        Style::new().fg(t.core(i)),
                    ))
                })
                .collect();
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), legend_area);
}

fn draw_mem(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let m = &app.mem;
    let used_pct = last(&app.mem_hist);
    let block = panel(
        t,
        vec![
            heading(t, "Memory"),
            key(
                t.level(used_pct),
                format!(
                    "used {} / {}  {:.0}%",
                    human(m.used() as f64),
                    human(m.total as f64),
                    used_pct
                ),
            ),
            key(
                t.mem_cache,
                format!("cache {}", human(m.reclaimable() as f64)),
            ),
            dim(t, format!("   free {}", human(m.free as f64))),
        ],
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [chart_area, info] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    stacked_area(
        f,
        t,
        chart_area,
        &app.mem_hist,
        &app.cache_hist,
        |pct| t.level(pct),
        t.mem_cache,
    );

    let mut spans = vec![
        dim(t, "avail "),
        Span::raw(human(m.available as f64)),
        dim(t, "  buffers "),
        Span::raw(human(m.buffers as f64)),
        dim(t, "  dirty "),
        Span::raw(human(m.dirty as f64)),
    ];
    if m.swap_total > 0 {
        let pct = m.swap_used() as f64 * 100.0 / m.swap_total as f64;
        spans.push(dim(t, "   swap "));
        spans.extend(bar(t, pct, 12, t.level(pct)));
        spans.push(Span::raw(format!(
            " {pct:.0}%  {} / {}",
            human(m.swap_used() as f64),
            human(m.swap_total as f64)
        )));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), info);
}

fn draw_net(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let block = panel(
        t,
        vec![
            heading(t, "Network"),
            key(t.rx, format!("↓ {}", rate(last(&app.rx_hist)))),
            key(t.tx, format!("↑ {}", rate(last(&app.tx_hist)))),
            dim(
                t,
                format!(
                    "   session ↓ {} ↑ {}",
                    human(app.session_rx as f64),
                    human(app.session_tx as f64)
                ),
            ),
        ],
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Physical interfaces always, virtual ones only while they carry traffic.
    let shown: Vec<_> = app
        .ifaces
        .iter()
        .filter(|i| !i.virt || i.rx_rate + i.tx_rate > 0.0)
        .take((inner.height / 3) as usize)
        .collect();
    let [chart_area, list] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(shown.len() as u16)])
            .areas(inner);

    let rx = app.rx_hist.points();
    let tx = app.tx_hist.points();
    let max = app.rx_hist.max().max(app.tx_hist.max());
    rate_graph(
        f,
        t,
        chart_area,
        &[(&rx, t.rx), (&tx, t.tx)],
        app.rx_hist.cap(),
        max,
    );

    let rows: Vec<Row> = shown
        .iter()
        .map(|i| {
            let name_style = if i.virt {
                Style::new().fg(t.dim)
            } else {
                Style::new()
            };
            Row::new(vec![
                Cell::from(Span::styled(i.name.clone(), name_style)),
                Cell::from(Span::styled(
                    format!("↓ {}", rate(i.rx_rate)),
                    Style::new().fg(t.rx),
                )),
                Cell::from(Span::styled(
                    format!("↑ {}", rate(i.tx_rate)),
                    Style::new().fg(t.tx),
                )),
                Cell::from(dim(
                    t,
                    format!(
                        "total ↓ {} ↑ {}",
                        human(i.rx_total as f64),
                        human(i.tx_total as f64)
                    ),
                )),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(16),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Fill(1),
    ];
    f.render_widget(Table::new(rows, widths), list);
}

fn ms(v: Option<f64>) -> String {
    match v {
        Some(v) if v >= 100.0 => format!("{v:.0}ms"),
        Some(v) => format!("{v:.1}ms"),
        None => "-".into(),
    }
}

fn uptime(secs: u64) -> String {
    match secs {
        s if s >= 86400 => format!("{}d{}h", s / 86400, s % 86400 / 3600),
        s if s >= 3600 => format!("{}h{}m", s / 3600, s % 3600 / 60),
        s => format!("{}m", s / 60),
    }
}

fn draw_nfs(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let block = panel(
        t,
        vec![
            heading(t, "NFS client"),
            key(
                t.nfs_read,
                format!("read {}", rate(last(&app.nfs_read_hist))),
            ),
            key(
                t.nfs_write,
                format!("write {}", rate(last(&app.nfs_write_hist))),
            ),
        ],
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.mounts.is_empty() {
        f.render_widget(Paragraph::new(dim(t, "no NFS mounts")), inner);
        return;
    }

    let per_mount = 4u16;
    let list_h = (per_mount * app.mounts.len() as u16).min(inner.height.saturating_sub(4));
    let [chart_area, list] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(list_h)]).areas(inner);

    let r = app.nfs_read_hist.points();
    let w = app.nfs_write_hist.points();
    let max = app.nfs_read_hist.max().max(app.nfs_write_hist.max());
    rate_graph(
        f,
        t,
        chart_area,
        &[(&r, t.nfs_read), (&w, t.nfs_write)],
        app.nfs_read_hist.cap(),
        max,
    );

    let mut lines = Vec::new();
    for m in &app.mounts {
        lines.push(Line::from(vec![
            Span::styled(
                m.mountpoint.clone(),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            dim(
                t,
                format!(
                    "  {}  v{} {} ×{}  up {}",
                    m.device,
                    m.vers,
                    m.proto,
                    m.connections,
                    uptime(m.age_secs)
                ),
            ),
        ]));
        let cache =
            (m.app_read > 0.0).then(|| (1.0 - m.wire_read / m.app_read).clamp(0.0, 1.0) * 100.0);
        lines.push(Line::from(vec![
            Span::styled(
                format!("  rd {:<9}", rate(m.wire_read)),
                Style::new().fg(t.nfs_read),
            ),
            Span::styled(
                format!(" wr {:<9}", rate(m.wire_write)),
                Style::new().fg(t.nfs_write),
            ),
            dim(t, " app rd "),
            Span::raw(rate(m.app_read)),
            dim(t, " wr "),
            Span::raw(rate(m.app_write)),
            dim(t, "  cache hit "),
            Span::raw(cache.map(|c| format!("{c:.0}%")).unwrap_or("-".into())),
        ]));
        let bad = m.retrans + m.timeouts + m.errors > 0;
        lines.push(Line::from(vec![
            dim(t, "  ops "),
            Span::raw(format!("{:.0}/s", m.ops_rate)),
            dim(t, "  rtt "),
            Span::raw(ms(m.rtt_ms)),
            dim(t, "  exec "),
            Span::raw(ms(m.exec_ms)),
            dim(t, "  READ "),
            Span::raw(ms(m.read_exec_ms)),
            dim(t, "  WRITE "),
            Span::raw(ms(m.write_exec_ms)),
            dim(t, "  retrans/timeo/err "),
            Span::styled(
                format!("{}/{}/{}", m.retrans, m.timeouts, m.errors),
                Style::new().fg(if bad { t.crit } else { t.dim }),
            ),
        ]));
        let mut top = vec![dim(t, "  top ")];
        if m.top_ops.is_empty() {
            top.push(dim(t, "idle"));
        }
        for (name, r) in &m.top_ops {
            top.push(Span::raw(format!("{name} ")));
            top.push(dim(t, format!("{r:.0}/s  ")));
        }
        top.push(dim(
            t,
            format!(
                " since mount rd {} wr {}",
                human(m.total_read as f64),
                human(m.total_write as f64)
            ),
        ));
        lines.push(Line::from(top));
    }
    f.render_widget(Paragraph::new(lines), list);
}

fn draw_disks(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let mut title = vec![heading(t, "Disks")];
    if app.disks_stale() {
        title.push(Span::styled(
            "  stale: df is blocked on a mount",
            Style::new().fg(t.crit),
        ));
    }
    let block = panel(t, title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(snap) = &app.disks else {
        f.render_widget(Paragraph::new(dim(t, "reading filesystems…")), inner);
        return;
    };
    let [totals_area, _, table_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);

    let total_line = |label: &str, (used, avail, size): (u64, u64, u64)| {
        let pct = if used + avail == 0 {
            0.0
        } else {
            used as f64 * 100.0 / (used + avail) as f64
        };
        let bar_w = (totals_area.width as usize)
            .saturating_sub(40)
            .clamp(10, 40);
        let mut spans = vec![Span::styled(
            format!("{label:<8}"),
            Style::new().add_modifier(Modifier::BOLD),
        )];
        spans.extend(bar(t, pct, bar_w, t.level(pct)));
        spans.push(Span::raw(format!(" {pct:>3.0}%  ")));
        spans.push(Span::raw(human(used as f64)));
        spans.push(dim(
            t,
            format!(
                " used of {}, {} free",
                human(size as f64),
                human(avail as f64)
            ),
        ));
        Line::from(spans)
    };
    let local = disk::totals(snap.list.iter().filter(|d| d.is_local_disk()));
    let network = disk::totals(snap.list.iter().filter(|d| d.is_network()));
    let mut totals = vec![total_line("local", local)];
    if network.2 > 0 {
        totals.push(total_line("network", network));
    }
    f.render_widget(Paragraph::new(totals), totals_area);

    let header = Row::new([
        "Filesystem",
        "Type",
        "Size",
        "Used",
        "Avail",
        "Use%",
        "",
        "IUse%",
        "Mounted on",
    ])
    .style(Style::new().fg(t.dim).add_modifier(Modifier::BOLD));
    let rows: Vec<Row> = snap
        .list
        .iter()
        .map(|d| {
            let pct = d.use_pct();
            let ipct = if d.inodes == 0 {
                None
            } else {
                Some((d.inodes_used as f64 * 100.0 / d.inodes as f64).ceil())
            };
            let pseudo = !d.is_local_disk() && !d.is_network();
            let base = if pseudo {
                Style::new().fg(t.dim)
            } else {
                Style::new()
            };
            Row::new(vec![
                Cell::from(d.source.clone()),
                Cell::from(d.fstype.clone()),
                Cell::from(human(d.size as f64)),
                Cell::from(human(d.used as f64)),
                Cell::from(human(d.avail as f64)),
                Cell::from(Span::styled(
                    format!("{pct:.0}%"),
                    Style::new().fg(t.level(pct)),
                )),
                Cell::from(Line::from(bar(t, pct, 8, t.level(pct)))),
                Cell::from(ipct.map(|p| format!("{p:.0}%")).unwrap_or("-".into())),
                Cell::from(d.mountpoint.clone()),
            ])
            .style(base)
        })
        .collect();
    let src_w = snap
        .list
        .iter()
        .map(|d| d.source.len())
        .max()
        .unwrap_or(10)
        .clamp(10, 28) as u16;
    let widths = [
        Constraint::Length(src_w),
        Constraint::Length(9),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Length(4),
        Constraint::Length(8),
        Constraint::Length(5),
        Constraint::Fill(1),
    ];
    f.render_widget(
        Table::new(rows, widths).header(header).column_spacing(1),
        table_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Mode, Theme};
    use ratatui::{Terminal, backend::TestBackend};
    use std::time::Duration;

    /// Renders a real frame from this machine's /proc. Run with
    /// `cargo test render -- --nocapture` to look at it; set RENDER_TICKS and
    /// RENDER_MS (e.g. 60 and 1000) to fill the graphs with real history.
    #[test]
    fn rate_axis_is_round() {
        assert_eq!(
            rate_labels(nice_ceiling(900_000.0), 5),
            ["0", "200K", "400K", "600K", "800K", "1000K"]
        );
        assert_eq!(
            rate_labels(nice_ceiling(3.0 * 1048576.0), 5),
            ["0", "1.0M", "2.0M", "3.0M", "4.0M", "5.0M"]
        );
        assert_eq!(nice_ceiling(0.0), 1024.0);
    }

    #[test]
    fn render() {
        let mut app = App::new(Theme::builtin(Mode::Light), Duration::from_secs(1), 60);
        for _ in 0..(std::env::var("RENDER_TICKS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3))
        {
            std::thread::sleep(Duration::from_millis(
                std::env::var("RENDER_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(400),
            ));
            app.tick();
        }
        std::thread::sleep(Duration::from_secs(1));
        app.tick();
        let mut term = Terminal::new(TestBackend::new(200, 56)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        let buf = term.backend().buffer();
        for y in 0..buf.area.height {
            let line: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            println!("{line}");
        }
    }
}
