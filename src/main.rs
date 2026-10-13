mod app;
mod collect;
mod history;
mod theme;
mod ui;

use app::App;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::{Duration, Instant};
use theme::{Mode, Theme};

const USAGE: &str = "\
dankmon: a small system monitor (CPU, memory, network, NFS client, disks)

usage: dankmon [-i SECONDS] [-w SAMPLES] [--light | --dark]

  -i, --interval SECONDS   sample interval (default 1)
  -w, --window SAMPLES     samples kept in each graph (default 60)
      --light, --dark      start on the built-in colours (default: the terminal's theme)

keys: q / Esc quit, t cycle colours (terminal theme, built-in light, built-in dark)
";

struct Args {
    interval: Duration,
    window: usize,
    mode: Option<Mode>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        interval: Duration::from_secs(1),
        window: 60,
        mode: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-i" | "--interval" => {
                let v: f64 = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--interval needs a number")?;
                if !(0.1..=60.0).contains(&v) {
                    return Err("--interval must be between 0.1 and 60".into());
                }
                args.interval = Duration::from_secs_f64(v);
            }
            "-w" | "--window" => {
                let v: usize = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--window needs a number")?;
                args.window = v.clamp(10, 3600);
            }
            "--light" => args.mode = Some(Mode::Light),
            "--dark" => args.mode = Some(Mode::Dark),
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

fn main() -> std::io::Result<()> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprint!("dankmon: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    let mut terminal = ratatui::init();
    let colors = theme::query();
    // Cycle with `t`: the terminal's own colours (when it reported them),
    // then the built-in light and dark sets. --light/--dark start on those.
    let mut themes: Vec<Theme> = Theme::from_terminal(&colors, theme::truecolor())
        .into_iter()
        .collect();
    themes.push(Theme::builtin(Mode::Light));
    themes.push(Theme::builtin(Mode::Dark));
    let start = match args.mode {
        Some(m) => themes
            .iter()
            .position(|t| t.name == if m == Mode::Light { "light" } else { "dark" }),
        None if themes[0].name == "terminal" => Some(0),
        None => colors.mode().map(|m| if m == Mode::Dark { 1 } else { 0 }),
    }
    .unwrap_or(0);
    let mut app = App::new(themes[start].clone(), args.interval, args.window);
    let result = run(&mut terminal, &mut app, &themes, start);
    ratatui::restore();
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    themes: &[Theme],
    mut current: usize,
) -> std::io::Result<()> {
    let mut next_tick = Instant::now() + app.interval;
    app.tick();
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        let timeout = next_tick.saturating_duration_since(Instant::now());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(());
                    }
                    KeyCode::Char('t') => {
                        current = (current + 1) % themes.len();
                        app.theme = themes[current].clone();
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if Instant::now() >= next_tick {
            app.tick();
            next_tick += app.interval;
            if next_tick < Instant::now() {
                next_tick = Instant::now() + app.interval;
            }
        }
    }
}
