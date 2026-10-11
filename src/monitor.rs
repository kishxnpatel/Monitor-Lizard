//! Finding monitors and reading/writing their settings over DDC/CI.

use std::mem::size_of;
use std::thread::sleep;
use std::time::Duration;

use windows_sys::core::BOOL;
use windows_sys::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply, SetVCPFeature,
    PHYSICAL_MONITOR,
};
use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};

use crate::laptop;
use crate::names::{friendly_names, wide_to_string};
use crate::vcp;

/// DDC/CI is a slow, chatty bus. Monitors sometimes ignore a request,
/// so we retry a few times with a short pause.
const RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(40);
const MONITORINFOF_PRIMARY: u32 = 1;

/// One physical monitor.
pub struct Monitor {
    /// 1-based number shown to the user.
    pub index: usize,
    /// Friendly name, e.g. "DELL U2720Q".
    pub name: String,
    /// Windows device name, e.g. `\\.\DISPLAY1`.
    pub device: String,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
    /// Built-in laptop screen (brightness via WMI, no DDC/CI).
    pub builtin: bool,
    physical: PHYSICAL_MONITOR,
}

/// A raw VCP reading: current value and the monitor's maximum.
#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub current: u32,
    pub max: u32,
}

impl Reading {
    /// Value as 0-100%.
    pub fn percent(&self) -> u8 {
        if self.max == 0 {
            return 0;
        }
        ((self.current.min(self.max) as u64 * 100 + self.max as u64 / 2) / self.max as u64) as u8
    }
}

impl Monitor {
    /// Read any VCP code.
    pub fn read(&self, code: u8) -> Result<Reading, String> {
        for attempt in 0..RETRIES {
            let mut kind = 0;
            let mut current = 0u32;
            let mut max = 0u32;
            let ok = unsafe {
                GetVCPFeatureAndVCPFeatureReply(
                    self.physical.hPhysicalMonitor,
                    code,
                    &mut kind,
                    &mut current,
                    &mut max,
                )
            };
            if ok != 0 {
                return Ok(Reading { current, max });
            }
            if attempt + 1 < RETRIES {
                sleep(RETRY_DELAY);
            }
        }
        Err(format!(
            "{} didn't answer (VCP 0x{code:02X}). DDC/CI may be off or unsupported.",
            self.name
        ))
    }

    /// Write any VCP code.
    pub fn write(&self, code: u8, value: u32) -> Result<(), String> {
        for attempt in 0..RETRIES {
            let ok = unsafe { SetVCPFeature(self.physical.hPhysicalMonitor, code, value) };
            if ok != 0 {
                return Ok(());
            }
            if attempt + 1 < RETRIES {
                sleep(RETRY_DELAY);
            }
        }
        Err(format!(
            "{} refused the change (VCP 0x{code:02X}). DDC/CI may be off or unsupported.",
            self.name
        ))
    }

    /// True if the monitor answers DDC/CI at all.
    pub fn supports_ddc(&self) -> bool {
        self.read(vcp::BRIGHTNESS).is_ok()
    }

    pub fn brightness(&self) -> Result<u8, String> {
        if self.builtin {
            return laptop::get();
        }
        Ok(self.read(vcp::BRIGHTNESS)?.percent())
    }

    pub fn set_brightness(&self, percent: u8) -> Result<(), String> {
        if self.builtin {
            return laptop::set(percent);
        }
        self.set_percent(vcp::BRIGHTNESS, percent)
    }

    pub fn contrast(&self) -> Result<u8, String> {
        self.external_only("contrast")?;
        Ok(self.read(vcp::CONTRAST)?.percent())
    }

    pub fn set_contrast(&self, percent: u8) -> Result<(), String> {
        self.external_only("contrast")?;
        self.set_percent(vcp::CONTRAST, percent)
    }

    /// Current input as the raw VCP value (see `vcp::input_name`).
    pub fn input(&self) -> Result<u8, String> {
        self.external_only("input switching")?;
        // Some monitors put extra bits in the high byte; the input is the low byte.
        Ok((self.read(vcp::INPUT_SOURCE)?.current & 0xFF) as u8)
    }

    pub fn set_input(&self, value: u8) -> Result<(), String> {
        self.external_only("input switching")?;
        self.write(vcp::INPUT_SOURCE, value as u32)
    }

