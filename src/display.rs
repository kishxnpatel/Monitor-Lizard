//! Windows display settings: resolution, refresh rate, rotation.
//!
//! These work on every screen, laptop or external, because Windows itself
//! handles them (no DDC/CI needed). Changes are tested first, and the
//! command-line tool can revert them if the user doesn't confirm.

use std::mem::size_of;

use windows_sys::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, EnumDisplaySettingsW, CDS_TEST, CDS_UPDATEREGISTRY, DEVMODEW,
    DISP_CHANGE_RESTART, DISP_CHANGE_SUCCESSFUL, DMDO_180, DMDO_270, DMDO_90, DMDO_DEFAULT,
    DM_DISPLAYFREQUENCY, DM_DISPLAYORIENTATION, DM_PELSHEIGHT, DM_PELSWIDTH,
    ENUM_CURRENT_SETTINGS,
};

/// The four orientations, named like Windows Settings names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    Landscape,
    Portrait,
    LandscapeFlipped,
    PortraitFlipped,
}

impl Rotation {
    pub fn parse(text: &str) -> Option<Rotation> {
        let t = text.trim().to_ascii_lowercase().replace([' ', '-', '_', '(', ')'], "");
        Some(match t.as_str() {
            "landscape" | "0" | "normal" => Rotation::Landscape,
            "portrait" | "90" => Rotation::Portrait,
            "landscapeflipped" | "180" | "flipped" | "upsidedown" => Rotation::LandscapeFlipped,
            "portraitflipped" | "270" => Rotation::PortraitFlipped,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Rotation::Landscape => "Landscape",
            Rotation::Portrait => "Portrait",
            Rotation::LandscapeFlipped => "Landscape (flipped)",
            Rotation::PortraitFlipped => "Portrait (flipped)",
        }
    }

    /// Portrait modes swap width and height.
    pub fn is_sideways(self) -> bool {
        matches!(self, Rotation::Portrait | Rotation::PortraitFlipped)
    }

    fn from_raw(v: u32) -> Rotation {
        match v {
            DMDO_90 => Rotation::Portrait,
            DMDO_180 => Rotation::LandscapeFlipped,
            DMDO_270 => Rotation::PortraitFlipped,
            _ => Rotation::Landscape,
        }
    }

    fn raw(self) -> u32 {
        match self {
            Rotation::Landscape => DMDO_DEFAULT,
            Rotation::Portrait => DMDO_90,
            Rotation::LandscapeFlipped => DMDO_180,
            Rotation::PortraitFlipped => DMDO_270,
        }
    }
}

/// What a screen is set to right now. Width/height are as seen on screen
/// (so a portrait screen reports e.g. 1536x2304).
#[derive(Clone, Copy)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
    pub rotation: Rotation,
    raw: DEVMODEW,
}

/// One supported resolution, always in landscape terms (wide x tall),
/// with every refresh rate it supports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
    pub refresh_rates: Vec<u32>,
}

/// What to change. `None` means "leave as is".
#[derive(Clone, Copy, Debug, Default)]
pub struct Change {
    /// In landscape terms, e.g. 1920x1080 even when rotating to portrait.
    pub resolution: Option<(u32, u32)>,
    pub refresh: Option<u32>,
    pub rotation: Option<Rotation>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.resolution.is_none() && self.refresh.is_none() && self.rotation.is_none()
    }
}

fn wide(device: &str) -> Vec<u16> {
    device.encode_utf16().chain(std::iter::once(0)).collect()
}

fn blank_devmode() -> DEVMODEW {
    DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() }
}

/// Current settings for a screen like `\\.\DISPLAY1`.
pub fn current(device: &str) -> Result<DisplayMode, String> {
    let name = wide(device);
    let mut dm = blank_devmode();
    if unsafe { EnumDisplaySettingsW(name.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) } == 0 {
        return Err(format!("Couldn't read display settings for {device}."));
    }
    let rotation = Rotation::from_raw(unsafe { dm.Anonymous1.Anonymous2.dmDisplayOrientation });
    Ok(DisplayMode {
        width: dm.dmPelsWidth,
        height: dm.dmPelsHeight,
        refresh: dm.dmDisplayFrequency,
        rotation,
        raw: dm,
    })
}

/// Every resolution the screen supports, biggest first.
pub fn resolutions(device: &str) -> Vec<Resolution> {
    let name = wide(device);
    let mut list: Vec<Resolution> = Vec::new();
    let mut i = 0u32;
    loop {
        let mut dm = blank_devmode();
        if unsafe { EnumDisplaySettingsW(name.as_ptr(), i, &mut dm) } == 0 {
            break;
        }
        i += 1;
        if dm.dmBitsPerPel < 32 {
            continue; // skip old low-colour modes
        }
        // Normalise to landscape so rotated screens list the same modes
        let (w, h) = landscape(dm.dmPelsWidth, dm.dmPelsHeight);
        let hz = dm.dmDisplayFrequency;
        match list.iter_mut().find(|r| r.width == w && r.height == h) {
            Some(r) => {
                if !r.refresh_rates.contains(&hz) {
                    r.refresh_rates.push(hz);
                }
            }
            None => list.push(Resolution { width: w, height: h, refresh_rates: vec![hz] }),
        }
    }
    for r in &mut list {
        r.refresh_rates.sort_unstable_by(|a, b| b.cmp(a));
    }
    list.sort_by_key(|r| std::cmp::Reverse((r.width * r.height, r.width)));
    list
}

