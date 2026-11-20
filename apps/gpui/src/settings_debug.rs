//! Settings › Debug: what uses memory, measured every two seconds while the page is open (DebugSettings.swift).
use crate::oxide::{Oxide, tr, trf};
use crate::settings::Page;
use crate::settings_controls::{card, card_title};
use crate::settings_memory::{ImageMemory, measure, size};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{AnyElement, Context, Div, FontWeight, RenderImage, div, prelude::*, px, relative};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

fn images(map: &HashMap<String, Arc<RenderImage>>) -> ImageMemory {
    let bytes = map
        .values()
        .map(|image| {
            (0..image.frame_count())
                .map(|f| {
                    let s = image.size(f);
                    (s.width.0.max(0) as u64) * (s.height.0.max(0) as u64) * 4
                })
                .sum::<u64>()
        })
        .sum();
    ImageMemory {
        count: map.len(),
        bytes,
    }
}

/// Starts measuring; the loop ends by itself once the page is left.
pub fn entered(this: &mut Oxide, cx: &mut Context<Oxide>) {
    if this.ui.settings.measuring {
        return;
    }
    this.ui.settings.measuring = true;
    cx.spawn(async move |this, cx| {
        loop {
            let report = cx.background_executor().spawn(async { measure() }).await;
            let more = this
                .update(cx, |this, cx| {
                    // Leaving Settings sends it back to its home (`settings::track_nav`).
                    let keep = this.ui.settings.page == Some(Page::Debug);
                    if keep {
                        let mut report = report;
                        report.avatars = images(&this.avatars.images);
                        report.previews = images(&this.images.images);
                        this.ui.settings.report = Some(report);
                        cx.notify();
                    } else {
                        this.ui.settings.measuring = false;
                        this.ui.settings.report = None;
                    }
                    keep
                })
                .unwrap_or(false);
            if !more {
                return;
            }
            cx.background_executor().timer(Duration::from_secs(2)).await;
        }
    })
    .detach();
}

/// A labelled bar, its length the share of the whole.
fn row(title: String, value: String, fraction: f64, detail: Option<String>, theme: &Theme) -> Div {
    let fraction = if fraction.is_finite() {
        fraction.clamp(0., 1.)
    } else {
        0.
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().text_size(px(13.)).child(title))
                .when_some(detail, |d, text| d.child(widgets::muted(text, 12., theme)))
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(value),
                ),
        )
        .child(
            div()
                .h(px(5.))
                .w_full()
                .rounded_full()
                .bg(theme.input)
                .child(
                    div()
                        .h_full()
                        .w(relative(fraction as f32))
                        .rounded_full()
                        .bg(theme.accent),
                ),
        )
}

pub fn section(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let Some(report) = this.ui.settings.report.clone() else {
        return widgets::spinner("debug-loading", 16., theme.muted);
    };
    let process = card(&theme)
        .gap(px(6.))
        .child(card_title(tr("ui.debug.footprint")))
        .child(
            div()
                .text_size(px(28.))
                .line_height(px(34.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(size(report.footprint)),
        )
        .child(widgets::muted(
            trf(
                "ui.debug.footprint_detail",
                &[
                    ("peak", &size(report.peak)),
                    ("resident", &size(report.resident)),
                ],
            ),
            12.,
            &theme,
        ));

    let largest = report.kinds.first().map(|k| k.1).unwrap_or(1).max(1) as f64;
    let mut kinds = card(&theme)
        .gap(px(10.))
        .child(card_title(tr("ui.debug.breakdown")));
    for (kind, bytes) in &report.kinds {
        kinds = kinds.child(row(
            kind.label(),
            size(*bytes),
            *bytes as f64 / largest,
            None,
            &theme,
        ));
    }
    kinds = kinds.child(widgets::muted(tr("ui.debug.breakdown_detail"), 12., &theme));

    // The Rust allocator counter of the core (`rust_memory`) is not exported to Rust front ends yet: the heap
    // shows its total and the pictures this window keeps.
    let total = report.heap_in_use.max(1) as f64;
    let heap = card(&theme)
        .gap(px(10.))
        .child(card_title(tr("ui.debug.heap")))
        .child(widgets::muted(
            trf(
                "ui.debug.heap_detail",
                &[
                    ("used", &size(report.heap_in_use)),
                    ("reserved", &size(report.heap_allocated)),
                ],
            ),
            12.,
            &theme,
        ))
        .child(row(
            tr("ui.debug.avatars"),
            size(report.avatars.bytes),
            report.avatars.bytes as f64 / total,
            Some(trf(
                "ui.debug.image_count",
                &[("n", &report.avatars.count.to_string())],
            )),
            &theme,
        ))
        .child(row(
            tr("ui.debug.previews"),
            size(report.previews.bytes),
            report.previews.bytes as f64 / total,
            Some(trf(
                "ui.debug.image_count",
                &[("n", &report.previews.count.to_string())],
            )),
            &theme,
        ));

    let cache_total = report.cache_preview_bytes + report.cache_photo_bytes;
    let cache_scale = cache_total.max(1) as f64;
    let empty_cache = report.cache_preview_files + report.cache_photo_files == 0;
    let disk = card(&theme)
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .child(card_title(tr("ui.debug.disk_cache")))
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(size(cache_total)),
                ),
        )
        .child(widgets::muted(
            tr("ui.debug.disk_cache_detail"),
            12.,
            &theme,
        ))
        .child(row(
            tr("ui.debug.cache_previews"),
            size(report.cache_preview_bytes),
            report.cache_preview_bytes as f64 / cache_scale,
            Some(trf(
                "ui.debug.file_count",
                &[("n", &report.cache_preview_files.to_string())],
            )),
            &theme,
        ))
        .child(row(
            tr("ui.debug.cache_photos"),
            size(report.cache_photo_bytes),
            report.cache_photo_bytes as f64 / cache_scale,
            Some(trf(
                "ui.debug.file_count",
                &[("n", &report.cache_photo_files.to_string())],
            )),
            &theme,
        ))
        .child(
            div().flex().child(
                widgets::disabled(
                    widgets::button(
                        "debug-clear-cache",
                        tr("ui.debug.clear_cache"),
                        ButtonKind::Outline,
                        &theme,
                    ),
                    empty_cache,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    if empty_cache {
                        return;
                    }
                    let cleared = cx
                        .background_executor()
                        .spawn(async { teams_core::clear_picture_cache() });
                    cx.spawn(async move |this, cx| {
                        cleared.await;
                        let report = cx.background_executor().spawn(async { measure() }).await;
                        let _ = this.update(cx, |this, cx| {
                            if let Some(old) = this.ui.settings.report.as_ref() {
                                let mut report = report;
                                report.avatars = old.avatars;
                                report.previews = old.previews;
                                this.ui.settings.report = Some(report);
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                })),
            ),
        );

    let text = report.text();
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(process)
        .child(kinds)
        .child(heap)
        .child(disk)
        .child(
            div().flex().child(
                widgets::button(
                    "debug-copy",
                    tr("ui.debug.copy_report"),
                    ButtonKind::Outline,
                    &theme,
                )
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()))
                }),
            ),
        )
        .into_any_element()
}