    fn external_only(&self, what: &str) -> Result<(), String> {
        if self.builtin {
            Err(format!("{} is a built-in screen, so {what} isn't available.", self.name))
        } else {
            Ok(())
        }
    }

    /// Percent (0-100) -> the monitor's own scale (usually also 0-100).
    fn set_percent(&self, code: u8, percent: u8) -> Result<(), String> {
        let percent = percent.min(100) as u32;
        let max = self.read(code).map(|r| r.max).unwrap_or(100).max(1);
        self.write(code, (percent * max + 50) / 100)
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe {
            DestroyPhysicalMonitors(1, &self.physical);
        }
    }
}

/// All connected monitors, in Windows' order.
pub struct Monitors(pub Vec<Monitor>);

impl Monitors {
    pub fn detect() -> Monitors {
        let names = friendly_names();
        let mut list = Vec::new();

        for hmon in display_handles() {
            let Some((device, rect, primary)) = monitor_info(hmon) else {
                continue;
            };

            let mut count = 0u32;
            if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut count) } == 0 || count == 0
            {
                continue;
            }
            let mut physical = vec![PHYSICAL_MONITOR::default(); count as usize];
            if unsafe { GetPhysicalMonitorsFromHMONITOR(hmon, count, physical.as_mut_ptr()) } == 0 {
                continue;
            }

            for p in physical {
                let desc_buf = p.szPhysicalMonitorDescription; // copy: struct is packed
                let description = wide_to_string(&desc_buf);
                let name = names
                    .get(&device)
                    .cloned()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| {
                        if description.is_empty() || description == "Generic PnP Monitor" {
                            "Display".to_string()
                        } else {
                            description
                        }
                    });

                list.push(Monitor {
                    index: list.len() + 1,
                    name,
                    device: device.clone(),
                    width: rect.right - rect.left,
                    height: rect.bottom - rect.top,
                    primary,
                    builtin: false,
                    physical: p,
                });
            }
        }
        // A screen that ignores DDC/CI while Windows can dim it through WMI
        // is the laptop's own panel. Laptops have one, so mark the first match.
        if list.iter().any(|m| !m.supports_ddc()) && laptop::available() {
            if let Some(m) = list.iter_mut().find(|m| !m.supports_ddc()) {
                m.builtin = true;
                if m.name == "Display" {
                    m.name = "Built-in screen".to_string();
                }
            }
        }
        Monitors(list)
    }

    /// Pick monitors by "all" or a 1-based number.
    pub fn select(&self, which: &str) -> Result<Vec<&Monitor>, String> {
        if which.eq_ignore_ascii_case("all") {
            return Ok(self.0.iter().collect());
        }
        let n: usize = which
            .parse()
            .map_err(|_| format!("'{which}' isn't a monitor number. Use a number or 'all'."))?;
        self.0
            .iter()
            .find(|m| m.index == n)
            .map(|m| vec![m])
            .ok_or_else(|| format!("No monitor {n}. Run `monitor-lizard list` to see them."))
    }
}

/// Every HMONITOR Windows knows about.
fn display_handles() -> Vec<HMONITOR> {
    unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data as *mut Vec<HMONITOR>);
        list.push(h);
        1 // keep going
    }

    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut handles as *mut _ as LPARAM,
        );
    }
    handles
}

/// Device name, screen rectangle and "is primary" for one HMONITOR.
fn monitor_info(hmon: HMONITOR) -> Option<(String, RECT, bool)> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    let ok = unsafe { GetMonitorInfoW(hmon, &mut info as *mut _ as *mut MONITORINFO) };
    if ok == 0 {
        return None;
    }
    Some((
        wide_to_string(&info.szDevice),
        info.monitorInfo.rcMonitor,
        info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
    ))
}

#[cfg(test)]
mod tests {
    use super::Reading;

    #[test]
    fn percent_scales() {
        assert_eq!(Reading { current: 50, max: 100 }.percent(), 50);
        assert_eq!(Reading { current: 128, max: 255 }.percent(), 50);
        assert_eq!(Reading { current: 0, max: 0 }.percent(), 0);
        assert_eq!(Reading { current: 120, max: 100 }.percent(), 100);
    }
}