fn landscape(w: u32, h: u32) -> (u32, u32) {
    if w >= h { (w, h) } else { (h, w) }
}

/// Build the new settings from the current ones plus a change, checking
/// that the screen actually supports the result.
pub fn plan(device: &str, change: &Change) -> Result<DisplayMode, String> {
    let now = current(device)?;
    let rotation = change.rotation.unwrap_or(now.rotation);
    let (cur_w, cur_h) = landscape(now.width, now.height);
    let (w, h) = change.resolution.unwrap_or((cur_w, cur_h));
    let (w, h) = landscape(w, h);

    let supported = resolutions(device);
    let res = supported
        .iter()
        .find(|r| r.width == w && r.height == h)
        .ok_or_else(|| format!("{w}x{h} isn't supported here. Run `monitor-lizard modes` to see options."))?;

    // Keep the current refresh rate if the new resolution supports it,
    // otherwise pick the fastest one it does support.
    let refresh = match change.refresh {
        Some(hz) if res.refresh_rates.contains(&hz) => hz,
        Some(hz) => {
            return Err(format!(
                "{hz}Hz isn't supported at {w}x{h}. Options: {}",
                rates(&res.refresh_rates)
            ))
        }
        None if res.refresh_rates.contains(&now.refresh) => now.refresh,
        None => res.refresh_rates[0],
    };

    let (out_w, out_h) = if rotation.is_sideways() { (h, w) } else { (w, h) };
    let mut dm = now.raw;
    dm.dmPelsWidth = out_w;
    dm.dmPelsHeight = out_h;
    dm.dmDisplayFrequency = refresh;
    dm.Anonymous1.Anonymous2.dmDisplayOrientation = rotation.raw();
    dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY | DM_DISPLAYORIENTATION;

    Ok(DisplayMode { width: out_w, height: out_h, refresh, rotation, raw: dm })
}

/// Ask Windows whether a mode would work, without applying it.
pub fn test(device: &str, mode: &DisplayMode) -> Result<(), String> {
    change(device, mode, CDS_TEST)
}

/// Apply a mode and save it, so it sticks after a restart.
pub fn apply(device: &str, mode: &DisplayMode) -> Result<(), String> {
    change(device, mode, CDS_UPDATEREGISTRY)
}

fn change(device: &str, mode: &DisplayMode, flags: u32) -> Result<(), String> {
    let name = wide(device);
    let result = unsafe {
        ChangeDisplaySettingsExW(
            name.as_ptr(),
            &mode.raw,
            std::ptr::null_mut(),
            flags,
            std::ptr::null(),
        )
    };
    match result {
        DISP_CHANGE_SUCCESSFUL => Ok(()),
        DISP_CHANGE_RESTART => Err("Windows says this needs a restart.".into()),
        code => Err(format!("Windows refused that display change (code {code}).")),
    }
}

/// "144, 120, 60Hz"
pub fn rates(list: &[u32]) -> String {
    let parts: Vec<String> = list.iter().map(|r| r.to_string()).collect();
    format!("{}Hz", parts.join(", "))
}

/// Parse "1920x1080", "1920*1080" or "1920 x 1080".
pub fn parse_resolution(text: &str) -> Option<(u32, u32)> {
    let t = text.to_ascii_lowercase().replace(' ', "");
    let (w, h) = t.split_once(['x', '*', '×'])?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rotation() {
        assert_eq!(Rotation::parse("portrait"), Some(Rotation::Portrait));
        assert_eq!(Rotation::parse("90"), Some(Rotation::Portrait));
        assert_eq!(Rotation::parse("Landscape (flipped)"), Some(Rotation::LandscapeFlipped));
        assert_eq!(Rotation::parse("270"), Some(Rotation::PortraitFlipped));
        assert_eq!(Rotation::parse("diagonal"), None);
    }

    #[test]
    fn parses_resolution() {
        assert_eq!(parse_resolution("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_resolution("2304 × 1536"), Some((2304, 1536)));
        assert_eq!(parse_resolution("1920*1080"), Some((1920, 1080)));
        assert_eq!(parse_resolution("big"), None);
    }

    #[test]
    fn landscape_normalises() {
        assert_eq!(landscape(1536, 2304), (2304, 1536));
        assert_eq!(landscape(1920, 1080), (1920, 1080));
    }
}
