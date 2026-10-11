//! Monitor Lizard command-line tool.
//!
//!   monitor-lizard list
//!   monitor-lizard set 1 --brightness 40
//!   monitor-lizard set all --brightness 70 --contrast 60
//!   monitor-lizard set 2 --input hdmi1
//!   monitor-lizard set 1 --resolution 1920x1080 --refresh 60 --rotate portrait

use std::io::Write;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use monitor_lizard::advanced::{self, Projection};
use monitor_lizard::display::{self, Change, Rotation};
use monitor_lizard::{vcp, Monitors};

/// How long a display change waits for "yes" before snapping back.
const KEEP_SECONDS: u64 = 15;

const HELP: &str = "\
Monitor Lizard - the monitor settings Windows forgot about.

USAGE:
  monitor-lizard list
  monitor-lizard modes [number]
  monitor-lizard layout
  monitor-lizard hdr <number> [on|off]
  monitor-lizard primary <number>
  monitor-lizard project [extend|duplicate|internal|external]
  monitor-lizard colors <number> [PROFILE NAME]
  monitor-lizard set <number|all> [options]
  monitor-lizard inputs
  monitor-lizard help

SET OPTIONS:
  --brightness 0-100
  --contrast 0-100        (external monitors)
  --input NAME            (external monitors)
  --resolution WxH        e.g. 1920x1080
  --refresh HZ            e.g. 60
  --rotate NAME           landscape, portrait, landscape-flipped, portrait-flipped
  --yes                   keep display changes without asking

EXAMPLES:
  monitor-lizard set 1 --brightness 40
  monitor-lizard set all --brightness 70 --contrast 60
  monitor-lizard set 2 --input hdmi1
  monitor-lizard set 1 --resolution 1920x1080 --refresh 60
  monitor-lizard set 2 --rotate portrait

Display changes (resolution, refresh, rotate) snap back after 15 seconds
unless you press Y, so a bad setting can't leave you stuck.

INPUT NAMES:
  hdmi1, hdmi2, dp1, dp2, usbc, dvi1, vga1, or a raw value like 0x11
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("list") | Some("ls") => list(),
        Some("set") => set(&args[1..]),
        Some("modes") => modes(args.get(1).map(String::as_str)),
        Some("layout") => layout(),
        Some("hdr") => hdr(&args[1..]),
        Some("primary") => primary(&args[1..]),
        Some("project") => project(&args[1..]),
        Some("colors") | Some("colours") => colors(&args[1..]),
        Some("doctor") => doctor(),
        Some("inputs") => {
            for (value, name) in vcp::INPUTS {
                println!("  0x{value:02X}  {name}");
            }
            Ok(())
        }
        Some("help") | Some("-h") | Some("--help") => {
            print!("{HELP}");
            Ok(())
        }
        Some("--version") | Some("-V") => {
            println!("monitor-lizard {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => Err(format!("Unknown command '{other}'. Try `monitor-lizard help`.")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("🦎 {e}");
            ExitCode::FAILURE
        }
    }
}

fn list() -> Result<(), String> {
    let monitors = Monitors::detect();
    if monitors.0.is_empty() {
        return Err("No monitors found.".into());
    }

    for m in &monitors.0 {
        let primary = if m.primary { "  (primary)" } else { "" };
        println!("{}  {}  {}x{}{}", m.index, m.name, m.width, m.height, primary);
        if let Ok(mode) = display::current(&m.device) {
            println!("   {}Hz   {}", mode.refresh, mode.rotation.name());
        }

        if m.builtin {
            let b = m.brightness().map(|v| format!("{v}%")).unwrap_or("-".into());
            println!("   brightness {b}   (built-in screen: brightness only)");
            continue;
        }
        if !m.supports_ddc() {
            println!("   DDC/CI not available (turn on DDC/CI in the monitor menu, or your dock may block it)");
            continue;
        }
        let brightness = m.brightness().map(|v| format!("{v}%")).unwrap_or("-".into());
        let contrast = m.contrast().map(|v| format!("{v}%")).unwrap_or("-".into());
        let input = m.input().map(vcp::input_name).unwrap_or("-".into());
        println!("   brightness {brightness}   contrast {contrast}   input {input}");
    }
    Ok(())
}

/// Troubleshooting: shows exactly what each check returns.
fn doctor() -> Result<(), String> {
    println!("Monitor Lizard {} doctor", env!("CARGO_PKG_VERSION"));
    match monitor_lizard::builtin_screen_check() {
        Ok(v) => println!("  built-in screen (WMI): OK, brightness {v}%"),
        Err(e) => println!("  built-in screen (WMI): {e}"),
    }
    for m in &Monitors::detect().0 {
        let ddc = match m.brightness() {
            Ok(v) => format!("OK, brightness {v}%"),
            Err(e) => e,
        };
        println!("  {} {} [{}] builtin={}  {}", m.index, m.name, m.device, m.builtin, ddc);
    }
    Ok(())
}

