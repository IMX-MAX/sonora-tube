use std::time::Duration;

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Global, MouseButton, Pixels,
    Render, ScrollHandle, SharedString, SpringConfig, Task, Window, anchored, deferred, div, point,
    px,
};
use i18n::t;
use input::POWERBAR_CONTEXT;
use router::{Destination, navigate, open_settings};
use state::{Subscriptions, VideoSearch, Watching};
use tube::Found;
use ui::{
    ActiveTheme as _, Card, Cascade, Dismiss, Input, Kbd, SelectNext, SelectPrevious, Spinner,
    Sprung, Submit, Text, Unfold, blurring, corners, effects, eyebrow, popover_fill, snapped,
    spoken_vacant,
};

use crate::shared::trouble;

/// How many results the powerbar lists, taken from the top of the ranked search.
const MAX_HITS: usize = 8;
/// How far down the room under the title bar the panel hangs, as a share of that room.
const PERCH: f32 = 0.14;
/// How wide the panel is, in covers.
const WIDTH: f32 = 4.6;
/// How tall the results may grow, as a share of the room under the title bar, before they scroll.
const REACH: f32 = 0.56;
/// The panel dropping into place. The little bounce makes it land rather than stop.
const ARRIVE: SpringConfig = ui::spring(0.3, 0.12);
/// The panel lifting away, quicker than it came and with no bounce.
const LEAVE: SpringConfig = ui::spring(0.16, 0.);
/// How far above its place the panel starts its drop.
const DROP: Pixels = px(28.);
/// How small the panel starts, as a share of its size.
const ZOOM: f32 = 0.92;
/// How hard the panel blurs as it starts to arrive.
const BLUR: Pixels = px(18.);
/// How hard the window behind the scrim blurs once the panel is open.
const SCRIM_BLUR: Pixels = px(12.);
/// How much of the page colour the scrim lays over the window.
const SCRIM: f32 = 0.42;
/// How hard the frost under the panel blurs what it covers.
const FROST: Pixels = px(24.);
/// Below this a blur is not worth a layer of its own.
const BLUR_LEAST: Pixels = px(0.05);
/// How close the panel's spring has to come to count as landed.
const REST: f32 = 0.001;

/// How many of the videos watched last are offered before anything is typed.
const RECENT: usize = 4;
/// How long typing rests before YouTube is asked, so that not every letter is a request.
const SETTLE: Duration = Duration::from_millis(250);

/// Something done to the video that is playing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Deed {
    Toggle,
    Mute,
    Captions,
    Full,
}

impl Deed {
    const ALL: [Self; 4] = [Self::Toggle, Self::Mute, Self::Captions, Self::Full];

    fn id(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Mute => "mute",
            Self::Captions => "captions",
            Self::Full => "full",
        }
    }

    fn icon(self, watching: &Watching) -> &'static str {
        let paused = watching.player().is_none_or(|player| player.paused());
        match self {
            Self::Toggle if paused => "icons/play-filled.svg",
            Self::Toggle => "icons/pause-filled.svg",
            Self::Mute => "icons/volume-x.svg",
            Self::Captions => "icons/text-select.svg",
            Self::Full => "icons/maximize.svg",
        }
    }

    /// The Fluent key of what it does as things stand.
    fn key(self, watching: &Watching) -> &'static str {
        let paused = watching.player().is_none_or(|player| player.paused());
        match self {
            Self::Toggle if paused => "video-play",
            Self::Toggle => "video-pause",
            Self::Mute if watching.muted() => "video-unmute",
            Self::Mute => "video-mute",
            Self::Captions if watching.captions() => "video-captions-hide",
            Self::Captions => "video-captions-show",
            Self::Full if watching.full() => "video-full-leave",
            Self::Full => "video-full-enter",
        }
    }

    fn run(self, watching: &mut Watching, cx: &mut Context<Watching>) {
        match self {
            Self::Toggle => watching.toggle(cx),
            Self::Mute => watching.toggle_muted(cx),
            Self::Captions => watching.toggle_captions(cx),
            Self::Full => watching.set_full(!watching.full(), cx),
        }
    }
}

