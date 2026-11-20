//! Settings › Out of office: automatic replies, and when they are sent (OutOfOffice.swift).
use crate::oxide::{Oxide, tr};
use crate::settings_controls::{self as controls, switch_row};
use crate::widgets::{self, ButtonKind};
use gpui::{AnyElement, Context, FontWeight, Window, div, prelude::*, px};
use teams_core::{Action, MeAction, OutOfOffice, OutsideAudience};

/// The page opened: read the settings again, the form waits for them.
pub fn entered(this: &mut Oxide, _cx: &mut Context<Oxide>) {
    this.ui.settings.ooo_draft = None;
    this.ui.settings.ooo_base = None;
    this.send(Action::Me {
        action: MeAction::LoadOutOfOffice,
    });
}

/// Takes the service's settings into the form when they change, unless a save is under way.
fn sync(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let Some(view) = this.me.as_ref().map(|m| m.out_of_office_settings.clone()) else {
        return;
    };
    let Some(settings) = view.settings else {
        return;
    };
    if view.saving || this.ui.settings.ooo_base.as_ref() == Some(&settings) {
        return;
    }
    this.ui.settings.ooo_base = Some(settings.clone());
    this.ui.settings.ooo_draft = Some(settings.clone());
    let internal = settings.internal_message.clone();
    let external = settings.external_message.clone();
    this.ui
        .settings
        .ooo_internal
        .update(cx, |i, cx| i.set_text(internal, None, cx));
    this.ui
        .settings
        .ooo_external
        .update(cx, |i, cx| i.set_text(external, None, cx));
}

fn edit(this: &mut Oxide, change: impl FnOnce(&mut OutOfOffice)) {
    if let Some(draft) = this.ui.settings.ooo_draft.as_mut() {
        change(draft);
    }
}

pub fn page(this: &mut Oxide, _window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    sync(this, cx);
    let theme = this.theme;
    let mut sections = Vec::new();
    match this.ui.settings.ooo_draft.clone() {
        Some(draft) => sections.push(form(this, draft, cx)),
        None => sections.push(widgets::spinner("ooo-loading", 16., theme.muted)),
    }
    if let Some(error) = this.me.as_ref().and_then(|m| m.error.clone()) {
        sections.push(
            controls::error_line("ooo-error-close", error, &theme, cx, |this| {
                this.send(Action::Me {
                    action: MeAction::DismissError,
                })
            })
            .into_any_element(),
        );
    }
    crate::settings::subpage(this, tr("ui.out_of_office.title"), sections, cx)
}

fn editor(this: &Oxide, external: bool, cx: &mut Context<Oxide>) -> AnyElement {
    let input = if external {
        &this.ui.settings.ooo_external
    } else {
        &this.ui.settings.ooo_internal
    };
    widgets::text_field(input, &this.theme, cx)
        .min_h(px(90.))
        .border_color(this.theme.border_strong)
        .into_any_element()
}

fn form(this: &mut Oxide, draft: OutOfOffice, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let view = this
        .me
        .as_ref()
        .map(|m| m.out_of_office_settings.clone())
        .unwrap_or_default();
    let mut body = controls::card(&theme)
        .p(px(20.))
        .gap_4()
        .child(switch_row(
            "ooo-enabled",
            tr("ui.out_of_office.turn_on"),
            None,
            draft.enabled,
            false,
            &theme,
            cx,
            |this, on, _| edit(this, |d| d.enabled = on),
        ))
        .child(widgets::muted(
            tr("ui.out_of_office.explanation"),
            12.,
            &theme,
        ));
    if draft.enabled {
        body = body
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr("ui.out_of_office.reply_inside")),
                    )
                    .child(editor(this, false, cx)),
            )
            .child(switch_row(
                "ooo-outside",
                tr("ui.out_of_office.reply_outside"),
                None,
                draft.reply_outside,
                false,
                &theme,
                cx,
                |this, on, _| edit(this, |d| d.reply_outside = on),
            ));
        if draft.reply_outside {
            let audience = draft.outside_audience;
            let radio = |id: &'static str,
                         value: OutsideAudience,
                         label: String,
                         cx: &mut Context<Oxide>| {
                widgets::radio(id, audience == value, label, &theme)
                    .text_size(px(13.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        edit(this, |d| d.outside_audience = value);
                        cx.notify();
                    }))
            };
            body = body
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(radio(
                            "ooo-contacts",
                            OutsideAudience::ContactsOnly,
                            tr("ui.out_of_office.contacts_only"),
                            cx,
                        ))
                        .child(radio(
                            "ooo-everyone",
                            OutsideAudience::All,
                            tr("ui.out_of_office.everyone_outside"),
                            cx,
                        )),
                )
                .child(editor(this, true, cx));
        }
        body = body.child(switch_row(
            "ooo-scheduled",
            tr("ui.out_of_office.only_during_period"),
            None,
            draft.scheduled,
            false,
            &theme,
            cx,
            |this, on, _| edit(this, |d| d.scheduled = on),
        ));
        if draft.scheduled {
            let start = controls::date_time_stepper(
                "ooo-start",
                draft.start_ms,
                None,
                &theme,
                cx,
                |this, ms| edit(this, |d| d.start_ms = ms),
            );
            let end = controls::date_time_stepper(
                "ooo-end",
                draft.end_ms,
                None,
                &theme,
                cx,
                |this, ms| edit(this, |d| d.end_ms = ms),
            );
            let line = |label: String, picker: gpui::Div| {
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().w(px(60.)).text_size(px(13.)).child(label))
                    .child(picker)
            };
            body = body
                .child(line(tr("ui.out_of_office.start"), start))
                .child(line(tr("ui.out_of_office.end"), end));
        }
    }
    let saving = view.saving;
    let disabled = saving || view.settings.as_ref() == Some(&draft);
    let label = tr(if saving {
        "ui.out_of_office.saving"
    } else {
        "ui.out_of_office.save"
    });
    body = body.child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                widgets::disabled(
                    widgets::button("ooo-save", label, ButtonKind::Primary, &theme),
                    disabled,
                )
                .on_click(cx.listener(move |this, _, _, _| {
                    if disabled {
                        return;
                    }
                    if let Some(settings) = this.ui.settings.ooo_draft.clone() {
                        this.send(Action::Me {
                            action: MeAction::SaveOutOfOffice { settings },
                        });
                    }
                })),
            )
            .when(view.loading, |d| {
                d.child(widgets::spinner("ooo-reloading", 14., theme.muted))
            }),
    );
    body.into_any_element()
}