fn one_monitor<'a>(monitors: &'a Monitors, which: Option<&String>) -> Result<&'a monitor_lizard::Monitor, String> {
    let which = which.ok_or("Which monitor? Add its number, e.g. 1")?;
    let list = monitors.select(which)?;
    list.into_iter().next().ok_or_else(|| "Pick one monitor, not 'all'.".into())
}

/// Show or switch HDR.
fn hdr(args: &[String]) -> Result<(), String> {
    let monitors = Monitors::detect();
    let m = one_monitor(&monitors, args.first())?;
    let state = advanced::hdr(&m.device)?;
    if !state.supported {
        return Err(format!("{} doesn't support HDR.", m.name));
    }
    match args.get(1).map(|s| s.to_ascii_lowercase()) {
        None => {
            println!("{}  {}: HDR {}", m.index, m.name, if state.enabled { "on" } else { "off" });
            Ok(())
        }
        Some(v) if v == "on" || v == "off" => {
            advanced::set_hdr(&m.device, v == "on")?;
            println!("{}  {}: HDR {v}", m.index, m.name);
            Ok(())
        }
        Some(v) => Err(format!("'{v}' should be on or off.")),
    }
}

/// Make a monitor the main display.
fn primary(args: &[String]) -> Result<(), String> {
    let monitors = Monitors::detect();
    let m = one_monitor(&monitors, args.first())?;
    if m.primary {
        println!("{}  {} is already the main display.", m.index, m.name);
        return Ok(());
    }
    let devices: Vec<String> = monitor_lizard::layout::read(&monitors).into_iter().map(|s| s.device).collect();
    advanced::make_primary(&m.device, &devices)?;
    println!("{}  {} is now the main display.", m.index, m.name);
    Ok(())
}

/// Win+P modes: extend, duplicate, one screen only.
fn project(args: &[String]) -> Result<(), String> {
    match args.first() {
        None => {
            match advanced::projection() {
                Some(p) => println!("{}", p.name()),
                None => println!("Only one screen is connected."),
            }
            Ok(())
        }
        Some(v) => {
            let mode = Projection::parse(v)
                .ok_or_else(|| format!("'{v}' isn't a mode. Use extend, duplicate, internal or external."))?;
            advanced::set_projection(mode)?;
            println!("{}", mode.name());
            Ok(())
        }
    }
}

/// List colour profiles, or set one.
fn colors(args: &[String]) -> Result<(), String> {
    let monitors = Monitors::detect();
    let m = one_monitor(&monitors, args.first())?;
    if args.len() > 1 {
        let name = args[1..].join(" ");
        advanced::set_color_profile(&m.device, &name)?;
        println!("{}  {}: color profile {name}", m.index, m.name);
        return Ok(());
    }
    let (list, current) = advanced::color_profiles(&m.device)?;
    if list.is_empty() {
        println!("{}  {}: no color profiles installed (Windows uses its default).", m.index, m.name);
    }
    for p in list {
        let mark = if current.as_deref() == Some(p.as_str()) { "  <- in use" } else { "" };
        println!("   {p}{mark}");
    }
    Ok(())
}

/// Where each screen sits, plus its real size.
fn layout() -> Result<(), String> {
    let screens = monitor_lizard::layout::read(&Monitors::detect());
    if screens.is_empty() {
        return Err("No monitors found.".into());
    }
    println!("Arrangement (pixels, primary at 0,0)");
    for s in &screens {
        let size = match (s.inches(), s.size_mm) {
            (Some(inches), Some((w, h))) => format!("{inches:.1}\"  ({w}x{h} mm)"),
            _ => "size unknown".into(),
        };
        let primary = if s.primary { "  (primary)" } else { "" };
        println!(
            "{}  {}  {}x{} at ({}, {})   {}{}",
            s.index, s.name, s.width, s.height, s.x, s.y, size, primary
        );
    }
    Ok(())
}

/// Supported resolutions and refresh rates.
fn modes(which: Option<&str>) -> Result<(), String> {
    let monitors = Monitors::detect();
    for m in monitors.select(which.unwrap_or("all"))? {
        println!("{}  {}", m.index, m.name);
        let now = display::current(&m.device).ok();
        for r in display::resolutions(&m.device) {
            let is_now = now.as_ref().is_some_and(|n| {
                (n.width.max(n.height), n.width.min(n.height)) == (r.width, r.height)
            });
            let mark = if is_now { "  <- current" } else { "" };
            println!("   {}x{}   {}{}", r.width, r.height, display::rates(&r.refresh_rates), mark);
        }
    }
    Ok(())
}

