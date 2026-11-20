//! The macOS app's "indigo net" palette (`Theme.swift`), light and dark.
use gpui::{Hsla, rgb};

#[derive(Clone, Copy)]
pub struct Theme {
    pub background: Hsla,
    pub surface: Hsla,
    pub conversation: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub dark: bool,
    pub bubble_border: Hsla,
    pub text: Hsla,
    pub text_soft: Hsla,
    pub muted: Hsla,
    pub subtle: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub accent_text: Hsla,
    pub mine_bubble: Hsla,
    pub input: Hsla,
    pub danger: Hsla,
    pub error_background: Hsla,
    pub error_text: Hsla,
    pub brand_top: Hsla,
    pub brand_bottom: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Theme {
    pub fn for_dark(dark: bool) -> Self {
        let pick = |light: u32, dark_hex: u32| c(if dark { dark_hex } else { light });
        Self {
            background: pick(0xF4F4F9, 0x1B1B22),
            surface: pick(0xFFFFFF, 0x24242C),
            conversation: pick(0xF7F7FB, 0x1F1F26),
            border: pick(0xE4E4EE, 0x34343F),
            border_strong: pick(0xD6D6E4, 0x44444F),
            dark,
            bubble_border: pick(0xE6E6F0, 0x383843),
            text: pick(0x1E1E2C, 0xEDEDF4),
            text_soft: pick(0x3F3F52, 0xC9C9D6),
            muted: pick(0x5C5C70, 0xA4A4B6),
            subtle: pick(0x6B6B80, 0x8F8FA3),
            accent: pick(0x5B5FC7, 0x8A8DF0),
            accent_soft: pick(0xECECF9, 0x34355C),
            accent_text: pick(0x4B4FB8, 0xB4B6FA),
            mine_bubble: pick(0xE5E7FA, 0x34365E),
            input: pick(0xF0F0F6, 0x2C2C35),
            danger: pick(0xC4314B, 0xF06A80),
            error_background: pick(0xFCE8EC, 0x4A2029),
            error_text: pick(0x8E1F33, 0xFFC2CC),
            brand_top: c(0x7B7FE0),
            brand_bottom: c(0x4B4FB8),
        }
    }

    /// Same seed → same colour as `Theme.avatar` in Swift.
    pub fn avatar(seed: &str) -> Hsla {
        const AVATARS: [u32; 6] = [0x5B5FC7, 0xB4467A, 0x2F7D6D, 0xA35A12, 0x3D6FB6, 0x7A4FB5];
        let hash = seed
            .bytes()
            .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        c(AVATARS[(hash % AVATARS.len() as u32) as usize])
    }

    pub fn presence(status: teams_core::PresenceStatus, out_of_office: bool) -> Option<Hsla> {
        use teams_core::PresenceStatus::*;
        if out_of_office {
            return Some(c(0xB4009E));
        }
        Some(c(match status {
            Available => 0x6BB700,
            Busy | DoNotDisturb => 0xD13438,
            BeRightBack | Away => 0xF8A800,
            Offline => 0x8A8886,
            Unknown => return None,
        }))
    }
}
