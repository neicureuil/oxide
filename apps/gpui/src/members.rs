//! The Participants tab of a chat or channel (`MembersScreen` in `Screens.swift`, with the group chat's
//! `GroupChatControls` from `ChatAdmin.swift`) (owner: teams agent).
use crate::icons::Icon;
use crate::input::{InputEvent, TextInput};
use crate::oxide::{Oxide, tr, trf};
use crate::teams_admin::confirmation;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, Context, Entity, FontWeight, SharedString, Subscription, Window, div, prelude::*,
    px, uniform_list,
};
use teams_core::{Action, AvatarShape, ChatAdminAction, MemberView};

pub struct Ui {
    /// Local filter on name and role, for long channel rosters.
    filter: Entity<TextInput>,
    /// The group chat's name, as typed.
    name: Entity<TextInput>,
    /// The chat name last put in the field, to follow renames from elsewhere.
    synced_name: Option<String>,
    /// The member whose removal waits for confirmation: (id, name).
    removing: Option<(String, String)>,
    confirm_leave: bool,
    _subscriptions: Vec<Subscription>,
}
impl Ui {
    pub fn new(_window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let filter = cx.new(|cx| TextInput::new(cx, tr("ui.screens.filter_participants")));
        let name = cx.new(|cx| TextInput::new(cx, tr("ui.chat_admin.name_the_chat")));
        let filtered = cx.subscribe(&filter, |_: &mut Oxide, _, _: &InputEvent, cx| cx.notify());
        let named = cx.subscribe(
            &name,
            |this: &mut Oxide, _, event: &InputEvent, cx| match event {
                InputEvent::Submit => rename(this, cx),
                _ => cx.notify(),
            },
        );
        Self {
            filter,
            name,
            synced_name: None,
            removing: None,
            confirm_leave: false,
            _subscriptions: vec![filtered, named],
        }
    }
}

fn rename(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let title = this.ui.members.name.read(cx).text().to_owned();
    this.send(Action::ChatAdmin {
        action: ChatAdminAction::Rename { title },
    });
}

/// Whether a member matches the filter, on name or role, ignoring case (Swift's `localizedStandardContains`).
pub(crate) fn matches(member: &MemberView, filter: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    filter.is_empty()
        || member.name.to_lowercase().contains(&filter)
        || member.role.to_lowercase().contains(&filter)
}

