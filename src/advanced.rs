//! The "Settings" rows that have official Windows APIs:
//! HDR, multiple-display mode (Extend / Duplicate / one screen only),
//! main display, and colour profiles.

use std::mem::size_of;

use windows_sys::core::PWSTR;
use windows_sys::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo, GetDisplayConfigBufferSizes,
    QueryDisplayConfig, SetDisplayConfig, DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE,
    DISPLAYCONFIG_TOPOLOGY_CLONE, DISPLAYCONFIG_TOPOLOGY_EXTEND, DISPLAYCONFIG_TOPOLOGY_EXTERNAL,
    DISPLAYCONFIG_TOPOLOGY_INTERNAL, QDC_DATABASE_CURRENT, SDC_APPLY, SDC_TOPOLOGY_CLONE,
    SDC_TOPOLOGY_EXTEND, SDC_TOPOLOGY_EXTERNAL, SDC_TOPOLOGY_INTERNAL,
};
use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
use windows_sys::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, EnumDisplaySettingsW, CDS_NORESET, CDS_SET_PRIMARY,
    CDS_UPDATEREGISTRY, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_POSITION, ENUM_CURRENT_SETTINGS,
};
use windows_sys::Win32::UI::ColorSystem::{
    ColorProfileGetDisplayDefault, ColorProfileGetDisplayList,
    ColorProfileSetDisplayDefaultAssociation, CPST_EXTENDED_DISPLAY_COLOR_MODE,
    CPST_STANDARD_DISPLAY_COLOR_MODE, CPT_ICC, WCS_PROFILE_MANAGEMENT_SCOPE_CURRENT_USER,
};

use crate::names::{ids, wide_to_string, Ids};

fn ids_for(device: &str) -> Result<Ids, String> {
    ids().get(device).copied().ok_or_else(|| format!("Windows doesn't list {device} as active."))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// ─── HDR ──────────────────────────────────────────────────────────────

/// HDR ("advanced color") support and state for one screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hdr {
    pub supported: bool,
    pub enabled: bool,
}

pub fn hdr(device: &str) -> Result<Hdr, String> {
    let id = ids_for(device)?;
    let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
    info.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
    info.header.size = size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
    info.header.adapterId = id.adapter;
    info.header.id = id.target;
    if unsafe { DisplayConfigGetDeviceInfo(&mut info.header) } != 0 {
        return Err("Couldn't read HDR state.".into());
    }
    let bits = unsafe { info.Anonymous.value };
    Ok(Hdr {
        supported: bits & 0b0001 != 0, // advancedColorSupported
        enabled: bits & 0b0010 != 0,   // advancedColorEnabled
    })
}

pub fn set_hdr(device: &str, on: bool) -> Result<(), String> {
    let id = ids_for(device)?;
    let mut state = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE::default();
    state.header.r#type = DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE;
    state.header.size = size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32;
    state.header.adapterId = id.adapter;
    state.header.id = id.target;
    state.Anonymous.value = on as u32; // bit 0: enableAdvancedColor
    match unsafe { DisplayConfigSetDeviceInfo(&state.header) } {
        0 => Ok(()),
        code => Err(format!("Windows refused to change HDR (code {code}).")),
    }
}

// ─── Multiple displays ────────────────────────────────────────────────

/// The Win+P modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// "PC screen only" / show only on the built-in screen
    InternalOnly,
    /// "Duplicate these displays"
    Duplicate,
    /// "Extend these displays"
    Extend,
    /// "Second screen only" / show only on the external screen
    ExternalOnly,
}

impl Projection {
    pub const ALL: [Projection; 4] = [
        Projection::Duplicate,
        Projection::Extend,
        Projection::InternalOnly,
        Projection::ExternalOnly,
    ];

    /// Names as Windows Settings shows them.
    pub fn name(self) -> &'static str {
        match self {
            Projection::Duplicate => "Duplicate these displays",
            Projection::Extend => "Extend these displays",
            Projection::InternalOnly => "Show only on main screen",
            Projection::ExternalOnly => "Show only on second screen",
        }
    }

    pub fn parse(text: &str) -> Option<Projection> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "extend" => Projection::Extend,
            "duplicate" | "clone" | "mirror" => Projection::Duplicate,
            "internal" | "pc" | "pconly" | "main" => Projection::InternalOnly,
            "external" | "second" | "secondonly" => Projection::ExternalOnly,
            _ => return None,
        })
    }
}