/// A page the quick search goes to: its name's Fluent key, its icon and where it is. Settings
/// have no destination of their own.
const PAGES: [(&str, &str, Option<Destination>); 6] = [
    ("nav-home", "icons/house.svg", Some(Destination::Home)),
    (
        "nav-discover",
        "icons/compass.svg",
        Some(Destination::Discover),
    ),
    ("nav-search", "icons/search.svg", Some(Destination::Search)),
    ("nav-later", "icons/clock.svg", Some(Destination::Later)),
    (
        "nav-history",
        "icons/rotate-ccw-clock.svg",
        Some(Destination::History),
    ),
    ("nav-settings", "icons/settings.svg", None),
];

/// One line of the quick search.
#[derive(Clone)]
enum Row {
    /// Something YouTube found, or a video watched lately.
    Found(Found),
    /// A page of the app, by its place in [`PAGES`].
    Page(usize),
    /// Something done to the playing video.
    Deed(Deed),
}

impl Row {
    /// What tells the row from every other, which its entrance and the cursor follow.
    fn key(&self) -> SharedString {
        match self {
            Self::Found(Found::Video(video)) => format!("video-{}", video.id).into(),
            Self::Found(Found::Channel(channel)) => format!("channel-{}", channel.id).into(),
            Self::Found(Found::Playlist(list)) => format!("playlist-{}", list.id).into(),
            Self::Page(place) => format!("page-{place}").into(),
            Self::Deed(deed) => format!("deed-{}", deed.id()).into(),
        }
    }
}

/// A floating quick search over the whole window: the videos, channels and playlists YouTube
/// finds for what is typed, the pages of the app and what can be done to the playing video. It
/// drops in from above with the window blurring away behind it.
pub(crate) struct Powerbar {
    open: bool,
    input: Entity<Input>,
    search: Entity<VideoSearch>,
    watching: Entity<Watching>,
    subscriptions: Entity<Subscriptions>,
    rows: Vec<Row>,
    cursor: Cursor,
    restore: Option<FocusHandle>,
    scroll: ScrollHandle,
    /// How far the panel has arrived, 0 gone and 1 in place. The panel stays drawn while this
    /// springs back to 0 after a close.
    shown: Sprung,
    /// The results' box, stretching to fit as they come and go.
    unfold: Unfold,
    /// The results surfacing one after another as they arrive.
    cascade: Cascade,
    /// The wait before YouTube is asked for what was typed.
    settle: Option<Task<()>>,
}

/// Where the keyboard cursor rests, which is what Enter acts on.
#[derive(Clone, Copy)]
enum Cursor {
    /// On whichever result is first, as the list fills and reorders. Every new query starts here.
    First,
    /// On a result the arrow keys moved to, following it as the list reorders.
    On(usize),
    /// Off the list and in the field, where Enter opens the search page for what is typed.
    Field,
}

/// What the quick search asks of the window around it.
pub(crate) enum PowerbarEvent {
    /// Open the search page with the full results for this query, or as it was left when the
    /// query is empty.
    Search(String),
}

struct Installed(Entity<Powerbar>);
impl Global for Installed {}

impl EventEmitter<PowerbarEvent> for Powerbar {}