pub fn tab(this: &mut Oxide, _window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let view = this.members.clone();
    let core = this.core.clone();
    let header = div()
        .flex_none()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .text_size(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.screens.participants")),
        )
        .when(view.as_ref().is_some_and(|v| v.is_channel), |d| {
            d.child(
                widgets::icon_button_sized(
                    "members-refresh",
                    Icon::Refresh,
                    34.,
                    16.,
                    false,
                    &theme,
                )
                .tooltip(widgets::tooltip(
                    tr("ui.screens.refresh_participants"),
                    theme,
                ))
                .on_click(move |_, _, _| core.send(Action::ChannelMembers)),
            )
        });
    let mut page = div()
        .size_full()
        .p_6()
        .flex()
        .flex_col()
        .gap_4()
        .child(header);
    let Some(view) = view else {
        return page.into_any_element();
    };
    if view.can_manage {
        page = page.child(group_controls(this, &view.chat_name, cx));
    }
    if let Some(members) = &view.members {
        let filter = this.ui.members.filter.read(cx).text().to_owned();
        if members.len() > 6 {
            page = page.child(div().flex_none().child(widgets::text_field(
                &this.ui.members.filter,
                &theme,
                cx,
            )));
        }
        let shown: Vec<usize> = (0..members.len())
            .filter(|&i| matches(&members[i], &filter))
            .collect();
        let count = members.len().to_string();
        page = page.child(widgets::muted(
            if shown.len() == members.len() {
                trf("ui.screens.participants_2", &[("membersCount", &count)])
            } else {
                trf(
                    "ui.screens.of_participants",
                    &[
                        ("shownCount", &shown.len().to_string()),
                        ("membersCount", &count),
                    ],
                )
            },
            13.,
            &theme,
        ));
        if !shown.is_empty() {
            let can_manage = view.can_manage;
            page = page.child(
                uniform_list(
                    "members",
                    shown.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|ix| member_row(this, shown[ix], can_manage, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1(),
            );
        }
        if members.is_empty() {
            page = page.child(widgets::muted(
                tr("ui.screens.the_service_returned_no_participants"),
                13.,
                &theme,
            ));
        }
    }
    if view.loading {
        page = page.child(widgets::loading(
            "members-loading",
            tr("ui.screens.loading_participants"),
            &theme,
        ));
    }
    if let Some(error) = &view.error {
        let core = this.core.clone();
        page = page
            .child(widgets::muted(
                trf("ui.screens.couldn_t_load_participants", &[("error", error)]),
                13.,
                &theme,
            ))
            .child(
                div().flex().child(
                    widgets::button(
                        "members-retry",
                        tr("ui.screens.try_again"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(move |_, _, _| core.send(Action::ChannelMembers)),
                ),
            );
    }
    page.into_any_element()
}

/// One participant's card, with the gap below it (the list's rows must share one height).
fn member_row(
    this: &mut Oxide,
    ix: usize,
    can_manage: bool,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    let theme = this.theme;
    let member = this.members.as_ref()?.members.as_ref()?.get(ix)?.clone();
    let avatar = this.avatar(
        &member.id,
        &member.name,
        36.,
        AvatarShape::Round,
        member.presence.as_ref(),
    );
    let me = this
        .shell
        .as_ref()
        .map(|s| s.me.to_lowercase())
        .unwrap_or_default();
    let removable = can_manage && member.id.to_lowercase() != me;
    let (id, name) = (member.id.clone(), member.name.clone());
    Some(
        div()
            .pb_3()
            .child(
                widgets::card(&theme)
                    .p_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(avatar)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(14.))
                                    .child(member.name.clone()),
                            )
                            .child(div().truncate().child(widgets::muted(
                                member.role.clone(),
                                13.,
                                &theme,
                            ))),
                    )
                    .when(removable, |d| {
                        d.child(
                            widgets::icon_button_sized(
                                SharedString::from(format!("member-remove-{}", member.id)),
                                Icon::UserMinus,
                                34.,
                                16.,
                                false,
                                &theme,
                            )
                            .tooltip(widgets::tooltip(
                                trf("ui.chat_admin.remove_from_chat", &[("name", &member.name)]),
                                theme,
                            ))
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.ui.members.removing = Some((id.clone(), name.clone()));
                                    cx.notify();
                                },
                            )),
                        )
                    }),
            )
            .into_any_element(),
    )
}

/// `GroupChatControls`: name, "Add people" and "Leave" of the open group chat.
fn group_controls(this: &mut Oxide, chat_name: &str, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    if this.ui.members.synced_name.as_deref() != Some(chat_name) {
        this.ui.members.synced_name = Some(chat_name.to_owned());
        let name = chat_name.to_owned();
        this.ui
            .members
            .name
            .update(cx, |input, cx| input.set_text(name, None, cx));
    }
    let busy = this.chat_admin.as_ref().is_some_and(|a| a.busy);
    let unchanged = this.ui.members.name.read(cx).text().trim() == chat_name;
    let core = this.core.clone();
    widgets::card(&theme)
        .flex_none()
        .p_4()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.chat_admin.group_name")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().min_w_0().child(widgets::text_field(
                    &this.ui.members.name,
                    &theme,
                    cx,
                )))
                .child(
                    widgets::disabled(
                        widgets::button(
                            "group-rename",
                            tr("ui.chat_admin.save"),
                            ButtonKind::Outline,
                            &theme,
                        ),
                        unchanged || busy,
                    )
                    .when(!unchanged && !busy, |d| {
                        d.on_click(cx.listener(|this, _, _, cx| rename(this, cx)))
                    }),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    widgets::button(
                        "group-add-people",
                        tr("ui.chat_admin.add_people"),
                        ButtonKind::Primary,
                        &theme,
                    )
                    // The icon leads, as in Swift's `Label`.
                    .flex_row_reverse()
                    .child(crate::icons::icon(Icon::UserPlus, px(15.), gpui::white()))
                    .on_click(move |_, _, _| {
                        core.send(Action::ChatAdmin {
                            action: ChatAdminAction::StartAdding,
                        })
                    }),
                )
                .child(
                    widgets::button(
                        "group-leave",
                        tr("ui.chat_admin.leave"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .text_color(theme.danger)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ui.members.confirm_leave = true;
                        cx.notify();
                    })),
                )
                .when(busy, |d| {
                    d.child(widgets::spinner("group-busy", 14., theme.muted))
                }),
        )
        .into_any_element()
}