fn set(args: &[String]) -> Result<(), String> {
    let which = args
        .first()
        .ok_or("Which monitor? e.g. `monitor-lizard set 1 --brightness 50`")?;

    let mut brightness = None;
    let mut contrast = None;
    let mut input = None;
    let mut change = Change::default();
    let mut yes = false;

    let mut rest = args[1..].iter();
    while let Some(flag) = rest.next() {
        if flag == "--yes" || flag == "-y" {
            yes = true;
            continue;
        }
        let value = rest
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--brightness" | "-b" => brightness = Some(percent(value)?),
            "--contrast" | "-c" => contrast = Some(percent(value)?),
            "--input" | "-i" => {
                input = Some(vcp::parse_input(value).ok_or_else(|| {
                    format!("Unknown input '{value}'. Run `monitor-lizard inputs`.")
                })?)
            }
            "--resolution" | "-r" => {
                change.resolution = Some(display::parse_resolution(value).ok_or_else(|| {
                    format!("'{value}' isn't a resolution. Use WIDTHxHEIGHT, e.g. 1920x1080.")
                })?)
            }
            "--refresh" | "--hz" => {
                change.refresh = Some(value.trim_end_matches("Hz").trim_end_matches("hz").parse().map_err(
                    |_| format!("'{value}' isn't a refresh rate. Use a number like 60."),
                )?)
            }
            "--rotate" => {
                change.rotation = Some(Rotation::parse(value).ok_or_else(|| {
                    format!("'{value}' isn't a rotation. Use landscape, portrait, landscape-flipped or portrait-flipped.")
                })?)
            }
            _ => return Err(format!("Unknown option '{flag}'. Try `monitor-lizard help`.")),
        }
    }

    if brightness.is_none() && contrast.is_none() && input.is_none() && change.is_empty() {
        return Err("Nothing to change. Try `monitor-lizard help` for options.".into());
    }

    let monitors = Monitors::detect();
    let targets = monitors.select(which)?;
    let mut failed = false;

    for m in targets {
        let mut report = |what: &str, r: Result<(), String>| match r {
            Ok(()) => println!("{}  {}: {what}", m.index, m.name),
            Err(e) => {
                failed = true;
                eprintln!("🦎 {e}");
            }
        };
        if let Some(v) = brightness {
            report(&format!("brightness {v}%"), m.set_brightness(v));
        }
        if let Some(v) = contrast {
            report(&format!("contrast {v}%"), m.set_contrast(v));
        }
        if let Some(v) = input {
            report(&format!("input {}", vcp::input_name(v)), m.set_input(v));
        }
    }

    if !change.is_empty() && !apply_display(&monitors, which, &change, yes)? {
        failed = true;
    }

    if failed {
        Err("Some changes didn't apply.".into())
    } else {
        Ok(())
    }
}

/// Apply resolution / refresh / rotation, then revert unless confirmed.
/// Returns Ok(false) only if something failed. Reverting is not a failure.
fn apply_display(monitors: &Monitors, which: &str, change: &Change, yes: bool) -> Result<bool, String> {
    let mut devices: Vec<(usize, String, String)> = Vec::new();
    for m in monitors.select(which)? {
        if !devices.iter().any(|(_, _, d)| *d == m.device) {
            devices.push((m.index, m.name.clone(), m.device.clone()));
        }
    }

    // Plan and test everything first, so nothing changes if any of it is invalid.
    let mut planned = Vec::new();
    for (index, name, device) in &devices {
        let before = display::current(device)?;
        let after = display::plan(device, change).map_err(|e| format!("{index} {name}: {e}"))?;
        display::test(device, &after).map_err(|e| format!("{index} {name}: {e}"))?;
        planned.push((index, name, device, before, after));
    }

    let mut applied = Vec::new();
    let mut ok = true;
    for (index, name, device, before, after) in &planned {
        match display::apply(device, after) {
            Ok(()) => {
                println!(
                    "{index}  {name}: {}x{} {}Hz {}",
                    after.width, after.height, after.refresh, after.rotation.name()
                );
                applied.push((device, before));
            }
            Err(e) => {
                ok = false;
                eprintln!("🦎 {index} {name}: {e}");
            }
        }
    }

    if applied.is_empty() || yes || confirm_keep() {
        return Ok(ok);
    }

    for (device, before) in applied {
        if let Err(e) = display::apply(device, before) {
            eprintln!("🦎 Couldn't revert {device}: {e}");
        }
    }
    println!("Reverted. Nothing changed.");
    Ok(ok)
}

/// "Keep these settings?" with a countdown. Anything but Y means no.
fn confirm_keep() -> bool {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let _ = tx.send(line);
    });

    for left in (1..=KEEP_SECONDS).rev() {
        print!("\rKeep these display settings? [y/N]  reverting in {left:>2}s ");
        let _ = std::io::stdout().flush();
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(answer) => {
                println!();
                return answer.trim().eq_ignore_ascii_case("y")
                    || answer.trim().eq_ignore_ascii_case("yes");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
    println!();
    false
}

fn percent(value: &str) -> Result<u8, String> {
    match value.trim_end_matches('%').parse::<u8>() {
        Ok(v) if v <= 100 => Ok(v),
        _ => Err(format!("'{value}' should be a number from 0 to 100")),
    }
}
