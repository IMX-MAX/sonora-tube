//! The Profiles tab: the guest profiles kept on this machine and the YouTube channels of the
//! Google account, one of which is in use at a time.

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{AnyElement, App, Context, SharedString, div, svg};
use i18n::t;
use state::{Subscriptions, VideoAccount};
use ui::{ActiveTheme as _, Avatar, Button, Text};

use super::{Group, SettingsView, Slot};

/// One profile as its row shows it.
struct Entry {
    /// What the row's id and its buttons' ids end in, unique among the rows.
    id: SharedString,
    name: SharedString,
    detail: Option<SharedString>,
    /// The picture of a YouTube channel. A profile with none shows an icon instead.
    avatar: Option<String>,
    used: bool,
    place: usize,
    count: usize,
}

impl SettingsView {
    pub(super) fn profiles_groups(&self, _: &App) -> Vec<Group> {
        vec![
            Group::new("settings-group-guests", "icons/user-round.svg").cards(Slot::GuestProfiles),
            Group::new("settings-group-google", "icons/circle-user-round.svg")
                .cards(Slot::GoogleProfiles),
        ]
    }

    /// The guest profiles, each with a way to use or delete it, and a button to start another.
    pub(super) fn guest_profiles(&self, cx: &mut Context<Self>) -> AnyElement {
        let (Some(kept), Some(account)) = (Subscriptions::entity(cx), VideoAccount::entity(cx))
        else {
            return div().into_any_element();
        };
        let guest = !account.read(cx).signed_in();
        let held = kept.read(cx);
        let (used, count) = (held.profile(), held.profiles().len());
        let rows = held
            .profiles()
            .iter()
            .enumerate()
            .map(|(place, &profile)| {
                let entry = Entry {
                    id: format!("guest-{profile}").into(),
                    name: t!("account-video-profile", number = profile),
                    detail: None,
                    avatar: None,
                    used: guest && profile == used,
                    place,
                    count,
                };
                let (picked, account) = (kept.clone(), account.clone());
                let pick = move |cx: &mut App| {
                    account.update(cx, |account, cx| account.leave(cx));
                    picked.update(cx, |kept, cx| kept.use_profile(profile, cx));
                };
                let remove = (place > 0).then(|| {
                    let kept = kept.clone();
                    Button::new(SharedString::from(format!("profile-remove-{profile}")))
                        .icon("icons/trash-2.svg")
                        .small()
                        .ghost()
                        .tooltip("settings-profile-remove")
                        .on_click(move |_, _, cx| {
                            kept.update(cx, |kept, cx| kept.remove_profile(profile, cx));
                        })
                });
                entry_row(entry, pick, remove, cx)
            })
            .collect::<Vec<_>>();
        let add = Button::new("profile-add")
            .label(t!("account-video-profile-new"))
            .icon("icons/plus.svg")
            .small()
            .outline()
            .on_click(move |_, _, cx| {
                account.update(cx, |account, cx| account.leave(cx));
                kept.update(cx, |kept, cx| kept.add_profile(cx));
            });

        section(t!("settings-guests-detail"), rows, vec![add], cx)
    }