/// Current mode. Windows only reports this when more than one screen
/// has been connected, so `None` is normal on a lone laptop.
pub fn projection() -> Option<Projection> {
    unsafe {
        let mut paths = 0u32;
        let mut modes = 0u32;
        if GetDisplayConfigBufferSizes(QDC_DATABASE_CURRENT, &mut paths, &mut modes) != ERROR_SUCCESS {
            return None;
        }
        let mut path_buf = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
        let mut mode_buf = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        let mut topology = 0;
        if QueryDisplayConfig(
            QDC_DATABASE_CURRENT,
            &mut paths,
            path_buf.as_mut_ptr(),
            &mut modes,
            mode_buf.as_mut_ptr(),
            &mut topology,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        Some(match topology {
            DISPLAYCONFIG_TOPOLOGY_INTERNAL => Projection::InternalOnly,
            DISPLAYCONFIG_TOPOLOGY_CLONE => Projection::Duplicate,
            DISPLAYCONFIG_TOPOLOGY_EXTEND => Projection::Extend,
            DISPLAYCONFIG_TOPOLOGY_EXTERNAL => Projection::ExternalOnly,
            _ => return None,
        })
    }
}

/// Same as pressing Win+P and picking a mode.
pub fn set_projection(mode: Projection) -> Result<(), String> {
    let topology = match mode {
        Projection::InternalOnly => SDC_TOPOLOGY_INTERNAL,
        Projection::Duplicate => SDC_TOPOLOGY_CLONE,
        Projection::Extend => SDC_TOPOLOGY_EXTEND,
        Projection::ExternalOnly => SDC_TOPOLOGY_EXTERNAL,
    };
    let code = unsafe {
        SetDisplayConfig(0, std::ptr::null(), 0, std::ptr::null(), SDC_APPLY | topology)
    };
    match code {
        0 => Ok(()),
        code => Err(format!("Windows refused that mode (code {code}). Is a second screen connected?")),
    }
}

// ─── Main display ─────────────────────────────────────────────────────

/// Make `device` the main display. Windows requires the main display to
/// sit at (0,0), so every screen shifts by the same amount and the
/// arrangement looks exactly the same afterwards.
pub fn make_primary(device: &str, all_devices: &[String]) -> Result<(), String> {
    let mut modes = Vec::new();
    for d in all_devices {
        let name = wide(d);
        let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        if unsafe { EnumDisplaySettingsW(name.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) } == 0 {
            return Err(format!("Couldn't read {d}."));
        }
        modes.push((d.clone(), name, dm));
    }
    let (ox, oy) = modes
        .iter()
        .find(|(d, _, _)| d == device)
        .map(|(_, _, dm)| unsafe {
            let p = dm.Anonymous1.Anonymous2.dmPosition;
            (p.x, p.y)
        })
        .ok_or_else(|| format!("{device} isn't an active display."))?;

    // Queue every screen, then commit them together: one flicker, not three.
    for (d, name, dm) in &mut modes {
        unsafe {
            let p = &mut dm.Anonymous1.Anonymous2.dmPosition;
            p.x -= ox;
            p.y -= oy;
        }
        dm.dmFields = DM_POSITION;
        let mut flags = CDS_UPDATEREGISTRY | CDS_NORESET;
        if d == device {
            flags |= CDS_SET_PRIMARY;
        }
        let r = unsafe {
            ChangeDisplaySettingsExW(name.as_ptr(), dm, std::ptr::null_mut(), flags, std::ptr::null())
        };
        if r != DISP_CHANGE_SUCCESSFUL {
            return Err(format!("Windows refused to move {d} (code {r})."));
        }
    }
    let r = unsafe {
        ChangeDisplaySettingsExW(std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), 0, std::ptr::null())
    };
    if r != DISP_CHANGE_SUCCESSFUL {
        return Err(format!("Windows refused the new main display (code {r})."));
    }
    Ok(())
}

// ─── Colour profiles ──────────────────────────────────────────────────

/// HDR screens keep a separate profile list from SDR ones.
fn subtype(hdr_on: bool) -> i32 {
    if hdr_on { CPST_EXTENDED_DISPLAY_COLOR_MODE } else { CPST_STANDARD_DISPLAY_COLOR_MODE }
}

/// Profiles installed for this screen, plus the one in use.
pub fn color_profiles(device: &str) -> Result<(Vec<String>, Option<String>), String> {
    let id = ids_for(device)?;
    let hdr_on = hdr(device).map(|h| h.enabled).unwrap_or(false);
    let mut list: Vec<String> = Vec::new();
    unsafe {
        let mut items: *mut PWSTR = std::ptr::null_mut();
        let mut count = 0u32;
        let hr = ColorProfileGetDisplayList(
            WCS_PROFILE_MANAGEMENT_SCOPE_CURRENT_USER,
            id.adapter,
            id.source,
            &mut items,
            &mut count,
        );
        if hr < 0 {
            return Err(format!("Couldn't read color profiles (0x{hr:08X})."));
        }
        if !items.is_null() {
            for i in 0..count as usize {
                let p = *items.add(i);
                if !p.is_null() {
                    list.push(pwstr_to_string(p));
                }
            }
            LocalFree(items.cast());
        }

        let mut current: PWSTR = std::ptr::null_mut();
        let hr = ColorProfileGetDisplayDefault(
            WCS_PROFILE_MANAGEMENT_SCOPE_CURRENT_USER,
            id.adapter,
            id.source,
            CPT_ICC,
            subtype(hdr_on),
            &mut current,
        );
        let current = (hr >= 0 && !current.is_null()).then(|| {
            let s = pwstr_to_string(current);
            LocalFree(current.cast());
            s
        });
        Ok((list, current))
    }
}

pub fn set_color_profile(device: &str, profile: &str) -> Result<(), String> {
    let id = ids_for(device)?;
    let hdr_on = hdr(device).map(|h| h.enabled).unwrap_or(false);
    let name = wide(profile);
    let hr = unsafe {
        ColorProfileSetDisplayDefaultAssociation(
            WCS_PROFILE_MANAGEMENT_SCOPE_CURRENT_USER,
            name.as_ptr(),
            CPT_ICC,
            subtype(hdr_on),
            id.adapter,
            id.source,
        )
    };
    if hr < 0 {
        return Err(format!("Windows refused that color profile (0x{hr:08X})."));
    }
    Ok(())
}

unsafe fn pwstr_to_string(p: PWSTR) -> String {
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    wide_to_string(std::slice::from_raw_parts(p, len))
}

#[cfg(test)]
mod tests {
    use super::Projection;

    #[test]
    fn parses_projection() {
        assert_eq!(Projection::parse("extend"), Some(Projection::Extend));
        assert_eq!(Projection::parse("Duplicate"), Some(Projection::Duplicate));
        assert_eq!(Projection::parse("second"), Some(Projection::ExternalOnly));
        assert_eq!(Projection::parse("pc"), Some(Projection::InternalOnly));
        assert_eq!(Projection::parse("sideways"), None);
    }
}