impl Powerbar {
    pub fn entity(
        search: Entity<VideoSearch>,
        watching: Entity<Watching>,
        subscriptions: Entity<Subscriptions>,
        cx: &mut App,
    ) -> Entity<Self> {
        if cx.try_global::<Installed>().is_none() {
            let bar = cx.new(|cx| {
                let input = cx.new(|cx| {
                    Input::new("powerbar-placeholder", cx)
                        .name("powerbar-title")
                        .icon("icons/search.svg")
                        .bare()
                        .clearable()
                });
                cx.observe(&input, |this: &mut Powerbar, input, cx| {
                    let query = input.read(cx).text().trim().to_owned();
                    // A caret move or a selection in the field asks nothing new.
                    if this.search.read(cx).query() == query {
                        return;
                    }
                    this.cursor = Cursor::First;
                    this.scroll.set_offset(point(Pixels::ZERO, Pixels::ZERO));
                    this.rebuild(cx);
                    this.settle = Some(cx.spawn(async move |this, cx| {
                        if !query.is_empty() {
                            cx.background_executor().timer(SETTLE).await;
                        }
                        this.update(cx, |this, cx| {
                            this.search
                                .update(cx, |search, cx| search.search(&query, cx));
                        })
                        .ok();
                    }));
                })
                .detach();

                cx.observe(&search, |this: &mut Powerbar, _, cx| this.rebuild(cx))
                    .detach();
                cx.observe(&watching, |this: &mut Powerbar, _, cx| {
                    if this.open {
                        cx.notify();
                    }
                })
                .detach();

                // A move to another page takes the panel away. Focus stays with the new page.
                cx.observe(&router::trail(cx), |this: &mut Powerbar, _, cx| {
                    if this.open {
                        this.open = false;
                        this.restore = None;
                        cx.notify();
                    }
                })
                .detach();

                Self {
                    open: false,
                    input,
                    search,
                    watching,
                    subscriptions,
                    rows: Vec::new(),
                    cursor: Cursor::First,
                    restore: None,
                    scroll: ScrollHandle::new(),
                    shown: Sprung::at(0.),
                    unfold: Unfold::default(),
                    cascade: Cascade::default(),
                    settle: None,
                }
            });
            cx.set_global(Installed(bar));
        }
        cx.global::<Installed>().0.clone()
    }

    pub fn toggle(window: &mut Window, cx: &mut App) {
        let Some(bar) = cx.try_global::<Installed>().map(|i| i.0.clone()) else {
            return;
        };
        bar.update(cx, |this, cx| {
            if this.open {
                this.close(window, cx);
            } else {
                this.show(window, cx);
            }
        });
    }

    /// Opens the panel on an empty query, offering the pages, the playing video's controls and
    /// the videos watched last.
    fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restore = window.focused(cx);
        self.open = true;
        self.cursor = Cursor::First;
        self.cascade.clear();
        self.unfold.reset();
        self.scroll.set_offset(point(Pixels::ZERO, Pixels::ZERO));
        self.input.update(cx, |input, cx| {
            input.set_text("", cx);
            input.focus(window, cx);
        });
        self.settle = None;
        self.search.update(cx, |search, cx| search.search("", cx));
        self.rebuild(cx);
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Some(focus) = self.restore.take() {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// What is typed, trimmed.
    fn query(&self, cx: &App) -> String {
        self.input.read(cx).text().trim().to_owned()
    }

    /// Lists the rows for what is typed: the controls and pages whose names hold it, then what
    /// YouTube found. Nothing typed lists them all, with the videos watched last.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        // A closing panel keeps showing what it had.
        if !self.open {
            return;
        }
        let query = self.query(cx).to_lowercase();
        let named =
            |key: &str| query.is_empty() || i18n::translate(key).to_lowercase().contains(&query);
        let watching = self.watching.read(cx);
        let mut rows: Vec<Row> = Vec::new();
        if watching.id().is_some() {
            rows.extend(
                Deed::ALL
                    .into_iter()
                    .filter(|deed| named(deed.key(watching)))
                    .map(Row::Deed),
            );
        }
        rows.extend(
            PAGES
                .iter()
                .enumerate()
                .filter(|(_, page)| named(page.0))
                .map(|(place, _)| Row::Page(place)),
        );
        match query.is_empty() {
            true => rows.extend(
                self.subscriptions
                    .read(cx)
                    .watched()
                    .iter()
                    .take(RECENT)
                    .map(|video| Row::Found(Found::Video(video.clone()))),
            ),
            false => {
                // Typing narrows the app's own rows to a few, so what YouTube found comes first.
                let found = self.search.read(cx).found();
                let mut all: Vec<Row> = found
                    .iter()
                    .take(MAX_HITS)
                    .cloned()
                    .map(Row::Found)
                    .collect();
                rows.append(&mut all);
            }
        }

        // A row the arrow keys moved to keeps the cursor as a new answer reorders the list.
        if let Cursor::On(idx) = self.cursor {
            let picked = self.rows.get(idx).map(Row::key);
            let found = picked.and_then(|key| rows.iter().position(|row| row.key() == key));
            let kept = found.or_else(|| rows.len().checked_sub(1).map(|last| idx.min(last)));
            self.cursor = kept.map_or(Cursor::First, Cursor::On);
            if let Some(idx) = kept {
                self.scroll.scroll_to_item(idx);
            }
        }
        self.rows = rows;
        cx.notify();
    }

