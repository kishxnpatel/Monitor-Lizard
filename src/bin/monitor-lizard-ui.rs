//! Monitor Lizard window. Open it, change things, close it.
//! Nothing keeps running after the window closes.

// Release builds are a normal Windows app (no console window).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use monitor_lizard::advanced::{self, Projection};
use monitor_lizard::display::{self, Change, DisplayMode, Resolution, Rotation};
use monitor_lizard::{layout, Monitors};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};

slint::include_modules!();

const ROTATIONS: [Rotation; 4] = [
    Rotation::Landscape,
    Rotation::Portrait,
    Rotation::LandscapeFlipped,
    Rotation::PortraitFlipped,
];
const KEEP_SECONDS: i32 = 15;

/// Everything the window needs to remember between clicks.
struct State {
    monitors: Monitors,
    selected: usize,
    resolutions: Vec<Resolution>,
    /// Settings before the last Apply, for the 15-second revert.
    before: Option<Undo>,
    countdown: Timer,
}

/// What the 15-second countdown puts back if you don't press Keep.
#[allow(clippy::large_enum_variant)] // one value at a time, size is irrelevant
enum Undo {
    Mode(String, DisplayMode),
    Projection(Projection),
}

/// Slider moves go to a background thread so the window never stutters,
/// even when a monitor takes 50 ms to answer over DDC/CI.
enum Job {
    Brightness(usize, u8),
    Contrast(usize, u8),
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.global::<Theme>().set_text_scale(windows_text_scale());
    size_to_screen(&ui);

    let state = Rc::new(RefCell::new(State {
        monitors: Monitors::detect(),
        selected: 0,
        resolutions: Vec::new(),
        before: None,
        countdown: Timer::default(),
    }));

    let jobs = spawn_worker();

    ui.set_projections(model(Projection::ALL.iter().map(|p| p.name().into()).collect()));
    refresh_monitors(&ui, &state);

    ui.on_monitor_changed({
        let (ui, state) = (ui.as_weak(), state.clone());
        move |i| {
            if let Some(ui) = ui.upgrade() {
                load_monitor(&ui, &state, i.max(0) as usize);
            }
        }
    });

    ui.on_brightness_moved({
        let (jobs, state) = (jobs.clone(), state.clone());
        move |v| {
            let index = state.borrow().selected;
            let _ = jobs.send(Job::Brightness(index, v.round().clamp(0.0, 100.0) as u8));
        }
    });

    ui.on_contrast_moved({
        let (jobs, state) = (jobs.clone(), state.clone());
        move |v| {
            let index = state.borrow().selected;
            let _ = jobs.send(Job::Contrast(index, v.round().clamp(0.0, 100.0) as u8));
        }
    });

    ui.on_resolution_changed({
        let (ui, state) = (ui.as_weak(), state.clone());
        move |i| {
            let Some(ui) = ui.upgrade() else { return };
            let st = state.borrow();
            let Some(res) = st.resolutions.get(i.max(0) as usize) else { return };
            // Keep the same refresh rate if the new resolution has it
            let current: Option<u32> = refresh_from_label(&ui.get_refresh_rates(), ui.get_refresh_index());
            let idx = current
                .and_then(|hz| res.refresh_rates.iter().position(|r| *r == hz))
                .unwrap_or(0);
            ui.set_refresh_rates(model(refresh_labels(&res.refresh_rates)));
            ui.set_refresh_index(idx as i32);
            ui.set_dirty(true);
        }
    });

    ui.on_selection_changed({
        let ui = ui.as_weak();
        move || {
            if let Some(ui) = ui.upgrade() {
                ui.set_dirty(true);
            }
        }
    });

    ui.on_discard({
        let (ui, state) = (ui.as_weak(), state.clone());
        move || {
            if let Some(ui) = ui.upgrade() {
                let i = state.borrow().selected;
                load_monitor(&ui, &state, i);
            }
        }
    });

    ui.on_apply({
        let (ui, state) = (ui.as_weak(), state.clone());
        move || {
            if let Some(ui) = ui.upgrade() {
                apply(&ui, &state);
            }
        }
    });

