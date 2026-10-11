//! Monitor Lizard command-line tool.
//!
//!   monitor-lizard list
//!   monitor-lizard set 1 --brightness 40
//!   monitor-lizard set all --brightness 70 --contrast 60
//!   monitor-lizard set 2 --input hdmi1

use std::process::ExitCode;

use monitor_lizard::{vcp, Monitors};

const HELP: &str = "\
Monitor Lizard - the monitor settings Windows forgot about.

USAGE:
  monitor-lizard list
  monitor-lizard set <number|all> [--brightness 0-100] [--contrast 0-100] [--input NAME]
  monitor-lizard inputs
  monitor-lizard help

EXAMPLES:
  monitor-lizard set 1 --brightness 40
  monitor-lizard set all --brightness 70 --contrast 60
  monitor-lizard set 2 --input hdmi1

INPUT NAMES:
  hdmi1, hdmi2, dp1, dp2, usbc, dvi1, vga1, or a raw value like 0x11
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("list") | Some("ls") => list(),
        Some("set") => set(&args[1..]),
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

fn set(args: &[String]) -> Result<(), String> {
    let which = args
        .first()
        .ok_or("Which monitor? e.g. `monitor-lizard set 1 --brightness 50`")?;

    let mut brightness = None;
    let mut contrast = None;
    let mut input = None;

    let mut rest = args[1..].iter();
    while let Some(flag) = rest.next() {
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
            _ => return Err(format!("Unknown option '{flag}'. Try `monitor-lizard help`.")),
        }
    }

    if brightness.is_none() && contrast.is_none() && input.is_none() {
        return Err("Nothing to change. Add --brightness, --contrast or --input.".into());
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

    if failed {
        Err("Some changes didn't apply.".into())
    } else {
        Ok(())
    }
}

fn percent(value: &str) -> Result<u8, String> {
    match value.trim_end_matches('%').parse::<u8>() {
        Ok(v) if v <= 100 => Ok(v),
        _ => Err(format!("'{value}' should be a number from 0 to 100")),
    }
}