    /// The row under the keyboard cursor, if there is one.
    fn selected(&self) -> Option<usize> {
        match self.cursor {
            Cursor::First => (!self.rows.is_empty()).then_some(0),
            Cursor::On(idx) => Some(idx),
            Cursor::Field => None,
        }
    }

    /// Moves the keyboard cursor to the result at `idx` and scrolls it into view. The keyboard
    /// itself stays in the field, so typing goes on refining the query.
    fn choose(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.cursor = Cursor::On(idx);
        self.scroll.scroll_to_item(idx);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _window: &mut Window, cx: &mut Context<Self>) {
        let total = self.rows.len();
        if total == 0 {
            return;
        }
        let idx = match self.selected() {
            None => 0,
            Some(i) => (i + 1).min(total - 1),
        };
        self.choose(idx, cx);
    }

    fn select_previous(
        &mut self,
        _: &SelectPrevious,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self.selected() else {
            return;
        };
        // Up from the first result lets go of the list, the keyboard staying in the field.
        match current.checked_sub(1) {
            Some(idx) => self.choose(idx, cx),
            None => {
                self.cursor = Cursor::Field;
                cx.notify();
            }
        }
    }

    /// Enter: does what the chosen row is for. With no row chosen it opens the search page
    /// for what is typed.
    fn activate(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected().and_then(|idx| self.rows.get(idx)).cloned() {
            Some(row) => self.enter(&row, cx),
            None => cx.emit(PowerbarEvent::Search(self.query(cx))),
        }
        self.close(window, cx);
    }

    /// Opens what a row stands for, or does it.
    fn enter(&self, row: &Row, cx: &mut App) {
        match row {
            Row::Found(Found::Video(video)) => {
                navigate(Destination::Watch(video.id.clone().into()), cx)
            }
            Row::Found(Found::Channel(channel)) => {
                navigate(Destination::Channel(channel.id.clone().into()), cx)
            }
            Row::Found(Found::Playlist(list)) => {
                navigate(Destination::Videos(list.id.clone().into()), cx)
            }
            Row::Page(place) => match PAGES[*place].2.clone() {
                Some(destination) => navigate(destination, cx),
                None => open_settings(cx),
            },
            Row::Deed(deed) => {
                let deed = *deed;
                self.watching
                    .update(cx, |watching, cx| deed.run(watching, cx));
            }
        }
    }

    /// The card of one row, marked when the keyboard cursor rests on it.
    fn card(&self, row: &Row, place: usize, cx: &App) -> Card {
        let line = |parts: [Option<String>; 2]| {
            SharedString::from(parts.into_iter().flatten().collect::<Vec<_>>().join(" · "))
        };
        match row {
            Row::Found(Found::Video(video)) => {
                Card::new(("powerbar-video", place), video.title.clone())
                    .cover(Some(video.thumbnail()))
                    .fallback("icons/play-filled.svg")
                    .meta(Self::under(
                        line([
                            video.channel.as_ref().map(|channel| channel.name.clone()),
                            video.length.clone(),
                        ]),
                        cx,
                    ))
            }
            Row::Found(Found::Channel(channel)) => {
                Card::new(("powerbar-channel", place), channel.name.clone())
                    .cover(channel.avatar.clone())
                    .fallback("icons/user.svg")
                    .circle()
                    .meta(Self::under(
                        line([
                            Some(t!("video-channel").to_string()),
                            channel.subscribers.clone(),
                        ]),
                        cx,
                    ))
            }
            Row::Found(Found::Playlist(list)) => {
                Card::new(("powerbar-playlist", place), list.title.clone())
                    .cover(list.thumbnail.clone())
                    .fallback("icons/list.svg")
                    .meta(Self::under(
                        line([Some(t!("video-playlist").to_string()), list.detail.clone()]),
                        cx,
                    ))
            }
            Row::Page(page) => {
                let (key, icon, _) = &PAGES[*page];
                Card::new(("powerbar-page", place), i18n::translate(key))
                    .cover(None)
                    .fallback(*icon)
            }
            Row::Deed(deed) => {
                let watching = self.watching.read(cx);
                Card::new(
                    ("powerbar-deed", place),
                    i18n::translate(deed.key(watching)),
                )
                .cover(None)
                .fallback(deed.icon(watching))
                .meta(Self::under(
                    match watching.watch() {
                        Some(watch) => SharedString::from(watch.title.clone()),
                        None => SharedString::default(),
                    },
                    cx,
                ))
            }
        }
        .tucked()
    }