    ui.on_keep({
        let (ui, state) = (ui.as_weak(), state.clone());
        move || {
            let Some(ui) = ui.upgrade() else { return };
            {
                let mut st = state.borrow_mut();
                st.countdown.stop();
                st.before = None;
            }
            ui.set_confirming(false);
            let i = state.borrow().selected;
            load_monitor(&ui, &state, i);
            ui.set_status("Saved.".into());
        }
    });

    ui.on_undo({
        let (ui, state) = (ui.as_weak(), state.clone());
        move || {
            if let Some(ui) = ui.upgrade() {
                revert(&ui, &state);
            }
        }
    });

    ui.on_hdr_toggled({
        let (ui, state) = (ui.as_weak(), state.clone());
        move |on| {
            let Some(ui) = ui.upgrade() else { return };
            let device = selected_device(&state);
            match advanced::set_hdr(&device, on) {
                Ok(()) => {
                    let i = state.borrow().selected;
                    load_monitor(&ui, &state, i); // profile list differs for HDR
                    ui.set_status(if on { "HDR on." } else { "HDR off." }.into());
                }
                Err(e) => {
                    ui.set_hdr_on(!on);
                    ui.set_status(e.into());
                }
            }
        }
    });

    ui.on_color_profile_changed({
        let (ui, state) = (ui.as_weak(), state.clone());
        move |i| {
            let Some(ui) = ui.upgrade() else { return };
            use slint::Model;
            let Some(name) = ui.get_color_profiles().row_data(i.max(0) as usize) else { return };
            match advanced::set_color_profile(&selected_device(&state), &name) {
                Ok(()) => ui.set_status(format!("Color profile: {name}").into()),
                Err(e) => ui.set_status(e.into()),
            }
        }
    });

    ui.on_projection_changed({
        let (ui, state) = (ui.as_weak(), state.clone());
        move |i| {
            let Some(ui) = ui.upgrade() else { return };
            let Some(mode) = Projection::ALL.get(i.max(0) as usize).copied() else { return };
            let previous = advanced::projection();
            if previous == Some(mode) {
                return;
            }
            match advanced::set_projection(mode) {
                Ok(()) => {
                    refresh_monitors(&ui, &state);
                    if let Some(prev) = previous {
                        state.borrow_mut().before = Some(Undo::Projection(prev));
                        ui.set_seconds_left(KEEP_SECONDS);
                        ui.set_confirming(true);
                        start_countdown(&ui, &state);
                    }
                }
                Err(e) => ui.set_status(e.into()),
            }
        }
    });

    ui.on_make_primary({
        let (ui, state) = (ui.as_weak(), state.clone());
        move || {
            let Some(ui) = ui.upgrade() else { return };
            let device = selected_device(&state);
            let devices: Vec<String> =
                layout::read(&state.borrow().monitors).into_iter().map(|s| s.device).collect();
            match advanced::make_primary(&device, &devices) {
                Ok(()) => {
                    refresh_monitors(&ui, &state);
                    ui.set_status("Main display changed.".into());
                }
                Err(e) => {
                    ui.set_is_primary(false);
                    ui.set_status(e.into());
                }
            }
        }
    });

    ui.on_open_settings(|uri| {
        // explorer.exe opens ms-settings: links without flashing a console
        let _ = std::process::Command::new("explorer.exe").arg(uri.as_str()).spawn();
    });

    ui.run()
}

