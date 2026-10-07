//! Design tokens. Names and values come from `.interface-design/system.md`.
//!
//! Graphite app, paper as the only bright surface, one accent (process cyan), hairline
//! rules and small lightness steps instead of shadows.

use gpui_kit::{Hsla, Rgba, rgba};
use shorui_core::tools::Group;

/// One theme's colours. `Copy` so views can grab the whole set once per render.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub dark: bool,
    // Surfaces
    pub bed: Hsla,
    /// The window itself: behind the sidebar and around the content card.
    pub frame: Hsla,
    /// The content card the work happens on.
    pub card: Hsla,
    pub plate: Hsla,
    pub plate_2: Hsla,
    pub plate_3: Hsla,
    pub sleeve: Hsla,
    pub well: Hsla,
    // Text
    pub toner: Hsla,
    pub toner_2: Hsla,
    pub toner_3: Hsla,
    pub toner_4: Hsla,
    // Lines
    pub rule_soft: Hsla,
    pub rule: Hsla,
    pub rule_strong: Hsla,
    pub control_rule: Hsla,
    pub key: Hsla,
    // Accent
    pub cyan: Hsla,
    pub cyan_hi: Hsla,
    pub cyan_lo: Hsla,
    pub cyan_ink: Hsla,
    pub cyan_wash: Hsla,
    pub cyan_rule: Hsla,
    // Meaning
    pub proof: Hsla,
    pub flag: Hsla,
    pub stamp: Hsla,
    pub stamp_fill: Hsla,
    pub stamp_hi: Hsla,
    pub stamp_lo: Hsla,
    pub stamp_ink: Hsla,
    pub stamp_wash: Hsla,
    pub stamp_rule: Hsla,
    // Paper
    pub stock: Hsla,
    pub stock_ink: Hsla,
    pub stock_edge: Hsla,
    pub stock_select: Hsla,
    /// The logo's vermilion seal.
    pub seal: Hsla,
    pub blackout: Hsla,
    // Controls
    pub knob: Hsla,
    pub thumb: Hsla,
    pub scrim: Hsla,
    // Group dots
    pub g_organise: Hsla,
    pub g_convert: Hsla,
    pub g_edit: Hsla,
    pub g_secure: Hsla,
    pub g_optimise: Hsla,
}

fn c(hex: u32) -> Hsla {
    let rgba: Rgba = rgba((hex << 8) | 0xFF);
    rgba.into()
}

/// Colour with alpha given as a percentage.
fn ca(hex: u32, percent: u32) -> Hsla {
    let alpha = (percent * 255 + 50) / 100;
    let rgba: Rgba = rgba((hex << 8) | alpha);
    rgba.into()
}

impl Palette {
    pub fn dark() -> Self {
        Palette {
            dark: true,
            bed: c(0x101113),
            frame: c(0x0B0C0E),
            card: c(0x101113),
            plate: c(0x18191C),
            plate_2: c(0x25272C),
            plate_3: c(0x2F3238),
            sleeve: c(0x1A1B1F),
            well: c(0x0A0B0C),
            toner: c(0xECEDEE),
            toner_2: c(0xA4A7AD),
            toner_3: c(0x868991),
            toner_4: c(0x55585F),
            rule_soft: ca(0xFFFFFF, 5),
            rule: ca(0xFFFFFF, 8),
            rule_strong: ca(0xFFFFFF, 14),
            control_rule: ca(0xFFFFFF, 12),
            key: ca(0xFFFFFF, 6),
            cyan: c(0x4CC2DE),
            cyan_hi: c(0x6FD0E8),
            cyan_lo: c(0x38AECB),
            cyan_ink: c(0x06222B),
            cyan_wash: ca(0x4CC2DE, 14),
            cyan_rule: ca(0x4CC2DE, 45),
            proof: c(0x4CB782),
            flag: c(0xD9A63A),
            stamp: c(0xF0736A),
            stamp_fill: c(0xC4372D),
            stamp_hi: c(0xD4463B),
            stamp_lo: c(0xAD2E25),
            stamp_ink: c(0xFFFFFF),
            stamp_wash: ca(0xE5594F, 12),
            stamp_rule: ca(0xE5594F, 38),
            stock: c(0xF4F2EC),
            stock_ink: c(0x1A1A1A),
            stock_edge: ca(0x000000, 0),
            stock_select: c(0x0A7690),
            seal: c(0xD63A26),
            blackout: c(0x0A0A0A),
            knob: c(0xA4A7AD),
            thumb: c(0xECEDEE),
            scrim: ca(0x050607, 62),
            g_organise: c(0x8C96F0),
            g_convert: c(0xDDAE4A),
            g_edit: c(0x5CC8A0),
            g_secure: c(0xE77C6E),
            g_optimise: c(0xB99AEC),
        }
    }