    /// The line under a row's name.
    fn under(line: SharedString, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        div()
            .truncate()
            .text_size(theme.text(Text::Small))
            .text_color(theme.muted_foreground)
            .child(line)
    }

    /// One card per row, each wrapped in its entrance.
    fn results(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let total = self.rows.len();
        self.rows
            .iter()
            .enumerate()
            .map(|(place, row)| {
                let shown = self.cascade.level(&row.key(), window, cx);
                let chosen = self.selected() == Some(place);
                let (pressed, me) = (row.clone(), cx.entity());
                let card = self
                    .card(row, place, cx)
                    .chosen(chosen)
                    .aria_position_in_set(place + 1)
                    .aria_size_of_set(total)
                    .when(chosen, |card| {
                        card.aria_selected(true).aria_active_descendant()
                    })
                    .press(move |_, window, cx| {
                        me.update(cx, |this, cx| {
                            this.enter(&pressed, cx);
                            this.close(window, cx);
                        });
                    });
                Cascade::surface(card, shown).into_any_element()
            })
            .collect()
    }

    /// Whether nothing is typed, so the panel offers rather than answers.
    fn recalling(&self, cx: &App) -> bool {
        self.query(cx).is_empty()
    }

    /// What shows under the field while there is a query and nothing to list for it: a spinner
    /// while the answer is on its way, the reason with a retry when the search failed, and "no
    /// matches" once it came back empty.
    fn quiet(&self, cx: &App) -> Option<AnyElement> {
        let query = self.query(cx);
        if query.is_empty() || !self.rows.is_empty() {
            return None;
        }
        let search = self.search.read(cx);
        if search.is_loading() || search.query() != query {
            return Some(
                div()
                    .flex()
                    .justify_center()
                    .p(cx.theme().metrics.pad * 2.)
                    .child(Spinner::new("powerbar-loading"))
                    .into_any_element(),
            );
        }
        if let Some(reason) = search.error() {
            let again = self.search.clone();
            return Some(
                trouble::lost(
                    "powerbar-lost",
                    t!("trouble-not-loaded"),
                    Some(reason),
                    move |_, _, cx| again.update(cx, |search, cx| search.search(&query, cx)),
                    cx,
                )
                .into_any_element(),
            );
        }
        Some(spoken_vacant("powerbar-empty", t!("search-no-matches"), cx).into_any_element())
    }

    /// The row of key hints along the foot of the panel. The Enter hint says it searches when
    /// the cursor is in the field.
    fn hints(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let enter = match self.selected() {
            Some(_) => t!("powerbar-open"),
            None => t!("search-submit"),
        };
        let hint = |id: &'static str, keystroke: &'static str, label: SharedString| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_1p5()
                .child(Kbd::new(keystroke).spoken(id))
                .child(label)
        };

        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_4()
            .gap_y_1()
            .px(theme.metrics.pad * 2.)
            .py(theme.metrics.pad)
            .border_t_1()
            .border_color(theme.border)
            .text_size(theme.text(Text::Small))
            .text_color(theme.muted_foreground)
            .child(hint("powerbar-hint-enter", "enter", enter))
            .child(hint("powerbar-hint-close", "escape", t!("common-dismiss")))
    }
}

