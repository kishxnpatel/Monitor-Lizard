//! Monitor arrangement: where each screen sits, how big it is in pixels,
//! and how big it really is on your desk.
//!
//! The drag-and-drop canvas draws boxes at their *physical* size (so a
//! 27" monitor looks bigger than a 13" laptop), while positions are kept
//! in *pixels*, because that's what Windows uses for the mouse.

use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY};

use crate::display;
use crate::monitor::Monitors;
use crate::names::device_paths;

/// One screen in the arrangement.
#[derive(Clone, Debug)]
pub struct Screen {
    pub index: usize,
    pub name: String,
    pub device: String,
    /// Position in the Windows desktop, in pixels. The primary is at 0,0.
    pub x: i32,
    pub y: i32,
    /// Size in pixels, as currently shown (portrait swaps these).
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    /// Real visible size in millimetres, from the monitor's EDID.
    /// Already swapped for portrait. `None` if the monitor doesn't say.
    pub size_mm: Option<(u32, u32)>,
}

impl Screen {
    /// Diagonal in inches, e.g. 27.0.
    pub fn inches(&self) -> Option<f32> {
        self.size_mm.map(|(w, h)| diagonal_inches(w, h))
    }
}

/// Read the current arrangement. One quick pass, no DDC/CI involved.
pub fn read(monitors: &Monitors) -> Vec<Screen> {
    let paths = device_paths();
    let mut screens: Vec<Screen> = Vec::new();

    for m in &monitors.0 {
        // Several physical monitors can share one desktop area (rare);
        // the layout only needs each area once.
        if screens.iter().any(|s| s.device == m.device) {
            continue;
        }
        let Ok(mode) = display::current(&m.device) else {
            continue;
        };
        let size_mm = paths
            .get(&m.device)
            .and_then(|p| edid_for(p))
            .and_then(|e| physical_size_mm(&e))
            .map(|(w, h)| if mode.rotation.is_sideways() { (h, w) } else { (w, h) });

        screens.push(Screen {
            index: m.index,
            name: m.name.clone(),
            device: m.device.clone(),
            x: mode.x,
            y: mode.y,
            width: mode.width,
            height: mode.height,
            primary: m.primary,
            size_mm,
        });
    }
    screens
}

/// Raw EDID bytes for a monitor device path, read from the registry.
///
/// `\\?\DISPLAY#SDC4161#4&2b0f5b3a&0&UID8388688#{guid}` lives under
/// `HKLM\SYSTEM\CurrentControlSet\Enum\DISPLAY\SDC4161\4&2b0f5b3a&0&UID8388688\Device Parameters`.
fn edid_for(device_path: &str) -> Option<Vec<u8>> {
    let key = registry_key(device_path)?;
    let key_w: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let value_w: Vec<u16> = "EDID".encode_utf16().chain(Some(0)).collect();

    let mut buf = vec![0u8; 512];
    let mut size = buf.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key_w.as_ptr(),
            value_w.as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    buf.truncate(size as usize);
    Some(buf)
}

/// Device path -> registry key holding its EDID.
fn registry_key(device_path: &str) -> Option<String> {
    let trimmed = device_path.strip_prefix(r"\\?\").unwrap_or(device_path);
    let mut parts = trimmed.split('#');
    let class = parts.next()?; // "DISPLAY"
    let hardware = parts.next()?; // "SDC4161"
    let instance = parts.next()?; // "4&2b0f5b3a&0&UID8388688"
    if class.is_empty() || hardware.is_empty() || instance.is_empty() {
        return None;
    }
    Some(format!(
        r"SYSTEM\CurrentControlSet\Enum\{class}\{hardware}\{instance}\Device Parameters"
    ))
}

/// Visible screen size in millimetres from an EDID block.
///
/// Prefers the first detailed timing descriptor (exact mm). Falls back to
/// the basic size field (whole centimetres) if that's missing.
pub fn physical_size_mm(edid: &[u8]) -> Option<(u32, u32)> {
    const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if edid.len() < 128 || edid[..8] != HEADER {
        return None;
    }

    // Detailed timing descriptor 1 starts at byte 54; a zero pixel clock
    // means it isn't a timing descriptor.
    let clock = u16::from_le_bytes([edid[54], edid[55]]);
    if clock != 0 {
        let w = edid[66] as u32 | ((edid[68] as u32 & 0xF0) << 4);
        let h = edid[67] as u32 | ((edid[68] as u32 & 0x0F) << 8);
        if w > 0 && h > 0 {
            return Some((w, h));
        }
    }

    let (w_cm, h_cm) = (edid[21] as u32, edid[22] as u32);
    if w_cm > 0 && h_cm > 0 {
        return Some((w_cm * 10, h_cm * 10));
    }
    None
}

fn diagonal_inches(w_mm: u32, h_mm: u32) -> f32 {
    ((w_mm * w_mm + h_mm * h_mm) as f32).sqrt() / 25.4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edid_with(dtd_w: u32, dtd_h: u32, cm: (u8, u8)) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        e[21] = cm.0;
        e[22] = cm.1;
        if dtd_w > 0 {
            e[54] = 0x01; // non-zero pixel clock
            e[66] = (dtd_w & 0xFF) as u8;
            e[67] = (dtd_h & 0xFF) as u8;
            e[68] = (((dtd_w >> 8) & 0x0F) << 4) as u8 | ((dtd_h >> 8) & 0x0F) as u8;
        }
        e
    }

    #[test]
    fn reads_exact_size() {
        // A 27" 16:9 panel is about 597 x 336 mm
        assert_eq!(physical_size_mm(&edid_with(597, 336, (60, 34))), Some((597, 336)));
    }

    #[test]
    fn falls_back_to_centimetres() {
        assert_eq!(physical_size_mm(&edid_with(0, 0, (31, 20))), Some((310, 200)));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(physical_size_mm(&[0u8; 128]), None);
        assert_eq!(physical_size_mm(&[0u8; 10]), None);
    }

    #[test]
    fn diagonal_is_right() {
        let d = diagonal_inches(597, 336);
        assert!((d - 27.0).abs() < 0.1, "got {d}");
    }

    #[test]
    fn builds_registry_key() {
        let p = r"\\?\DISPLAY#SDC4161#4&2b0f5b3a&0&UID8388688#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert_eq!(
            registry_key(p).as_deref(),
            Some(r"SYSTEM\CurrentControlSet\Enum\DISPLAY\SDC4161\4&2b0f5b3a&0&UID8388688\Device Parameters")
        );
        assert_eq!(registry_key("nonsense"), None);
    }
}