    pub fn light() -> Self {
        Palette {
            dark: false,
            bed: c(0xF6F6F4),
            frame: c(0xF6F6F4),
            card: c(0xFDFDFC),
            plate: c(0xEDEDEA),
            plate_2: c(0xE4E4E0),
            plate_3: c(0xDADAD5),
            sleeve: c(0xFFFFFF),
            well: c(0xECECE8),
            toner: c(0x1B1C1E),
            toner_2: c(0x4A4D54),
            toner_3: c(0x63666E),
            toner_4: c(0xA0A3A9),
            rule_soft: ca(0x000000, 6),
            rule: ca(0x000000, 9),
            rule_strong: ca(0x000000, 16),
            control_rule: ca(0x000000, 14),
            key: ca(0x000000, 5),
            cyan: c(0x0A7690),
            cyan_hi: c(0x0C86A3),
            cyan_lo: c(0x08667D),
            cyan_ink: c(0xFFFFFF),
            cyan_wash: ca(0x0A7690, 12),
            cyan_rule: ca(0x0A7690, 45),
            proof: c(0x1F8A5B),
            flag: c(0x9A6B00),
            stamp: c(0xB93228),
            stamp_fill: c(0xC4372D),
            stamp_hi: c(0xD4463B),
            stamp_lo: c(0xAD2E25),
            stamp_ink: c(0xFFFFFF),
            stamp_wash: ca(0xC4372D, 8),
            stamp_rule: ca(0xC4372D, 32),
            stock: c(0xFFFFFF),
            stock_ink: c(0x1A1A1A),
            stock_edge: ca(0x000000, 16),
            stock_select: c(0x0A7690),
            seal: c(0xD63A26),
            blackout: c(0x0A0A0A),
            knob: c(0xFFFFFF),
            thumb: c(0xFFFFFF),
            scrim: ca(0x1E1F21, 32),
            g_organise: c(0x6C78E8),
            g_convert: c(0xC99A2E),
            g_edit: c(0x2FA87C),
            g_secure: c(0xD9614F),
            g_optimise: c(0x9B78DD),
        }
    }

    pub fn of(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }

    pub fn group(&self, group: Group) -> Hsla {
        match group {
            Group::Organise => self.g_organise,
            Group::Convert => self.g_convert,
            Group::Edit => self.g_edit,
            Group::Secure => self.g_secure,
            Group::Optimise => self.g_optimise,
        }
    }
}

// Sizes, in pixels. Base unit 4.
pub const SIDEBAR_W: f32 = 220.0;
pub const RAIL_W: f32 = 48.0;
pub const PANEL_W: f32 = 300.0;
pub const TOPBAR_H: f32 = 44.0;
/// Gap between the content card and the window edge.
pub const INSET: f32 = 8.0;
pub const RUNLINE_H: f32 = 52.0;
pub const ROW_H: f32 = 48.0;
pub const NAV_H: f32 = 28.0;
pub const CONTROL_H: f32 = 28.0;
pub const R_XS: f32 = 4.0;
pub const R_SM: f32 = 6.0;
pub const R_MD: f32 = 8.0;
pub const R_LG: f32 = 12.0;

pub const FONT_UI: &str = "IBM Plex Sans";
pub const FONT_MONO: &str = "IBM Plex Mono";
/// The face used for text typed onto a page. The saved file uses Helvetica, so the page
/// shows the system face with the same letter widths.
pub const FONT_PAGE: &str = if cfg!(target_os = "windows") {
    "Arial"
} else if cfg!(target_os = "macos") {
    "Helvetica"
} else {
    "Liberation Sans"
};
pub const TEXT: f32 = 13.0;
pub const TEXT_SM: f32 = 12.0;
pub const TEXT_XS: f32 = 11.0;
