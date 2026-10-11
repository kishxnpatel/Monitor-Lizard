//! VCP codes: the "settings numbers" monitors understand over DDC/CI.
//! Defined by the VESA MCCS standard.

pub const BRIGHTNESS: u8 = 0x10;
pub const CONTRAST: u8 = 0x12;
pub const INPUT_SOURCE: u8 = 0x60;

/// Common input source values (VCP 0x60). Not every monitor uses all of
/// these, and a few brands use their own numbers for USB-C.
pub const INPUTS: &[(u8, &str)] = &[
    (0x01, "VGA 1"),
    (0x02, "VGA 2"),
    (0x03, "DVI 1"),
    (0x04, "DVI 2"),
    (0x0F, "DisplayPort 1"),
    (0x10, "DisplayPort 2"),
    (0x11, "HDMI 1"),
    (0x12, "HDMI 2"),
    (0x1B, "USB-C"),
];

/// Friendly name for an input value, e.g. 0x11 -> "HDMI 1".
pub fn input_name(value: u8) -> String {
    INPUTS
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| format!("Unknown (0x{value:02X})"))
}

/// Parse what a user types ("hdmi1", "HDMI 1", "dp2", "usbc", "0x11", "17").
pub fn parse_input(text: &str) -> Option<u8> {
    let t = text.trim().to_ascii_lowercase().replace([' ', '-', '_'], "");
    if let Some(hex) = t.strip_prefix("0x") {
        return u8::from_str_radix(hex, 16).ok();
    }
    if let Ok(n) = t.parse::<u8>() {
        return Some(n);
    }
    let alias = match t.as_str() {
        "vga" | "vga1" => 0x01,
        "vga2" => 0x02,
        "dvi" | "dvi1" => 0x03,
        "dvi2" => 0x04,
        "dp" | "dp1" | "displayport" | "displayport1" => 0x0F,
        "dp2" | "displayport2" => 0x10,
        "hdmi" | "hdmi1" => 0x11,
        "hdmi2" => 0x12,
        "usbc" | "typec" => 0x1B,
        _ => return None,
    };
    Some(alias)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inputs() {
        assert_eq!(parse_input("hdmi1"), Some(0x11));
        assert_eq!(parse_input("HDMI 2"), Some(0x12));
        assert_eq!(parse_input("DP"), Some(0x0F));
        assert_eq!(parse_input("usb-c"), Some(0x1B));
        assert_eq!(parse_input("0x11"), Some(0x11));
        assert_eq!(parse_input("15"), Some(15));
        assert_eq!(parse_input("toaster"), None);
    }

    #[test]
    fn names_inputs() {
        assert_eq!(input_name(0x11), "HDMI 1");
        assert_eq!(input_name(0xAB), "Unknown (0xAB)");
    }
}
