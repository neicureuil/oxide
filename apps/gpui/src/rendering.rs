//! How GPUI draws, chosen in Settings › Appearance and applied at the next launch (GPUI picks its device once, at
//! startup, from the environment). Nothing here patches GPUI:
//! - Linux: software rendering (CPU), by limiting the Vulkan loader to Mesa's lavapipe driver.
//! - Windows: GPUI's own switch to present without DirectComposition (`GPUI_DISABLE_DIRECT_COMPOSITION`).
//!
//! macOS has neither: GPUI draws with Metal only.
use crate::oxide::{Oxide, tr};
use crate::prefs::Prefs;
use crate::widgets;
use gpui::{AnyElement, Context, div, prelude::*, px};
use std::sync::OnceLock;

/// What this launch applied, to tell when the setting waits for a restart.
static APPLIED: OnceLock<Prefs> = OnceLock::new();

/// Sets the environment GPUI reads; call before `Application::new`, while the process has one thread.
pub fn apply(prefs: &Prefs) {
    let _ = APPLIED.set(prefs.clone());
    #[cfg(target_os = "linux")]
    if prefs.software_rendering {
        let drivers = lavapipe_drivers();
        if !drivers.is_empty() {
            let list = std::env::join_paths(&drivers).unwrap_or_default();
            // SAFETY: still single-threaded, before GPUI or any runtime starts.
            unsafe {
                std::env::set_var("VK_DRIVER_FILES", &list);
                // Older Vulkan loaders read this name.
                std::env::set_var("VK_ICD_FILENAMES", &list);
            }
        }
    }
    #[cfg(windows)]
    if prefs.no_direct_composition && std::env::var_os("GPUI_DISABLE_DIRECT_COMPOSITION").is_none()
    {
        // SAFETY: still single-threaded, before GPUI or any runtime starts.
        unsafe { std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "1") };
    }
}

/// Mesa's CPU Vulkan driver manifests (`lvp_icd.*.json`), where distributions install them.
#[cfg(target_os = "linux")]
fn lavapipe_drivers() -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for dir in [
        "/usr/share/vulkan/icd.d",
        "/usr/local/share/vulkan/icd.d",
        "/etc/vulkan/icd.d",
    ] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("lvp_icd") && name.ends_with(".json") {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found
}

#[allow(clippy::too_many_arguments)]
/// One setting: title, its toggle, what it does, and a note while it waits for a restart.
fn row(
    this: &mut Oxide,
    id: &'static str,
    title: String,
    detail: String,
    on: bool,
    pending: bool,
    enabled: bool,
    toggle: fn(&mut Prefs),
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let switch = widgets::toggle(id, on, &theme);
    let switch = if enabled {
        switch
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(&mut this.prefs);
                this.prefs.save();
                cx.notify();
            }))
            .into_any_element()
    } else {
        widgets::disabled(switch, true).into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(crate::settings_controls::card_title(title).flex_1())
                .child(switch),
        )
        .child(widgets::muted(detail, 12., &theme))
        .when(pending, |d| {
            d.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.accent)
                    .child(tr("ui.rendering.restart")),
            )
        })
        .into_any_element()
}

/// Settings › Appearance › Rendering; `None` on macOS, where there is nothing to choose.
pub fn section(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    #[cfg(any(target_os = "linux", windows))]
    let applied = APPLIED.get().cloned().unwrap_or_default();
    #[cfg(target_os = "linux")]
    {
        let available = !lavapipe_drivers().is_empty();
        let on = this.prefs.software_rendering;
        let detail = if available || on {
            tr("ui.rendering.software_detail")
        } else {
            tr("ui.rendering.software_unavailable")
        };
        let row = row(
            this,
            "software-rendering",
            tr("ui.rendering.software"),
            detail,
            on,
            on != applied.software_rendering,
            available || on,
            |prefs| prefs.software_rendering = !prefs.software_rendering,
            cx,
        );
        return Some(card(this, row));
    }
    #[cfg(windows)]
    {
        // Shown as "use DirectComposition", on by default.
        let on = !this.prefs.no_direct_composition;
        let row = row(
            this,
            "direct-composition",
            tr("ui.rendering.direct_composition"),
            tr("ui.rendering.direct_composition_detail"),
            on,
            on == applied.no_direct_composition,
            true,
            |prefs| prefs.no_direct_composition = !prefs.no_direct_composition,
            cx,
        );
        return Some(card(this, row));
    }
    #[allow(unreachable_code)]
    {
        let _ = (this, cx, row);
        None
    }
}

fn card(this: &Oxide, row: AnyElement) -> AnyElement {
    crate::settings_controls::card(&this.theme)
        .gap_3()
        .child(crate::settings_controls::card_title(tr(
            "ui.rendering.title",
        )))
        .child(row)
        .into_any_element()
}