fn close_remove(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui.members.removing = None;
    cx.notify();
}
fn close_leave(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui.members.confirm_leave = false;
    cx.notify();
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    this: &mut Oxide,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    let theme = this.theme;
    // Both confirmations belong to the open conversation's tab: dropped once it is gone.
    if this.members.is_none() {
        this.ui.members.removing = None;
        this.ui.members.confirm_leave = false;
    }
    if let Some((id, name)) = this.ui.members.removing.clone() {
        return Some(confirmation(
            "remove-member",
            trf("ui.chat_admin.remove_question", &[("name", &name)]),
            None,
            tr("ui.chat_admin.remove"),
            &theme,
            cx,
            move |this, cx| {
                this.send(Action::ChatAdmin {
                    action: ChatAdminAction::RemoveMember { mri: id.clone() },
                });
                close_remove(this, cx);
            },
            close_remove,
        ));
    }
    if this.ui.members.confirm_leave {
        let chat_name = this
            .members
            .as_ref()
            .map(|m| m.chat_name.clone())
            .unwrap_or_default();
        let title = if chat_name.is_empty() {
            tr("ui.chat_admin.this_chat")
        } else {
            chat_name
        };
        return Some(confirmation(
            "leave-group",
            trf("ui.chat_admin.leave_question", &[("title", &title)]),
            Some(tr("ui.chat_admin.leave_explanation")),
            tr("ui.chat_admin.leave"),
            &theme,
            cx,
            |this, cx| {
                if let Some(id) = this
                    .shell
                    .as_ref()
                    .and_then(|s| s.selected.as_ref())
                    .map(|t| t.id.clone())
                {
                    this.send(Action::ChatAdmin {
                        action: ChatAdminAction::Leave { chat_id: id },
                    });
                }
                close_leave(this, cx);
            },
            close_leave,
        ));
    }
    None
}

/// A step of `OXIDE_SCRIPT` this module understands (see `screens::run_script`); false if not its own.
/// `members:open` shows the tab, `members:filter <text>` types in the filter, `members:remove <n>` asks to remove the n-th participant,
/// `members:leave` asks to leave the group chat.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(rest) = step.strip_prefix("members:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    match verb {
        "filter" => {
            let text = arg.to_owned();
            this.ui
                .members
                .filter
                .update(cx, |input, cx| input.set_text(text, None, cx));
        }
        "remove" => {
            let member = arg
                .parse::<usize>()
                .ok()
                .and_then(|n| this.members.as_ref()?.members.as_ref()?.get(n).cloned());
            let Some(member) = member else { return false };
            this.ui.members.removing = Some((member.id, member.name));
        }
        "leave" => this.ui.members.confirm_leave = true,
        "open" => this.send(Action::SetTab {
            tab: teams_core::Tab::Members,
        }),
        _ => return false,
    }
    cx.notify();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matches_name_or_role_ignoring_case() {
        let member = MemberView {
            id: "8:orgid:1".into(),
            name: "Claire Martin".into(),
            role: "Owner".into(),
            presence: None,
        };
        assert!(matches(&member, ""));
        assert!(matches(&member, "claire"));
        assert!(matches(&member, " OWN "));
        assert!(!matches(&member, "bob"));
    }
}