/// Fill the window with the selected monitor's current settings.
fn load_monitor(ui: &AppWindow, state: &Rc<RefCell<State>>, i: usize) {
    let mut st = state.borrow_mut();
    let Some(m) = st.monitors.0.get(i) else { return };
    let device = m.device.clone();

    // Brightness works on laptops (WMI) and on DDC/CI monitors
    match m.brightness() {
        Ok(v) => {
            ui.set_brightness_available(true);
            ui.set_brightness(v as f32);
        }
        Err(_) => ui.set_brightness_available(false),
    }
    match (!m.builtin).then(|| m.contrast()) {
        Some(Ok(v)) => {
            ui.set_has_contrast(true);
            ui.set_contrast(v as f32);
        }
        _ => ui.set_has_contrast(false),
    }

    let kind = if m.builtin { "Built-in screen" } else { "External monitor" };
    let primary = if m.primary { " · Main display" } else { "" };

    let resolutions = display::resolutions(&device);
    match display::current(&device) {
        Ok(now) => {
            let (w, h) = (now.width.max(now.height), now.width.min(now.height));
            let res_idx = resolutions
                .iter()
                .position(|r| r.width == w && r.height == h)
                .unwrap_or(0);
            let labels: Vec<SharedString> = resolutions
                .iter()
                .enumerate()
                .map(|(n, r)| {
                    let tag = if n == 0 { " (Recommended)" } else { "" };
                    format!("{} × {}{}", r.width, r.height, tag).into()
                })
                .collect();
            ui.set_resolutions(model(labels));
            ui.set_resolution_index(res_idx as i32);

            let rates = resolutions.get(res_idx).map(|r| r.refresh_rates.clone()).unwrap_or_default();
            let rate_idx = rates.iter().position(|r| *r == now.refresh).unwrap_or(0);
            ui.set_refresh_rates(model(refresh_labels(&rates)));
            ui.set_refresh_index(rate_idx as i32);

            let rot_idx = ROTATIONS.iter().position(|r| *r == now.rotation).unwrap_or(0);
            ui.set_rotation_index(rot_idx as i32);

            ui.set_monitor_subtitle(
                format!("{kind} · {} × {} · {} Hz{primary}", now.width, now.height, now.refresh).into(),
            );
        }
        Err(e) => {
            ui.set_monitor_subtitle(format!("{kind}{primary}").into());
            ui.set_status(e.into());
        }
    }

    // HDR
    let hdr = advanced::hdr(&device).unwrap_or(advanced::Hdr { supported: false, enabled: false });
    ui.set_hdr_supported(hdr.supported);
    ui.set_hdr_on(hdr.enabled);

    // Colour profiles (falls back to the Windows link if none are installed)
    match advanced::color_profiles(&device) {
        Ok((list, current)) => {
            let idx = current.as_ref().and_then(|c| list.iter().position(|p| p == c)).unwrap_or(0);
            ui.set_color_profiles(model(list.into_iter().map(Into::into).collect()));
            ui.set_color_profile_index(idx as i32);
        }
        Err(_) => ui.set_color_profiles(model(Vec::new())),
    }

    // Multiple displays
    ui.set_is_primary(m.primary);
    ui.set_monitor_count(st.monitors.0.len() as i32);
    if let Some(p) = advanced::projection() {
        let idx = Projection::ALL.iter().position(|x| *x == p).unwrap_or(1);
        ui.set_projection_index(idx as i32);
    }

    st.selected = i;
    st.resolutions = resolutions;
    ui.set_dirty(false);
    ui.set_status("".into());
}

/// Re-detect monitors (after Win+P or a main-display change) and redraw.
fn refresh_monitors(ui: &AppWindow, state: &Rc<RefCell<State>>) {
    let keep = {
        let mut st = state.borrow_mut();
        let old_device = st.monitors.0.get(st.selected).map(|m| m.device.clone());
        st.monitors = Monitors::detect();
        let names: Vec<SharedString> =
            st.monitors.0.iter().map(|m| format!("{}. {}", m.index, m.name).into()).collect();
        ui.set_monitors(model(names));
        old_device
            .and_then(|d| st.monitors.0.iter().position(|m| m.device == d))
            .unwrap_or(0)
    };
    ui.set_monitor_index(keep as i32);
    load_monitor(ui, state, keep);
}

fn selected_device(state: &Rc<RefCell<State>>) -> String {
    let st = state.borrow();
    st.monitors.0.get(st.selected).map(|m| m.device.clone()).unwrap_or_default()
}

/// Apply resolution / refresh / orientation, then start the 15s countdown.
fn apply(ui: &AppWindow, state: &Rc<RefCell<State>>) {
    let (device, change) = {
        let st = state.borrow();
        let Some(m) = st.monitors.0.get(st.selected) else { return };
        let res = st.resolutions.get(ui.get_resolution_index().max(0) as usize);
        let change = Change {
            resolution: res.map(|r| (r.width, r.height)),
            refresh: refresh_from_label(&ui.get_refresh_rates(), ui.get_refresh_index()),
            rotation: ROTATIONS.get(ui.get_rotation_index().max(0) as usize).copied(),
        };
        (m.device.clone(), change)
    };

    let result = (|| -> Result<DisplayMode, String> {
        let before = display::current(&device)?;
        let after = display::plan(&device, &change)?;
        display::test(&device, &after)?;
        display::apply(&device, &after)?;
        Ok(before)
    })();

    match result {
        Ok(before) => {
            state.borrow_mut().before = Some(Undo::Mode(device, before));
            ui.set_seconds_left(KEEP_SECONDS);
            ui.set_confirming(true);
            start_countdown(ui, state);
        }
        Err(e) => ui.set_status(e.into()),
    }
}

