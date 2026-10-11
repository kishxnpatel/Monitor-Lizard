//! Real monitor names ("DELL U2720Q") instead of "Generic PnP Monitor".
//!
//! Windows only gives us `\\.\DISPLAY1`-style names when listing monitors.
//! The Display Config API can map those to the name stored in the
//! monitor's EDID, which is what people actually recognise.

use std::collections::HashMap;
use std::mem::size_of;

use windows_sys::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;

/// Map of GDI device name (`\\.\DISPLAY1`) -> friendly monitor name.
/// Returns an empty map if anything fails; callers fall back gracefully.
pub fn friendly_names() -> HashMap<String, String> {
    targets().into_iter().map(|t| (t.gdi, t.name)).collect()
}

/// Map of GDI device name -> monitor device path
/// (`\\?\DISPLAY#SDC4161#4&...#{guid}`), used to find the monitor's EDID.
pub fn device_paths() -> HashMap<String, String> {
    targets().into_iter().map(|t| (t.gdi, t.path)).collect()
}

struct Target {
    gdi: String,
    name: String,
    path: String,
}

/// Every active monitor Windows is driving, with its names.
fn targets() -> Vec<Target> {
    let mut out = Vec::new();
    unsafe {
        let mut path_count = 0u32;
        let mut mode_count = 0u32;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            != ERROR_SUCCESS
        {
            return out;
        }

        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            std::ptr::null_mut(),
        ) != ERROR_SUCCESS
        {
            return out;
        }
        paths.truncate(path_count as usize);

        for path in &paths {
            // Which \\.\DISPLAYn this path drives
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }

            // The monitor plugged into it
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            target.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            target.header.adapterId = path.targetInfo.adapterId;
            target.header.id = path.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut target.header) != 0 {
                continue;
            }

            let gdi = wide_to_string(&source.viewGdiDeviceName);
            let name = wide_to_string(&target.monitorFriendlyDeviceName);
            let path = wide_to_string(&target.monitorDevicePath);
            if !gdi.is_empty() {
                out.push(Target { gdi, name, path });
            }
        }
    }
    out
}

/// Fixed-size UTF-16 buffer from Windows -> Rust String.
pub fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().to_string()
}
