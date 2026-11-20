//! "Check for updates" at the foot of Settings (Updater.swift), informational only: this build is not an
//! Oxide.app bundle the updater could replace, so a newer release opens its GitHub page instead of installing.
use crate::oxide::{Oxide, tr, trf};
use crate::settings_controls::heading;
use crate::widgets::{self, ButtonKind};
use gpui::{AnyElement, Context, div, prelude::*, px};

const LATEST: &str = "https://api.github.com/repos/neicureuil/oxide/releases/latest";

/// A published release: what the sheet shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    /// The release sheet is open.
    Prompt(Release),
    Failed(String),
}

/// Compares dotted versions number by number ("0.10.0" > "0.9.1"); a leading "v" and a "-beta" are ignored.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        let trimmed = version.strip_prefix('v').unwrap_or(version);
        let core = trimmed.split(['-', '+']).next().unwrap_or_default();
        core.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }
    let (a, b) = (parts(candidate), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// Parses GitHub's `releases/latest` answer; headings of the notes become plain lines.
pub fn parse_release(json: &[u8]) -> Option<Release> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    let tag = value.get("tag_name")?.as_str()?;
    let notes = value
        .get("body")
        .and_then(|b| b.as_str())
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .lines()
        .map(|line| line.trim_start_matches('#').trim_start().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    Some(Release {
        version: tag.strip_prefix('v').unwrap_or(tag).to_owned(),
        notes,
        page: value
            .get("html_url")
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_owned(),
    })
}

/// Asks GitHub through `curl` (this app has no HTTP client of its own), off the main thread.
fn fetch() -> Result<Release, String> {
    let out = std::process::Command::new("curl")
        .args([
            "-sSfL",
            "--max-time",
            "20",
            "-H",
            "Accept: application/vnd.github+json",
            LATEST,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    parse_release(&out.stdout).ok_or_else(|| "invalid answer".to_owned())
}

fn check(this: &mut Oxide, cx: &mut Context<Oxide>) {
    if this.ui.settings.update == UpdateState::Checking {
        return;
    }
    this.ui.settings.update = UpdateState::Checking;
    cx.notify();
    cx.spawn(async move |this, cx| {
        let result = cx.background_executor().spawn(async { fetch() }).await;
        let _ = this.update(cx, |this, cx| {
            this.ui.settings.update = match result {
                Ok(release) if is_newer(&release.version, &teams_core::app_version()) => {
                    UpdateState::Available(release)
                }
                Ok(_) => UpdateState::UpToDate,
                Err(detail) => {
                    UpdateState::Failed(trf("ui.updater.check_failed", &[("detail", &detail)]))
                }
            };
            cx.notify();
        });
    })
    .detach();
}

/// The button and its status line; none in the demo, as the macOS app.
pub fn row(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    if this.demo {
        return None;
    }
    let theme = this.theme;
    let state = this.ui.settings.update.clone();
    let button = match &state {
        UpdateState::Available(release) | UpdateState::Prompt(release) => {
            let release = release.clone();
            widgets::button(
                "update-install",
                trf(
                    "ui.updater.install_version",
                    &[("version", &release.version)],
                ),
                ButtonKind::Primary,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.ui.settings.update = UpdateState::Prompt(release.clone());
                cx.notify();
            }))
        }
        _ => widgets::disabled(
            widgets::button(
                "update-check",
                tr("ui.updater.check"),
                ButtonKind::Outline,
                &theme,
            ),
            state == UpdateState::Checking,
        )
        .on_click(cx.listener(|this, _, _, cx| check(this, cx))),
    };
    let status = match &state {
        UpdateState::Checking => Some(tr("ui.updater.checking")),
        UpdateState::UpToDate => Some(tr("ui.updater.up_to_date")),
        UpdateState::Failed(message) => Some(message.clone()),
        _ => None,
    };
    Some(
        div()
            .pt_1()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .child(button)
            .when_some(status, |d, text| d.child(widgets::muted(text, 11., &theme)))
            .into_any_element(),
    )
}

/// The release sheet: notes, and the release page to download it by hand.
pub fn sheet(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let UpdateState::Prompt(release) = this.ui.settings.update.clone() else {
        return None;
    };
    let theme = this.theme;
    let kept = release.clone();
    let later =
        move |this: &mut Oxide| this.ui.settings.update = UpdateState::Available(kept.clone());
    let page = release.page.clone();
    let content = div()
        .p(px(24.))
        .flex()
        .flex_col()
        .gap_3()
        .child(heading(trf(
            "ui.updater.available_title",
            &[("version", &release.version)],
        )))
        .child(widgets::muted(
            trf(
                "ui.updater.current_version",
                &[("version", &teams_core::app_version())],
            ),
            12.,
            &theme,
        ))
        .when(!release.notes.is_empty(), |d| {
            d.child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(tr("ui.updater.release_notes")),
            )
            .child(
                div()
                    .id("update-notes")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .p_3()
                    .rounded(px(8.))
                    .bg(theme.input)
                    .text_size(px(12.))
                    .child(release.notes.clone()),
            )
        })
        .child(widgets::muted(tr("ui.updater.manual_install"), 12., &theme))
        .child(
            div()
                .flex()
                .gap_2()
                .child(div().flex_1())
                .child(
                    widgets::button(
                        "update-later",
                        tr("ui.updater.later"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener({
                        let later = later.clone();
                        move |this, _, _, cx| {
                            later(this);
                            cx.notify();
                        }
                    })),
                )
                .child(
                    widgets::button(
                        "update-open",
                        tr("ui.updater.open_release_page"),
                        ButtonKind::Primary,
                        &theme,
                    )
                    .on_click(cx.listener({
                        let later = later.clone();
                        move |this, _, _, cx| {
                            cx.open_url(&page);
                            later(this);
                            cx.notify();
                        }
                    })),
                ),
        );
    let entity = cx.entity().downgrade();
    Some(widgets::modal(
        "update-sheet",
        px(460.),
        content,
        &theme,
        move |_, cx| {
            let _ = entity.update(cx, |this, cx| {
                later(this);
                cx.notify();
            });
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.10.0", "0.9.1"));
        assert!(is_newer("v0.12.0", "0.11.0"));
        assert!(!is_newer("0.11.0", "0.11.0"));
        assert!(!is_newer("0.11.0-beta", "0.11.0"));
        assert!(is_newer("1.0", "0.99.99"));
    }

    #[test]
    fn parses_release() {
        let json = br###"{"tag_name":"v0.12.0","body":"## New\r\n- faster","html_url":"https://github.com/x/y/releases/tag/v0.12.0"}"###;
        let release = parse_release(json).unwrap();
        assert_eq!(release.version, "0.12.0");
        assert_eq!(release.notes, "New\n- faster");
        assert!(release.page.ends_with("v0.12.0"));
    }
}