    /// The YouTube channels of the Google session kept here, each with a way to use it, and
    /// the buttons to sign in, add an account or sign out.
    pub(super) fn google_profiles(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(account) = VideoAccount::entity(cx) else {
            return div().into_any_element();
        };
        let held = account.read(cx);
        let idle = held.supported() && !held.signing_in();
        let sign_in = |id: &'static str, label: SharedString, icon: &'static str| {
            let account = account.clone();
            Button::new(id)
                .label(label)
                .icon(icon)
                .small()
                .disabled(!idle)
                .on_click(move |_, _, cx| {
                    let title = t!("login-choose-title", provider = "YouTube").to_string();
                    account.update(cx, |account, cx| account.sign_in(title, cx));
                })
        };
        if !held.kept() {
            let button = sign_in(
                "profile-google-in",
                t!("settings-google-sign-in"),
                "icons/log-in.svg",
            )
            .primary();
            return section(t!("settings-google-detail"), Vec::new(), vec![button], cx);
        }

        let identities = held.identities();
        let count = identities.len().max(1);
        let mut rows = identities
            .iter()
            .enumerate()
            .map(|(place, identity)| {
                let page = identity.page.as_deref().unwrap_or("own");
                let entry = Entry {
                    id: format!("google-{}-{page}", identity.user).into(),
                    name: identity.name.clone().into(),
                    detail: identity.detail.clone().map(SharedString::from),
                    avatar: identity.avatar.clone(),
                    used: held.acts_as(identity),
                    place,
                    count,
                };
                let (account, chosen) = (account.clone(), identity.clone());
                let pick = move |cx: &mut App| {
                    account.update(cx, |account, cx| account.choose(&chosen, cx));
                };
                entry_row(entry, pick, None, cx)
            })
            .collect::<Vec<_>>();
        // The channels are only listed once the session was in use, so until then one row
        // stands for the account.
        if rows.is_empty() {
            let entry = Entry {
                id: "google-kept".into(),
                name: t!("settings-google-account"),
                detail: None,
                avatar: None,
                used: held.signed_in(),
                place: 0,
                count,
            };
            let account = account.clone();
            let pick = move |cx: &mut App| account.update(cx, |account, cx| account.resume(cx));
            rows.push(entry_row(entry, pick, None, cx));
        }
        let add = sign_in(
            "profile-google-add",
            t!("settings-google-add"),
            "icons/log-in.svg",
        )
        .outline();
        let leave = Button::new("profile-google-out")
            .label(t!("settings-google-sign-out"))
            .icon("icons/log-out.svg")
            .small()
            .ghost()
            .on_click(move |_, _, cx| account.update(cx, |account, cx| account.sign_out(cx)));

        section(t!("settings-google-detail"), rows, vec![add, leave], cx)
    }
}

/// A group's body: what it is for, its profiles as one bordered list, then its buttons.
fn section(
    detail: SharedString,
    rows: Vec<AnyElement>,
    buttons: Vec<Button>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let list = (!rows.is_empty()).then(|| {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .rounded(theme.radius)
            .corner_smoothing(theme.smoothing)
            .border_1()
            .border_color(theme.border)
            .children(rows)
    });

    div()
        .flex()
        .flex_col()
        .gap_3()
        .pb_4()
        .child(
            div()
                .text_size(theme.text(Text::Small))
                .text_color(theme.muted_foreground)
                .child(detail),
        )
        .children(list)
        .child(div().flex().items_center().gap_2().children(buttons))
        .into_any_element()
}

/// One profile: its picture, name and detail, then the mark of the one in use or the button
/// that makes it so, and `extra` after it.
fn entry_row(
    entry: Entry,
    pick: impl Fn(&mut App) + 'static,
    extra: Option<Button>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let size = theme.metrics.control;
    let picture = match entry.avatar {
        Some(url) => Avatar::new(Some(url)).size(size).into_any_element(),
        None => div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(size)
            .rounded_full()
            .bg(theme.secondary)
            .child(
                svg()
                    .path(icons::path("icons/user-round.svg"))
                    .size(size * 0.5)
                    .text_color(theme.muted_foreground),
            )
            .into_any_element(),
    };
    let label = match entry.used {
        true => t!("settings-profile-used-name", name = entry.name.as_ref()),
        false => entry.name.clone(),
    };
    let trailing = match entry.used {
        true => div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .text_size(theme.text(Text::Small))
            .text_color(theme.primary)
            .child(
                svg()
                    .path(icons::path("icons/check.svg"))
                    .size(theme.text(Text::Small))
                    .text_color(theme.primary),
            )
            .child(t!("settings-profile-used"))
            .into_any_element(),
        false => Button::new(SharedString::from(format!("profile-use-{}", entry.id)))
            .label(t!("settings-profile-use"))
            .name(t!("settings-profile-use-name", name = entry.name.as_ref()))
            .small()
            .secondary()
            .on_click(move |_, _, cx| pick(cx))
            .into_any_element(),
    };

    div()
        .id(SharedString::from(format!("profile-{}", entry.id)))
        .role(Role::ListItem)
        .aria_label(label)
        .aria_position_in_set(entry.place + 1)
        .aria_size_of_set(entry.count)
        .flex()
        .items_center()
        .gap_3()
        .px_2()
        .py_1p5()
        .rounded(theme.radius)
        .corner_smoothing(theme.smoothing)
        .when(entry.used, |row| row.bg(theme.secondary))
        .child(picture)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().truncate().child(entry.name))
                .children(entry.detail.map(|detail| {
                    div()
                        .truncate()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(detail)
                })),
        )
        .child(trailing)
        .children(extra)
        .into_any_element()
}