impl Render for Powerbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (target, config) = match self.open {
            true => (1., ARRIVE),
            false => (0., LEAVE),
        };
        let shown = self.shown.follow(target, config, REST, window, cx);
        if !self.open && self.shown.resting() {
            return div().into_any_element();
        }

        let theme = *cx.theme();
        let frosted = blurring(cx) && effects();
        let fade = shown.clamp(0., 1.);
        let gone = 1. - fade;

        let chrome = snapped(theme.metrics.title_bar, window);
        let viewport = window.viewport_size();
        let room = viewport.height - chrome;
        let width = (theme.metrics.cover * WIDTH).min(viewport.width - theme.metrics.inset * 2.);
        let radius = corners(theme.radius);
        let reach = room * REACH;

        self.cascade.show(self.rows.iter().map(Row::key));
        let results = self.results(window, cx);
        let quiet = self.quiet(cx);
        let empty = results.is_empty() && quiet.is_none();
        let scroll = self.scroll.clone();

        let body = div()
            .flex()
            .flex_col()
            .w_full()
            .when(self.recalling(cx) && !self.rows.is_empty(), |this| {
                let label = t!("powerbar-suggested");
                this.child(
                    eyebrow(label.clone(), cx)
                        .id("powerbar-recent-heading")
                        .role(Role::Heading)
                        .aria_level(2)
                        .aria_label(label)
                        .px(theme.metrics.pad * 2.)
                        .pt(theme.metrics.pad * 1.5),
                )
            })
            .map(|this| match quiet {
                Some(quiet) => this.child(quiet),
                None => this.child(
                    div()
                        .id("powerbar-results")
                        .role(Role::ListBox)
                        .aria_label(match self.recalling(cx) {
                            true => t!("powerbar-suggested"),
                            false => t!("search-results"),
                        })
                        .flex()
                        .flex_col()
                        .w_full()
                        .max_h(reach)
                        .p(theme.metrics.pad)
                        .overflow_y_scroll()
                        .track_scroll(&scroll)
                        .children(results),
                ),
            })
            // The key hints only make sense with something to act on, so they arrive with the
            // first results and ride the same growing height.
            .when(!self.rows.is_empty(), |this| this.child(self.hints(cx)));
        let body = self.unfold.wrap(body, empty, window, cx);
        let height = self.unfold.height();

        let panel = div()
            .id("powerbar-panel")
            .role(Role::Dialog)
            .aria_label(t!("powerbar-title"))
            .relative()
            .occlude()
            .w_full()
            .flex()
            .flex_col()
            .rounded(radius)
            .border_1()
            .border_color(theme.border)
            .bg(popover_fill(cx))
            .text_color(theme.popover_foreground)
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .px(theme.metrics.pad)
                    .text_size(theme.text(Text::Large))
                    .child(self.input.clone()),
            )
            .child(body.when(height > Pixels::ZERO, |this| {
                this.border_t_1().border_color(theme.border)
            }))
            // The arrival is a paint filter, so the frost cannot ride it and fades in on its own
            // underneath instead.
            .when(shown != 1., |this| {
                this.opacity(fade)
                    .layer_translate(point(px(0.), -DROP * (1. - shown)))
                    .layer_scale(1. - (1. - ZOOM) * (1. - shown))
                    .layer_scale_origin(point(0.5, 0.))
            })
            .map(|this| match BLUR * gone {
                blur if blur > BLUR_LEAST && effects() => this.blur(blur),
                _ => this,
            });

        let scrim = div()
            .id("powerbar-scrim")
            .absolute()
            .inset_0()
            .bg(theme.background.opacity(SCRIM * fade))
            .when(frosted && SCRIM_BLUR * fade > BLUR_LEAST, |this| {
                this.backdrop_blur(SCRIM_BLUR * fade)
            })
            .when(self.open, |this| {
                this.occlude()
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            cx.stop_propagation();
                            this.close(window, cx);
                        }),
                    )
            });

        let column = div()
            .absolute()
            .top(chrome + room * PERCH)
            .left((viewport.width - width) / 2.)
            .w(width)
            .when(frosted, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(radius)
                        .backdrop_blur(FROST)
                        .opacity(fade * fade),
                )
            })
            .child(panel);

        let frame = div()
            .relative()
            .w(viewport.width)
            .h(viewport.height)
            .when(self.open, |this| {
                this.key_context(POWERBAR_CONTEXT)
                    .on_action(cx.listener(Self::select_next))
                    .on_action(cx.listener(Self::select_previous))
                    .on_action(cx.listener(Self::activate))
                    .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
                        cx.stop_propagation();
                        this.close(window, cx);
                    }))
            })
            .child(scrim)
            .child(column);

        deferred(
            anchored()
                .position(point(Pixels::ZERO, Pixels::ZERO))
                .child(frame),
        )
        .into_any_element()
    }
}
