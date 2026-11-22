use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{AnyElement, App, Context, MouseUpEvent, div};
use i18n::t;
use ui::{ActiveTheme as _, Scrubber, Switch};

use super::{Group, SettingsView, Slot};

/// How wide the sleep slider and the time beside it stand, in covers.
const SLEEP_WIDTH: f32 = 1.;
const SLEEP_LABEL: f32 = 0.7;
impl SettingsView {
    pub(super) fn playback_groups(&self, _: &App) -> Vec<Group> {
        vec![Group::new("settings-group-videos", "icons/play.svg").rows([
            Slot::HideShorts,
            Slot::Captions,
            Slot::Ambient,
        ])]
    }

    /// The switch that keeps played tracks on disk. Soloist streams every track and keeps its
    /// own cache, so the switch is out of use while it plays.
    pub(super) fn keep_tracks_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let on = self.playback.read(cx).keep_tracks();
        let unavailable = self.soloist_plays(cx);

        self.soloist_row_for(
            t!("settings-keep-tracks"),
            Switch::new("keep-tracks", on)
                .name(t!("settings-keep-tracks"))
                .disabled(unavailable)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.playback
                        .update(cx, |playback, cx| playback.set_keep_tracks(!on, cx));
                }))
                .into_any_element(),
            unavailable,
        )
    }

    /// How many gigabytes the cache of played tracks may take, as a slider with the size before
    /// it. The label follows a drag at once, and the limit changes when the drag lets go.
    pub(super) fn cache_size_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let (low, high) = state::CACHE_GB_RANGE;
        let current = self
            .pending_cache
            .unwrap_or_else(|| self.playback.read(cx).cache_gb());
        let fraction = (current.saturating_sub(low)) as f32 / (high - low) as f32;

        let control = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                ui::spoken("settings-cache-size-value", t!("unit-gb", value = current))
                    .flex_none()
                    .w(theme.metrics.cover * SLEEP_LABEL)
                    .whitespace_nowrap()
                    .text_right(),
            )
            .child(
                div()
                    .id("settings-cache-slider")
                    .role(Role::Group)
                    .aria_label(t!("settings-cache-size"))
                    .w(theme.metrics.cover * SLEEP_WIDTH)
                    .child(
                        Scrubber::new(&self.cache, fraction)
                            .name(t!("settings-cache-size"))
                            .value(t!("unit-gb", value = current))
                            .colors(
                                theme.progress_bar,
                                theme.muted_foreground.opacity(0.3),
                                theme.foreground,
                            )
                            .on_move(cx.listener(move |this, fraction: &f32, _, cx| {
                                let span = (high - low) as f32;
                                this.pending_cache =
                                    Some(low + (fraction.clamp(0., 1.) * span).round() as u32);
                                cx.notify();
                            }))
                            .on_release(cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                let Some(gb) = this.pending_cache.take() else {
                                    return;
                                };
                                this.playback
                                    .update(cx, |playback, cx| playback.set_cache_gb(gb, cx));
                            })),
                    ),
            );

        self.row(t!("settings-cache-size"), control.into_any_element())
    }
}

impl SettingsView {}
