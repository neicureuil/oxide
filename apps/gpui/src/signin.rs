//! The sign-in screen: Microsoft's page opens in the default browser and the return address is pasted back.
use crate::oxide::{Oxide, text, tr};
use crate::icons::{Icon, icon};
use gpui::{AnyElement, Context, FontWeight, div, img, linear_color_stop, linear_gradient, prelude::*, px};
use teams_core::{Action, Shell};

pub fn screen(this: &mut Oxide, shell: &Shell, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    // The violet half (`BrandPanel`): the same in light and dark mode.
    let feature = |glyph: Icon, label: String| {
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .size(px(30.))
                    .rounded(px(8.))
                    .bg(gpui::white().opacity(0.16))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(glyph, px(14.), gpui::white())),
            )
            .child(div().text_size(px(14.)).opacity(0.92).child(label))
    };
    let brand = div()
        .relative()
        .overflow_hidden()
        .w(px(440.))
        .h_full()
        .flex_none()
        .bg(linear_gradient(
            135.,
            linear_color_stop(theme.brand_top, 0.),
            linear_color_stop(theme.brand_bottom, 1.),
        ))
        .text_color(gpui::white())
        .child(
            img("brand/bubble-white-420x400.svg")
                .absolute()
                .left(px(170.))
                .top(px(420.))
                .w(px(420.))
                .h(px(400.))
                .opacity(0.07),
        )
        .child(
            div()
                .size_full()
                .p(px(40.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(img("brand/mark-white-30x28.svg").w(px(30.)).h(px(28.)))
                        .child(div().text_size(px(19.)).font_weight(FontWeight::SEMIBOLD).child("Oxide")),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(30.))
                        .line_height(px(38.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(tr("ui.sign_in_view.microsoft_teams_native_on_your_mac")),
                )
                .child(
                    div()
                        .pt(px(28.))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(feature(Icon::Chat, tr("ui.sign_in_view.chats_teams_and_channels")))
                        .child(feature(Icon::Video, tr("ui.sign_in_view.calls_and_meetings")))
                        .child(feature(Icon::Calendar, tr("ui.sign_in_view.calendar_and_files"))),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(11.))
                        .opacity(0.65)
                        .child(tr("ui.sign_in_view.independent_client_not_affiliated_with_microsoft")),
                ),
        );

    let core = this.core.clone();
    let busy = shell.signing_in;
    let primary = |id: &'static str, label: String, enabled: bool| {
        div()
            .id(id)
            .mt_6()
            .h(px(40.))
            .w_full()
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.accent)
            .text_color(gpui::white())
            .font_weight(FontWeight::SEMIBOLD)
            .when(enabled, |this| this.cursor_pointer().hover(|s| s.opacity(0.9)))
            .when(!enabled, |this| this.opacity(0.6))
            .child(label)
    };

    let mut form = div()
        .w(px(360.))
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.sign_in_view.welcome")),
        )
        .child(
            div()
                .mt_2()
                .text_size(px(14.))
                .line_height(px(20.))
                .text_color(theme.muted)
                .child(tr("ui.sign_in_view.sign_in_with_your_microsoft_work")),
        );
    if shell.browser_login && !busy {
        let paste = this.core.clone();
        form = form
            .child(
                div()
                    .mt_6()
                    .text_size(px(13.))
                    .line_height(px(19.))
                    .text_color(theme.text_soft)
                    .child(text(
                        "Connectez-vous dans le navigateur qui vient de s'ouvrir. À la fin, la page de Teams s'affiche : copiez son adresse (⌘L puis ⌘C) et revenez ici.",
                        "Sign in in the browser that just opened. When Teams' page shows, copy its address (⌘L then ⌘C) and come back here.",
                    )),
            )
            .child(
                primary(
                    "paste-callback",
                    text("Coller l'adresse de retour", "Paste the return address").into(),
                    true,
                )
                .on_click(move |_, _, cx| {
                    let value = cx
                        .read_from_clipboard()
                        .and_then(|item| item.text())
                        .unwrap_or_default();
                    paste.send(Action::CallbackChanged {
                        value: value.trim().to_owned(),
                    });
                    paste.send(Action::CompleteSignIn);
                }),
            );
    } else {
        let label = if busy {
            tr("ui.sign_in_view.signing_in")
        } else {
            tr("ui.sign_in_view.sign_in_with_microsoft")
        };
        // Microsoft's page opens in the app's own sign-in window (`Effect::OpenLogin`).
        form = form.child(primary("sign-in", label, !busy).on_click(move |_, _, _| {
            if !busy {
                core.send(Action::SignIn);
            }
        }));
        // Should the sign-in window not open on this system, the default browser can do it.
        if !busy {
            let browser = this.core.clone();
            form = form.child(
                div()
                    .id("sign-in-browser")
                    .mt_3()
                    .text_size(px(12.))
                    .text_color(this.theme.accent_text)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child(text("Se connecter dans le navigateur", "Sign in in the browser"))
                    .on_click(move |_, _, _| browser.send(Action::SignInBrowser)),
            );
        }
    }
    if let Some(error) = &shell.error {
        form = form.child(
            div()
                .mt_4()
                .p_3()
                .rounded(px(8.))
                .bg(theme.error_background)
                .text_color(theme.error_text)
                .text_size(px(12.))
                .child(error.clone()),
        );
    }
    form = form.child(
        div()
            .mt_4()
            .text_size(px(12.))
            .text_color(theme.muted)
            .child(tr("ui.sign_in_view.you_sign_in_on_microsoft_s")),
    );
    let _ = cx;
    div()
        .size_full()
        .flex()
        .child(brand)
        .child(
            div()
                .flex_1()
                .h_full()
                .bg(theme.surface)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(form),
        )
        .into_any_element()
}