fn start_countdown(ui: &AppWindow, state: &Rc<RefCell<State>>) {
    let weak = ui.as_weak();
    let st_clone = state.clone();
    state.borrow().countdown.start(TimerMode::Repeated, Duration::from_secs(1), move || {
        let Some(ui) = weak.upgrade() else { return };
        let left = ui.get_seconds_left() - 1;
        ui.set_seconds_left(left);
        if left <= 0 {
            revert(&ui, &st_clone);
        }
    });
}

/// Put the screen back the way it was before Apply.
fn revert(ui: &AppWindow, state: &Rc<RefCell<State>>) {
    let before = {
        let mut st = state.borrow_mut();
        st.countdown.stop();
        st.before.take()
    };
    ui.set_confirming(false);
    let result = match before {
        Some(Undo::Mode(device, mode)) => display::apply(&device, &mode),
        Some(Undo::Projection(mode)) => advanced::set_projection(mode),
        None => Ok(()),
    };
    if let Err(e) = result {
        ui.set_status(format!("Couldn't revert: {e}").into());
        return;
    }
    refresh_monitors(ui, state);
    ui.set_status("Reverted. Nothing changed.".into());
}

/// Background thread that talks to monitors. It always skips to the
/// newest value, so dragging a slider fast never builds up a queue.
fn spawn_worker() -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    std::thread::spawn(move || {
        let monitors = Monitors::detect();
        while let Ok(first) = rx.recv() {
            let mut brightness = None;
            let mut contrast = None;
            for job in std::iter::once(first).chain(rx.try_iter()) {
                match job {
                    Job::Brightness(i, v) => brightness = Some((i, v)),
                    Job::Contrast(i, v) => contrast = Some((i, v)),
                }
            }
            if let Some((i, v)) = brightness {
                if let Some(m) = monitors.0.get(i) {
                    let _ = m.set_brightness(v);
                }
            }
            if let Some((i, v)) = contrast {
                if let Some(m) = monitors.0.get(i) {
                    let _ = m.set_contrast(v);
                }
            }
        }
    });
    tx
}

/// Open at ~40% of the main screen's width, 75% of its height.
/// Text and icons follow the Windows scale setting automatically.
fn size_to_screen(ui: &AppWindow) {
    let screens = layout::read(&Monitors::detect());
    let Some(main) = screens.iter().find(|s| s.primary).or(screens.first()) else { return };
    let w = (main.width as f32 * 0.40) as u32;
    let h = (main.height as f32 * 0.75) as u32;
    ui.window().set_size(slint::PhysicalSize::new(w.max(640), h.max(720)));
}

/// Windows "Text size" setting (Settings > Accessibility > Text size),
/// stored as 100-225 in the registry. Windows Settings follows it, so we do too.
fn windows_text_scale() -> f32 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
        let key: Vec<u16> = "Software\\Microsoft\\Accessibility".encode_utf16().chain(Some(0)).collect();
        let value: Vec<u16> = "TextScaleFactor".encode_utf16().chain(Some(0)).collect();
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut data as *mut u32).cast(),
                &mut size,
            )
        };
        if status == 0 && (100..=225).contains(&data) {
            return data as f32 / 100.0;
        }
    }
    1.0
}

fn model(items: Vec<SharedString>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items))
}

fn refresh_labels(rates: &[u32]) -> Vec<SharedString> {
    rates.iter().map(|r| format!("{r} Hz").into()).collect()
}

fn refresh_from_label(list: &ModelRc<SharedString>, index: i32) -> Option<u32> {
    use slint::Model;
    let label = list.row_data(index.max(0) as usize)?;
    label.trim_end_matches(" Hz").parse().ok()
}
