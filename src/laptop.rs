//! Built-in laptop screens.
//!
//! Laptop panels don't speak DDC/CI. Windows exposes their brightness
//! through WMI instead (`root\WMI`, classes `WmiMonitorBrightness` and
//! `WmiMonitorBrightnessMethods`). That's the same thing the Windows
//! brightness slider uses.

use windows::core::{BSTR, PCWSTR, VARIANT, w};
use windows::Win32::Security::PSECURITY_DESCRIPTOR;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoInitializeSecurity, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Wmi::{
    IEnumWbemClassObject, IWbemClassObject, IWbemLocator, IWbemServices, WbemLocator,
    WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_FLAG_RETURN_WBEM_COMPLETE,
    WBEM_INFINITE,
};

/// Connect to the `root\WMI` namespace.
fn connect() -> Result<IWbemServices, String> {
    unsafe {
        // Fine to call more than once; we ignore "already initialised".
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        // WMI needs the process to allow impersonation. Errors here just mean
        // it was already set, which is fine.
        let _ = CoInitializeSecurity(
            PSECURITY_DESCRIPTOR::default(),
            -1,
            None,
            None,
            RPC_C_AUTHN_LEVEL_DEFAULT,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
            None,
        );
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("WMI unavailable: {e}"))?;
        locator
            .ConnectServer(
                &BSTR::from("ROOT\\WMI"),
                &BSTR::new(),
                &BSTR::new(),
                &BSTR::new(),
                0,
                &BSTR::new(),
                None,
            )
            .map_err(|e| format!("WMI unavailable: {e}"))
    }
}

/// First object returned by a WQL query, if any.
fn first(services: &IWbemServices, query: &str) -> Result<IWbemClassObject, String> {
    unsafe {
        let results: IEnumWbemClassObject = services
            .ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from(query),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None,
            )
            .map_err(|e| format!("WMI query failed: {e}"))?;

        let mut row = [None];
        let mut returned = 0u32;
        let _ = results.Next(WBEM_INFINITE, &mut row, &mut returned);
        row[0]
            .take()
            .filter(|_| returned > 0)
            .ok_or_else(|| "No built-in screen brightness control found.".to_string())
    }
}

/// Read one property as a number.
fn number(obj: &IWbemClassObject, name: PCWSTR) -> Result<u32, String> {
    unsafe {
        let mut value = VARIANT::default();
        obj.Get(name, 0, &mut value, None, None)
            .map_err(|e| format!("WMI read failed: {e}"))?;
        u32::try_from(&value).map_err(|e| format!("WMI read failed: {e}"))
    }
}

/// True if this PC has a built-in screen Windows can dim.
pub fn available() -> bool {
    get().is_ok()
}

/// Current built-in screen brightness, 0-100%.
pub fn get() -> Result<u8, String> {
    let services = connect()?;
    let obj = first(
        &services,
        "SELECT CurrentBrightness FROM WmiMonitorBrightness WHERE Active=TRUE",
    )?;
    Ok(number(&obj, w!("CurrentBrightness"))?.min(100) as u8)
}

/// Set built-in screen brightness, 0-100%.
pub fn set(percent: u8) -> Result<(), String> {
    let services = connect()?;
    unsafe {
        // Which screen instance to call the method on
        let instance = first(
            &services,
            "SELECT * FROM WmiMonitorBrightnessMethods WHERE Active=TRUE",
        )?;
        let mut path = VARIANT::default();
        instance
            .Get(w!("__PATH"), 0, &mut path, None, None)
            .map_err(|e| format!("WMI read failed: {e}"))?;
        let path = BSTR::try_from(&path).map_err(|e| format!("WMI read failed: {e}"))?;

        // Build the method's input: WmiSetBrightness(Timeout, Brightness)
        let mut class = None;
        services
            .GetObject(
                &BSTR::from("WmiMonitorBrightnessMethods"),
                WBEM_FLAG_RETURN_WBEM_COMPLETE,
                None,
                Some(&mut class),
                None,
            )
            .map_err(|e| format!("WMI unavailable: {e}"))?;
        let class: IWbemClassObject = class.ok_or("WMI unavailable")?;

        let mut signature = None;
        class
            .GetMethod(w!("WmiSetBrightness"), 0, &mut signature, std::ptr::null_mut())
            .map_err(|e| format!("WMI unavailable: {e}"))?;
        let params = signature
            .ok_or("WMI unavailable")?
            .SpawnInstance(0)
            .map_err(|e| format!("WMI unavailable: {e}"))?;

        params
            .Put(w!("Timeout"), 0, &VARIANT::from(0i32), 0)
            .map_err(|e| format!("WMI write failed: {e}"))?;
        params
            .Put(w!("Brightness"), 0, &VARIANT::from(percent.min(100)), 0)
            .map_err(|e| format!("WMI write failed: {e}"))?;

        services
            .ExecMethod(
                &path,
                &BSTR::from("WmiSetBrightness"),
                WBEM_FLAG_RETURN_WBEM_COMPLETE,
                None,
                &params,
                None,
                None,
            )
            .map_err(|e| format!("Couldn't set built-in screen brightness: {e}"))
    }
}
