//! The video screens: the search results, the page a video plays on and the page of a channel or
//! a playlist. All three lay videos out as the same thumbnail tiles.

use std::time::{Duration, Instant};

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnyElement, AnyView, AnyWindowHandle, App, Bounds, ClipboardItem, Context, Entity, FontWeight,
    MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollHandle, SharedString,
    Size, Task, Window, WindowBounds, WindowControlArea, WindowKind, WindowOptions, anchored,
    deferred, div, point, px, size,
};
use i18n::t;
use router::{Destination, navigate};
use state::{Outcome, Subscriptions, Toasts, VideoList, VideoSearch, Watching};
use tube::{Channel, Comment, Found, Playlist, Video};
use ui::{
    ActiveTheme as _, Artwork, Avatar, Button, Input, Room, Scrollbar, Scroller, Scrubber,
    ScrubberState, Spinner, Submit, Text, clock, heading, vacant,
};

use crate::chrome::Chrome;
use crate::shared::cells;
use crate::shared::hero::{HeroMetaStrip, PageHero};
use crate::shared::transport::{percent, volume_icon};
use crate::shared::trouble;

const CHANNEL_ICON: &str = "icons/user.svg";
const PLAYLIST_ICON: &str = "icons/list.svg";
/// The narrowest a video tile gets before the grid drops a column.
const TILE_MIN: Pixels = px(220.);
/// How wide the picture and the text under it may grow on the watch page.
const STAGE_MAX: Pixels = px(1120.);
/// How wide the whole watch page may grow once the videos to watch next sit beside the picture.
const WATCH_MAX: Pixels = px(1680.);
/// How wide the column of videos to watch next is when it sits beside the picture.
const NEXT_WIDTH: Pixels = px(400.);
/// How wide a thumbnail is in that column.
/// The widest a thumbnail gets in a row of search results, and the share of the row it may take.
const ROW_PICTURE: Pixels = px(360.);
const ROW_SHARE: f32 = 0.42;
/// The widest the list of search results gets, so a row's text stays next to its picture.
/// How wide the search field grows before it stops and sits centred above the results.
const FIELD_MAX: Pixels = px(640.);
const ROWS_MAX: Pixels = px(1100.);
/// How tall the transcript panel is when it sits under the video rather than beside it.
/// The widest the home page, the history and the saved videos lay their tiles out.
const FEED_MAX: Pixels = px(1500.);
/// A followed channel's avatar on the home page, in controls, and its cell in avatars.
const FACE: f32 = 1.8;
const FACE_ROOM: f32 = 1.7;
const SCRIPT_HEIGHT: Pixels = px(420.);
/// How much of the window's height the picture may take in theater mode.
const THEATER_SHARE: f32 = 0.78;
/// How long the controls stay over a full screen picture after the pointer last moved.
const LINGER: Duration = Duration::from_secs(3);
/// How see-through the bar under a full screen picture is.
const VEIL: f32 = 0.85;
/// How large the picture-in-picture window opens, and how far from the screen's corner.
const PIP_SIZE: Size<Pixels> = size(px(480.), px(270.));
const PIP_MARGIN: Pixels = px(24.);
/// How many lines of a description show before it is opened.
const OPENING: usize = 3;
const STEADY: Pixels = px(0.5);
/// How often the watch page redraws its clock and scrubber while a video plays.
const TICK: Duration = Duration::from_millis(250);
/// How far the skip buttons move the video, in seconds.
const SKIP: f64 = 10.;
/// How large a channel's avatar is among search results, against a control's height.
const CHIP_AVATAR: f32 = 1.5;
/// How wide the volume slider under the video is.
const VOLUME_WIDTH: Pixels = px(110.);

/// The width of one tile and how many fit across `available`.
#[derive(Clone, Copy)]
struct Tiles {
    width: Pixels,
    gap: Pixels,
    /// How many tiles one row holds.
    columns: usize,
}

impl Tiles {
    fn new(available: Pixels, cx: &App) -> Self {
        let gap = cx.theme().metrics.pad;
        let available = available.max(TILE_MIN);
        let columns = ((available + gap) / (TILE_MIN + gap)).floor().max(1.);
        Self {
            width: ((available - gap * (columns - 1.)) / columns).floor(),
            gap,
            columns: columns as usize,
        }
    }

    /// Lays `tiles` out in wrapping rows.
    fn grid(self, tiles: Vec<AnyElement>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .w_full()
            .gap_x(self.gap)
            .gap_y(self.gap * 1.5)
            .children(tiles)
    }
}

/// The dimmed line under a tile's title, its parts joined by a dot.
fn caption(parts: impl IntoIterator<Item = Option<String>>) -> SharedString {
    parts
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
        .into()
}

/// What a tile shows: a picture with a `badge` in its corner, a two-line title and up to two
/// dimmed `lines`.
struct Card<'a> {
    picture: Artwork,
    badge: Option<SharedString>,
    title: &'a str,
    lines: [SharedString; 2],
    /// What sits over the picture's top corner, shown while the tile is pointed at.
    corner: Option<AnyElement>,
}

/// A tile showing `card`, opening `destination` when pressed.
fn tile(
    id: SharedString,
    card: Card<'_>,
    place: (usize, usize),
    destination: Destination,
    tiles: Tiles,
    cx: &App,
) -> AnyElement {
    let Card {
        picture,
        badge,
        title,
        lines,
        corner,
    } = card;
    let theme = cx.theme();
    let title = SharedString::from(title.to_owned());
    let caption = caption(lines.iter().map(|line| Some(line.to_string())));
    let radius = theme.radius;

    div()
        .id(id.clone())
        .group(id)
        .role(Role::Link)
        .aria_label(title.clone())
        .aria_description(caption)
        .aria_position_in_set(place.0 + 1)
        .aria_size_of_set(place.1)
        .flex()
        .flex_col()
        .flex_none()
        .w(tiles.width)
        .gap_2()
        .cursor_pointer()
        .on_click(move |_, _, cx| navigate(destination.clone(), cx))
        .child(
            div()
                .relative()
                .child(picture)
                .when_some(badge, |this, badge| {
                    this.child(
                        div()
                            .absolute()
                            .right_1p5()
                            .bottom_1p5()
                            .px_1p5()
                            .py_0p5()
                            .rounded(radius)
                            .corner_smoothing(theme.smoothing)
                            .bg(theme.overlay)
                            .text_size(theme.text(Text::Small))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.overlay_foreground)
                            .child(badge),
                    )
                })
                .children(corner),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .line_clamp(2)
                        .text_size(theme.text(Text::Label))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(title),
                )
                .children(
                    lines
                        .into_iter()
                        .filter(|line| !line.is_empty())
                        .map(|line| {
                            div()
                                .truncate()
                                .text_size(theme.text(Text::Small))
                                .text_color(theme.muted_foreground)
                                .child(line)
                        }),
                ),
        )
        .into_any_element()
}

/// The tile of one video: its thumbnail with the length in the corner, then who made it and how
/// it has done.
fn video_tile(
    scope: &str,
    video: &Video,
    place: (usize, usize),
    tiles: Tiles,
    kept: &Entity<Subscriptions>,
    cx: &App,
) -> AnyElement {
    let picture = Artwork::new(Some(video.thumbnail()))
        .size(tiles.width)
        .wide((tiles.width * 9. / 16.).floor())
        .corner_radius(cx.theme().radius);
    let channel = video.channel.as_ref().map(|channel| channel.name.clone());
    let lines = [
        channel.unwrap_or_default().into(),
        caption([video.views.clone(), video.published.clone()]),
    ];
    let id = SharedString::from(format!("{scope}-video-{}", video.id));
    tile(
        id,
        Card {
            picture,
            badge: video.length.clone().map(Into::into),
            title: &video.title,
            lines,
            corner: Some(later(scope, video, kept, cx)),
        },
        place,
        Destination::Watch(video.id.clone().into()),
        tiles,
        cx,
    )
}

/// The button over a thumbnail's corner that saves the video to watch later or takes it back out.
/// It shows while its tile, the group named after the tile's id, is pointed at, and stays once saved.
fn later(scope: &str, video: &Video, kept: &Entity<Subscriptions>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let saved = kept.read(cx).saved(&video.id);
    let group = SharedString::from(format!("{scope}-video-{}", video.id));
    let (kept, video) = (kept.clone(), video.clone());

    div()
        .absolute()
        .top_1p5()
        .right_1p5()
        .rounded(theme.radius)
        .corner_smoothing(theme.smoothing)
        .bg(theme.overlay)
        .when(!saved, |this| {
            this.invisible().group_hover(group, |this| this.visible())
        })
        .child(
            Button::new(SharedString::from(format!("{scope}-later-{}", video.id)))
                .icon(match saved {
                    true => "icons/check.svg",
                    false => "icons/clock.svg",
                })
                .ghost()
                .small()
                .tint(theme.overlay_foreground)
                .tooltip(match saved {
                    true => "video-later-remove",
                    false => "video-later-add",
                })
                .on_click(move |_, _, cx| {
                    kept.update(cx, |kept, cx| kept.toggle_later(&video, cx));
                }),
        )
        .into_any_element()
}

/// The length of a video over the corner of its thumbnail.
fn length_badge(length: SharedString, cx: &App) -> impl IntoElement {
    let theme = cx.theme();

    div()
        .absolute()
        .right_1p5()
        .bottom_1p5()
        .px_1p5()
        .py_0p5()
        .rounded(theme.radius)
        .corner_smoothing(theme.smoothing)
        .bg(theme.overlay)
        .text_size(theme.text(Text::Small))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.overlay_foreground)
        .child(length)
}

/// One video as a row: its thumbnail `picture` wide on the left and what is known about it on the
/// right. A `roomy` row is a search result, with the channel's avatar and a line of the
/// description; the other is the short one beside a playing video.
fn video_row(
    scope: &str,
    video: &Video,
    place: (usize, usize),
    picture: Pixels,
    roomy: bool,
    kept: &Entity<Subscriptions>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let title = SharedString::from(video.title.clone());
    let channel = video.channel.as_ref().map(|channel| channel.name.clone());
    let numbers = caption([video.views.clone(), video.published.clone()]);
    let spoken = caption([channel.clone(), Some(numbers.to_string())]);
    let destination = Destination::Watch(video.id.clone().into());
    let dim = |line: SharedString| {
        div()
            .truncate()
            .text_size(theme.text(Text::Small))
            .text_color(theme.muted_foreground)
            .child(line)
    };
    let channel = channel.map(SharedString::from);

    let id = SharedString::from(format!("{scope}-video-{}", video.id));
    div()
        .id(id.clone())
        .group(id)
        .role(Role::Link)
        .aria_label(title.clone())
        .aria_description(spoken)
        .aria_position_in_set(place.0 + 1)
        .aria_size_of_set(place.1)
        .flex()
        .w_full()
        .rounded(theme.radius)
        .corner_smoothing(theme.smoothing)
        .cursor_pointer()
        .hover(|this| this.bg(theme.secondary))
        .on_click(move |_, _, cx| navigate(destination.clone(), cx))
        .map(|row| match roomy {
            true => row.gap_4().p_2(),
            false => row.gap_2().p_1(),
        })
        .child(
            div()
                .relative()
                .flex_none()
                .child(
                    Artwork::new(Some(video.thumbnail()))
                        .size(picture)
                        .wide((picture * 9. / 16.).floor())
                        .corner_radius(cx.theme().radius),
                )
                .when_some(video.length.clone(), |this, length| {
                    this.child(length_badge(length.into(), cx))
                })
                .child(later(scope, video, kept, cx)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .map(|column| match roomy {
                    true => column.gap_2().py_1(),
                    false => column.gap_0p5(),
                })
                .child(
                    div()
                        .line_clamp(2)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .text_size(theme.text(match roomy {
                            true => Text::Body,
                            false => Text::Label,
                        }))
                        .child(title),
                )
                .map(|column| match roomy {
                    true => column
                        .when(!numbers.is_empty(), |this| this.child(dim(numbers.clone())))
                        .when_some(channel.clone(), |this, name| {
                            this.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Avatar::new(video.avatar.clone())
                                            .size(theme.metrics.control_small),
                                    )
                                    .child(dim(name)),
                            )
                        })
                        .when_some(video.snippet.clone(), |this, snippet| {
                            this.child(
                                div()
                                    .line_clamp(2)
                                    .text_size(theme.text(Text::Small))
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(snippet)),
                            )
                        }),
                    false => column
                        .when_some(channel.clone(), |this, name| this.child(dim(name)))
                        .when(!numbers.is_empty(), |this| this.child(dim(numbers.clone()))),
                }),
        )
        .into_any_element()
}

fn playlist_tile(
    scope: &str,
    playlist: &Playlist,
    place: (usize, usize),
    tiles: Tiles,
    cx: &App,
) -> AnyElement {
    let picture = Artwork::new(playlist.thumbnail.clone())
        .size(tiles.width)
        .wide((tiles.width * 9. / 16.).floor())
        .corner_radius(cx.theme().radius)
        .fallback(PLAYLIST_ICON);
    tile(
        format!("{scope}-playlist-{}", playlist.id).into(),
        Card {
            picture,
            badge: Some(t!("video-playlist")),
            title: &playlist.title,
            lines: [
                playlist.detail.clone().unwrap_or_default().into(),
                SharedString::default(),
            ],
            corner: None,
        },
        place,
        Destination::Videos(playlist.id.clone().into()),
        tiles,
        cx,
    )
}

/// A channel among search results: its avatar, name and following on one line, with the button to
/// subscribe, opening the channel when pressed.
fn channel_chip(
    channel: &Channel,
    place: (usize, usize),
    subscriptions: &Entity<Subscriptions>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let name = SharedString::from(channel.name.clone());
    let destination = Destination::Channel(channel.id.clone().into());
    let subscribed = subscriptions.read(cx).follows(&channel.id);
    let (kept, followed) = (subscriptions.clone(), channel.clone());

    div()
        .id(SharedString::from(format!("search-channel-{}", channel.id)))
        .role(Role::Link)
        .aria_label(name.clone())
        .aria_position_in_set(place.0 + 1)
        .aria_size_of_set(place.1)
        .flex()
        .flex_none()
        .items_center()
        .gap_3()
        .p_2()
        .pr_3()
        .rounded(theme.radius)
        .corner_smoothing(theme.smoothing)
        .bg(theme.secondary)
        .hover(|this| this.bg(theme.secondary_hover))
        .cursor_pointer()
        .on_click(move |_, _, cx| navigate(destination.clone(), cx))
        .child(Avatar::new(channel.avatar.clone()).size(theme.metrics.control * CHIP_AVATAR))
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(theme.text(Text::Label))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(name),
                )
                .when_some(channel.subscribers.clone(), |this, subscribers| {
                    this.child(
                        div()
                            .text_size(theme.text(Text::Small))
                            .text_color(theme.muted_foreground)
                            .child(subscribers),
                    )
                }),
        )
        .child(
            Button::new(SharedString::from(format!(
                "search-subscribe-{}",
                channel.id
            )))
            .label(match subscribed {
                true => t!("video-subscribed"),
                false => t!("video-subscribe"),
            })
            .map(|button| match subscribed {
                true => button.ghost(),
                false => button,
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                kept.update(cx, |kept, cx| kept.toggle(&followed, cx));
            }),
        )
        .into_any_element()
}

/// The spinner a page shows while its first or next stretch is on its way.
fn waiting(id: &'static str) -> impl IntoElement {
    div()
        .flex()
        .w_full()
        .justify_center()
        .py_4()
        .child(Spinner::new(id))
}

/// The button under a list that fetches its next stretch.
fn more(id: &'static str, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
    div().flex().w_full().justify_center().child(
        Button::new(id)
            .label(t!("video-more"))
            .secondary()
            .on_click(move |_, _, cx| on_click(cx)),
    )
}

/// The button that subscribes to a channel, or drops the subscription it already has.
fn subscribe(
    id: &'static str,
    channel: &Channel,
    subscriptions: &Entity<Subscriptions>,
    cx: &App,
) -> impl IntoElement + use<> {
    let subscribed = subscriptions.read(cx).follows(&channel.id);
    let (channel, subscriptions) = (channel.clone(), subscriptions.clone());
    Button::new(id)
        .label(match subscribed {
            true => t!("video-subscribed"),
            false => t!("video-subscribe"),
        })
        .map(|button| match subscribed {
            true => button.secondary(),
            false => button,
        })
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            subscriptions.update(cx, |kept, cx| kept.toggle(&channel, cx));
        })
}

/// The results of a video search.
pub(crate) struct VideoSearchView {
    search: Entity<VideoSearch>,
    subscriptions: Entity<Subscriptions>,
    input: Entity<Input>,
    scrollbar: Entity<Scrollbar>,
    width: Pixels,
}

impl VideoSearchView {
    pub(crate) fn new(
        search: Entity<VideoSearch>,
        subscriptions: Entity<Subscriptions>,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = cx.entity_id();
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        cx.observe(&subscriptions, |_, _, cx| cx.notify()).detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();

        let input = cx.new(|cx| {
            Input::new("video-search-placeholder", cx)
                .icon("icons/search.svg")
                .clearable()
        });

        Self {
            search,
            subscriptions,
            input,
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(id)),
            width: Pixels::ZERO,
        }
    }

    /// Puts the caret in the search field.
    pub(crate) fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Searches for `query` and shows it in the field.
    pub(crate) fn search_for(&mut self, query: &str, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_text(query.to_owned(), cx));
        self.search
            .update(cx, |search, cx| search.search(query, cx));
    }

    /// Enter in the field: a search for what it holds.
    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        let query = self.input.read(cx).text().to_owned();
        self.search
            .update(cx, |search, cx| search.search(&query, cx));
    }

    fn results(&self, cx: &Context<Self>) -> AnyElement {
        let search = self.search.read(cx);
        let found = search.found();
        let tiles = Tiles::new(self.width, cx);

        if search.query().is_empty() {
            let label = t!("video-search-prompt");
            return div()
                .id("video-search-prompt")
                .role(Role::Status)
                .aria_label(label.clone())
                .child(vacant(label, cx))
                .into_any_element();
        }
        if found.is_empty() && !search.is_loading() {
            let label = t!("search-no-matches");
            return div()
                .id("video-search-empty")
                .role(Role::Status)
                .aria_label(label.clone())
                .child(vacant(label, cx))
                .into_any_element();
        }

        // Channels go in a row of their own above the videos, rather than as odd round tiles
        // scattered among them.
        let channels: Vec<&Channel> = found
            .iter()
            .filter_map(|found| match found {
                Found::Channel(channel) => Some(channel),
                _ => None,
            })
            .collect();
        let count = channels.len();
        let chips = channels
            .into_iter()
            .enumerate()
            .map(|(place, channel)| channel_chip(channel, (place, count), &self.subscriptions, cx));
        let rows = self.width.min(ROWS_MAX);
        let picture = (rows * ROW_SHARE).min(ROW_PICTURE).floor();
        let videos: Vec<&Video> = found
            .iter()
            .filter_map(|found| match found {
                Found::Video(video) => Some(video),
                _ => None,
            })
            .collect();
        let listed = videos.len();
        let videos = videos.into_iter().enumerate().map(|(place, video)| {
            video_row(
                "search",
                video,
                (place, listed),
                picture,
                true,
                &self.subscriptions,
                cx,
            )
        });
        let playlists: Vec<&Playlist> = found
            .iter()
            .filter_map(|found| match found {
                Found::Playlist(playlist) => Some(playlist),
                _ => None,
            })
            .collect();
        let held = playlists.len();
        let playlists: Vec<AnyElement> = playlists
            .into_iter()
            .enumerate()
            .map(|(place, playlist)| playlist_tile("search", playlist, (place, held), tiles, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_6()
            .when(count > 0, |this| {
                this.child(div().flex().flex_wrap().gap(tiles.gap).children(chips))
            })
            .child(div().flex().flex_col().w(rows).gap_1().children(videos))
            .when(held > 0, |this| this.child(tiles.grid(playlists)))
            .into_any_element()
    }
}

impl Render for VideoSearchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let search = self.search.read(cx);
        if let Some(reason) = search.error().map(str::to_owned)
            && search.found().is_empty()
        {
            let search = self.search.clone();
            return div()
                .id("video-search-failure")
                .role(Role::Alert)
                .flex()
                .flex_col()
                .size_full()
                .child(
                    trouble::lost(
                        "video-search-lost",
                        t!("trouble-not-loaded"),
                        Some(&reason),
                        move |_, _, cx| search.update(cx, |search, cx| search.refresh(cx)),
                        cx,
                    )
                    .size_full(),
                )
                .into_any_element();
        }

        let pad = cx.theme().metrics.inset;
        let room = cells::content_width(window, pad * 2., cx);
        if (room - self.width).abs() >= STEADY {
            self.width = room;
        }
        let search = self.search.read(cx);
        let (loading, has_more) = (search.is_loading(), search.has_more());
        let title: SharedString = match search.query() {
            "" => t!("search-submit"),
            query => t!("video-search-results", query = query),
        };
        let next = self.search.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .key_context(::input::SEARCH_CONTEXT)
            .on_action(cx.listener(Self::submit))
            .child(
                Scroller::new("video-search-page", &self.scrollbar)
                    .hold()
                    .bottom_inset(Chrome::bottom(cx))
                    .py(pad)
                    .px(pad + Chrome::margin(window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_6()
                            .child(
                                div().flex().justify_center().child(
                                    div()
                                        .id("video-search-field")
                                        .role(Role::Search)
                                        .aria_label(title)
                                        .w_full()
                                        .max_w(FIELD_MAX)
                                        .child(self.input.clone()),
                                ),
                            )
                            .child(self.results(cx))
                            .when(loading, |this| this.child(waiting("video-search-waiting")))
                            .when(has_more && !loading, |this| {
                                this.child(more("video-search-more", move |cx| {
                                    next.update(cx, |search, cx| search.load_more(cx));
                                }))
                            }),
                    ),
            )
            .into_any_element()
    }
}

/// The page a video plays on: the picture, its controls, what it is, its comments and what to
/// watch next.
pub(crate) struct WatchView {
    watching: Entity<Watching>,
    subscriptions: Entity<Subscriptions>,
    scrollbar: Entity<Scrollbar>,
    seek: ScrubberState,
    volume: ScrubberState,
    /// Where a drag of the scrubber stands, until its release seeks there.
    pending: Option<f32>,
    width: Pixels,
    /// Whether there is room for the videos to watch next beside the picture.
    beside: bool,
    /// The picture-in-picture window, while one is open.
    pip: Option<AnyWindowHandle>,
    /// Whether the whole description shows rather than its opening lines.
    described: bool,
    /// Whether the volume is being dragged, which keeps its slider open.
    adjusting: bool,
    /// Whether the pointer is over the speaker or its slider.
    over_volume: bool,
    /// Whether the comments show in a panel beside the picture rather than under the page.
    discussed: bool,
    /// Whether the picture spans the page with everything else under it.
    theater: bool,
    /// Whether going full screen put the window into the system's full screen, so leaving undoes it.
    entered: bool,
    /// When the pointer last moved over the full screen picture, which keeps its controls up.
    stirred: Instant,
    _tick: Task<()>,
}

impl WatchView {
    pub(crate) fn new(
        watching: Entity<Watching>,
        subscriptions: Entity<Subscriptions>,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = cx.entity_id();
        cx.observe(&watching, |_, _, cx| cx.notify()).detach();
        cx.observe(&subscriptions, |_, _, cx| cx.notify()).detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();

        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this.update(cx, |this: &mut Self, cx| {
                    let playing = this
                        .watching
                        .read(cx)
                        .player()
                        .is_some_and(|player| !player.paused());
                    if playing {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        Self {
            watching,
            subscriptions,
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(id)),
            seek: ScrubberState::new("video-seek"),
            volume: ScrubberState::new("video-volume"),
            pending: None,
            width: Pixels::ZERO,
            beside: false,
            pip: None,
            described: false,
            adjusting: false,
            over_volume: false,
            discussed: false,
            theater: false,
            entered: false,
            stirred: Instant::now(),
            _tick: tick,
        }
    }

    /// Moves the picture into a small window that stays above the others, or brings it back.
    fn toggle_pip(&mut self, cx: &mut Context<Self>) {
        if self.watching.read(cx).floating() {
            if let Some(pip) = self.pip.take() {
                pip.update(cx, |_, window, _| window.remove_window()).ok();
            }
            self.watching
                .update(cx, |watching, cx| watching.set_floating(false, cx));
            return;
        }
        let Some(picture) = self.watching.update(cx, |watching, cx| watching.mirror(cx)) else {
            return;
        };
        // The corner of the screen the main window is on, or wherever the platform puts it.
        let corner = cx.primary_display().map(|display| {
            let screen = display.bounds();
            Bounds::new(
                screen.bottom_right()
                    - point(PIP_SIZE.width, PIP_SIZE.height)
                    - point(PIP_MARGIN, PIP_MARGIN),
                PIP_SIZE,
            )
        });
        let watching = self.watching.clone();
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: corner.map(WindowBounds::Windowed),
                titlebar: None,
                kind: WindowKind::PopUp,
                focus: false,
                is_movable: true,
                is_resizable: false,
                is_minimizable: false,
                ..Default::default()
            },
            |window, cx| {
                window.set_rem_size(cx.theme().font_size);
                cx.new(|cx| PipView::new(watching, picture, cx))
            },
        );
        match opened {
            Ok(pip) => {
                self.pip = Some(pip.into());
                self.watching
                    .update(cx, |watching, cx| watching.set_floating(true, cx));
            }
            Err(error) => log::warn!("video: cannot open picture in picture: {error:#}"),
        }
    }

    /// The picture, letterboxed in a 16:9 box as wide as the page allows.
    /// The picture, `room` large. Pressing it plays or pauses and pressing it twice fills the screen.
    fn stage(&self, room: Size<Pixels>, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let view = self.watching.read(cx).view();
        let floating = self.watching.read(cx).floating();
        let full = self.watching.read(cx).full();
        let toggled = self.watching.clone();

        div()
            .id("video-stage")
            .role(Role::Video)
            .aria_label(t!("video-picture"))
            .w(room.width)
            .h(room.height)
            .flex_none()
            .when(!full, |this| {
                this.rounded(theme.radius).corner_smoothing(theme.smoothing)
            })
            .overflow_hidden()
            .bg(gpui::black())
            .on_click(move |event, _, cx| {
                toggled.update(cx, |watching, cx| match event.click_count() {
                    2 => watching.set_full(!full, cx),
                    _ => watching.toggle(cx),
                })
            })
            .map(|stage| match floating {
                true => stage
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(theme.text(Text::Label))
                    .text_color(theme.overlay_foreground)
                    .child(t!("video-pip-playing")),
                false => stage.when_some(view, |this, view| this.child(view)),
            })
    }

    fn controls(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let watching = self.watching.read(cx);
        let player = watching.player();
        let paused = player.is_none_or(|player| player.paused());
        let position = player.and_then(|player| player.position()).unwrap_or(0.);
        let total = player.and_then(|player| player.duration()).unwrap_or(0.);
        let seekable = total > 0.;
        let (level, muted, captions) = (watching.volume(), watching.muted(), watching.captions());
        let (captioned, silenced) = (self.watching.clone(), self.watching.clone());
        let (scripted, script) = (watching.transcript_open(), self.watching.clone());
        let floating = watching.floating();
        let (full, filled) = (watching.full(), self.watching.clone());
        let (discussed, theater) = (self.discussed, self.theater);
        let fraction = match self.pending {
            Some(pending) => pending,
            None if seekable => (position / total).clamp(0., 1.) as f32,
            None => 0.,
        };
        let elapsed = match self.pending {
            Some(pending) => f64::from(pending) * total,
            None => position,
        };
        let elapsed = clock(Duration::from_secs_f64(elapsed.max(0.)));
        let (toggled, back, ahead) = (
            self.watching.clone(),
            self.watching.clone(),
            self.watching.clone(),
        );
        let time = |text: SharedString| {
            div()
                .flex_none()
                .text_size(theme.text(Text::Small))
                .text_color(theme.muted_foreground)
                .child(text)
        };

        let ends = SharedString::from(format!(
            "{elapsed} / {}",
            clock(Duration::from_secs_f64(total.max(0.)))
        ));
        let buttons = div()
            .flex()
            .w_full()
            .items_center()
            .gap_2()
            .child(
                Button::new("video-back")
                    .icon("icons/skip-back.svg")
                    .ghost()
                    .tooltip("video-skip-back")
                    .on_click(move |_, _, cx| {
                        back.update(cx, |watching, cx| watching.seek_by(-SKIP, cx));
                    }),
            )
            .child(
                Button::new("video-toggle")
                    .icon(match paused {
                        true => "icons/play-filled.svg",
                        false => "icons/pause-filled.svg",
                    })
                    .ghost()
                    .tooltip(match paused {
                        true => "video-play",
                        false => "video-pause",
                    })
                    .on_click(move |_, _, cx| {
                        toggled.update(cx, |watching, cx| watching.toggle(cx));
                    }),
            )
            .child(
                Button::new("video-ahead")
                    .icon("icons/skip-forward.svg")
                    .ghost()
                    .tooltip("video-skip-ahead")
                    .on_click(move |_, _, cx| {
                        ahead.update(cx, |watching, cx| watching.seek_by(SKIP, cx));
                    }),
            )
            .child(time(ends))
            .child(div().flex_1())
            .child(
                Button::new("video-transcript")
                    .icon("icons/text-select.svg")
                    .ghost()
                    .selected(scripted)
                    .tooltip(match scripted {
                        true => "video-transcript-hide",
                        false => "video-transcript-show",
                    })
                    .on_click(move |_, _, cx| {
                        script.update(cx, |watching, cx| watching.toggle_transcript(cx));
                    }),
            )
            .child(
                Button::new("video-discussion")
                    .icon("icons/message-square.svg")
                    .ghost()
                    .selected(discussed)
                    .tooltip(match discussed {
                        true => "video-discussion-hide",
                        false => "video-discussion-show",
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.discussed = !this.discussed;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("video-captions")
                    .label(t!("video-captions"))
                    .ghost()
                    .selected(captions)
                    .tooltip(match captions {
                        true => "video-captions-hide",
                        false => "video-captions-show",
                    })
                    .on_click(move |_, _, cx| {
                        captioned.update(cx, |watching, cx| watching.toggle_captions(cx));
                    }),
            )
            .child(
                Button::new("video-pip")
                    .icon("icons/external-link.svg")
                    .ghost()
                    .selected(floating)
                    .tooltip(match floating {
                        true => "video-pip-close",
                        false => "video-pip-open",
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_pip(cx))),
            )
            .when(!full, |this| {
                this.child(
                    Button::new("video-theater")
                        .icon("icons/rectangle-horizontal.svg")
                        .ghost()
                        .selected(theater)
                        .tooltip(match theater {
                            true => "video-theater-leave",
                            false => "video-theater-enter",
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.theater = !this.theater;
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("video-full")
                    .icon(match full {
                        true => "icons/minimize.svg",
                        false => "icons/maximize.svg",
                    })
                    .ghost()
                    .tooltip(match full {
                        true => "video-full-leave",
                        false => "video-full-enter",
                    })
                    .on_click(move |_, _, cx| {
                        filled.update(cx, |watching, cx| watching.set_full(!full, cx));
                    }),
            )
            .child(
                // The slider folds away until the pointer is over the speaker or dragging it.
                div()
                    .id("video-volume-fold")
                    .on_hover(cx.listener(|this, over: &bool, _, cx| {
                        this.over_volume = *over;
                        cx.notify();
                    }))
                    .flex()
                    .flex_none()
                    .items_center()
                    .child(
                        Button::new("video-mute")
                            .icon(volume_icon(level))
                            .ghost()
                            .tooltip(match muted {
                                true => "video-unmute",
                                false => "video-mute",
                            })
                            .on_click(move |_, _, cx| {
                                silenced.update(cx, |watching, cx| watching.toggle_muted(cx));
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .overflow_hidden()
                            .map(|slider| match self.adjusting || self.over_volume {
                                true => slider.w(VOLUME_WIDTH).pl_2(),
                                false => slider.w_0(),
                            })
                            .child(
                                Scrubber::new(&self.volume, level)
                                    .name(t!("video-volume"))
                                    .value(percent(level))
                                    .colors(theme.progress_bar, theme.muted, theme.foreground)
                                    .on_move(cx.listener(|this, fraction: &f32, _, cx| {
                                        let level = *fraction;
                                        this.adjusting = true;
                                        this.watching.update(cx, |watching, cx| {
                                            watching.set_volume(level, cx)
                                        });
                                    }))
                                    .on_release(cx.listener(|this, _: &MouseUpEvent, _, cx| {
                                        this.adjusting = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap_1()
            .child(
                div().w_full().child(
                    Scrubber::new(&self.seek, fraction)
                        .name(t!("video-position"))
                        .value(elapsed)
                        .colors(theme.progress_bar, theme.muted, theme.foreground)
                        .enabled(seekable)
                        .on_move(cx.listener(|this, fraction: &f32, _, cx| {
                            this.pending = Some(*fraction);
                            cx.notify();
                        }))
                        .on_release(cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                            let Some(fraction) = this.pending.take() else {
                                return;
                            };
                            this.watching.update(cx, |watching, cx| {
                                watching.seek_to(f64::from(fraction) * total, cx);
                            });
                        })),
                ),
            )
            .child(buttons)
    }

    /// The transcript panel, `height` tall: its heading with the buttons to copy and close it,
    /// then every line with its time, the one being spoken lit. A line seeks to its time.
    fn transcript(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let theme = *cx.theme();
        let watching = self.watching.read(cx);
        if !watching.transcript_open() {
            return None;
        }
        let lines = watching.transcript();
        let now = watching
            .player()
            .and_then(|player| player.position())
            .unwrap_or(0.);
        let title = t!("video-transcript");
        let (closed, retried) = (self.watching.clone(), self.watching.clone());
        let copied = lines.clone().filter(|lines| !lines.is_empty());
        let note = |id: &'static str, label: SharedString| {
            div()
                .id(id)
                .role(Role::Status)
                .aria_label(label.clone())
                .p_3()
                .text_size(theme.text(Text::Small))
                .text_color(theme.muted_foreground)
                .child(label)
        };
        let body = match lines {
            None if watching.transcript_failed() => div()
                .flex()
                .flex_col()
                .items_start()
                .child(note("video-transcript-lost", t!("video-transcript-lost")))
                .child(
                    div().px_3().child(
                        Button::new("video-transcript-retry")
                            .label(t!("video-transcript-retry"))
                            .on_click(move |_, _, cx| {
                                retried.update(cx, |watching, cx| watching.load_transcript(cx));
                            }),
                    ),
                )
                .into_any_element(),
            None => waiting("video-transcript-waiting").into_any_element(),
            Some(lines) if lines.is_empty() => {
                note("video-transcript-none", t!("video-transcript-none")).into_any_element()
            }
            Some(lines) => {
                let spoken = lines.iter().rposition(|line| line.start <= now);
                let total = lines.len();
                div()
                    .flex()
                    .flex_col()
                    .p_1()
                    .children(lines.iter().enumerate().map(|(place, line)| {
                        let (sought, start) = (self.watching.clone(), line.start);
                        let text = SharedString::from(line.text.clone());
                        let lit = spoken == Some(place);
                        div()
                            .id(("video-transcript-line", (start * 1000.) as usize))
                            .role(Role::Button)
                            .aria_label(text.clone())
                            .aria_position_in_set(place + 1)
                            .aria_size_of_set(total)
                            .flex()
                            .gap_3()
                            .px_2()
                            .py_1()
                            .rounded(theme.radius)
                            .corner_smoothing(theme.smoothing)
                            .cursor_pointer()
                            .text_size(theme.text(Text::Small))
                            .when(lit, |this| this.bg(theme.secondary))
                            .hover(|this| this.bg(theme.secondary_hover))
                            .on_click(move |_, _, cx| {
                                sought.update(cx, |watching, cx| watching.seek_to(start, cx));
                            })
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(theme.muted_foreground)
                                    .child(clock(Duration::from_secs_f64(start.max(0.)))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(match lit {
                                        true => theme.foreground,
                                        false => theme.muted_foreground,
                                    })
                                    .child(text),
                            )
                    }))
                    .into_any_element()
            }
        };

        Some(
            div()
                .flex()
                .flex_col()
                .w_full()
                .flex_1()
                .min_h_0()
                .rounded(theme.radius)
                .corner_smoothing(theme.smoothing)
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar)
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .pl_3()
                        .pr_1()
                        .py_1()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .id("video-transcript-heading")
                                .role(Role::Heading)
                                .aria_level(2)
                                .aria_label(title.clone())
                                .flex_1()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(theme.text(Text::Label))
                                .text_color(theme.foreground)
                                .child(title),
                        )
                        .when_some(copied, |this, lines| {
                            this.child(
                                Button::new("video-transcript-copy")
                                    .icon("icons/copy.svg")
                                    .ghost()
                                    .tooltip("video-transcript-copy")
                                    .on_click(move |_, _, cx| {
                                        let text = lines
                                            .iter()
                                            .map(|line| line.text.as_str())
                                            .collect::<Vec<_>>()
                                            .join("\n");
                                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                                        Toasts::show(Outcome::Done, "toast-transcript-copied", cx);
                                    }),
                            )
                        })
                        .child(
                            Button::new("video-transcript-close")
                                .icon("icons/x.svg")
                                .ghost()
                                .tooltip("video-transcript-hide")
                                .on_click(move |_, _, cx| {
                                    closed.update(cx, |watching, cx| {
                                        watching.toggle_transcript(cx);
                                    });
                                }),
                        ),
                )
                .child(
                    div()
                        .id("video-transcript-lines")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(body),
                )
                .into_any_element(),
        )
    }

    /// The comments in a panel shaped like the transcript's, once they are asked for there.
    fn discussion(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let theme = *cx.theme();
        if !self.discussed {
            return None;
        }
        let title: SharedString = match self.watching.read(cx).comment_total() {
            Some(total) => total.to_owned().into(),
            None => t!("video-comments"),
        };

        Some(
            div()
                .flex()
                .flex_col()
                .w_full()
                .flex_1()
                .min_h_0()
                .rounded(theme.radius)
                .corner_smoothing(theme.smoothing)
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar)
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .pl_3()
                        .pr_1()
                        .py_1()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .id("video-discussion-heading")
                                .role(Role::Heading)
                                .aria_level(2)
                                .aria_label(title.clone())
                                .flex_1()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(theme.text(Text::Label))
                                .text_color(theme.foreground)
                                .child(title),
                        )
                        .child(
                            Button::new("video-discussion-close")
                                .icon("icons/x.svg")
                                .ghost()
                                .tooltip("video-discussion-hide")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.discussed = false;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .id("video-discussion-lines")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_3()
                        .child(self.comments(false, cx)),
                )
                .into_any_element(),
        )
    }

    /// The panels asked for, the transcript first: side by side they share their column's height.
    fn panels(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        self.transcript(cx)
            .into_iter()
            .chain(self.discussion(cx))
            .collect()
    }

    /// The picture over the whole window, its controls coming up while the pointer moves or the
    /// video is paused.
    fn screen(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let room = window.viewport_size();
        let paused = self
            .watching
            .read(cx)
            .player()
            .is_none_or(|player| player.paused());
        let shown = paused || self.stirred.elapsed() < LINGER;

        deferred(
            anchored().position(Point::default()).child(
                div()
                    .id("video-screen")
                    .relative()
                    .w(room.width)
                    .h(room.height)
                    .bg(gpui::black())
                    .occlude()
                    .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _, cx| {
                        let hidden = this.stirred.elapsed() >= LINGER;
                        this.stirred = Instant::now();
                        if hidden {
                            cx.notify();
                        }
                    }))
                    .child(self.stage(room, cx))
                    .when(shown, |this| {
                        this.child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left_0()
                                .right_0()
                                .px_4()
                                .py_2()
                                .bg(theme.background.opacity(VEIL))
                                .child(self.controls(cx)),
                        )
                    }),
            ),
        )
        .with_priority(1)
    }

    /// The title, the numbers under it, the channel and the description.
    fn about(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let theme = cx.theme();
        let watch = self.watching.read(cx).watch()?;
        let title = SharedString::from(watch.title.clone());
        let numbers = caption([
            watch.views.clone(),
            watch.published.clone(),
            watch
                .likes
                .as_ref()
                .map(|likes| t!("video-likes", likes = likes.as_str()).to_string()),
        ]);
        let opening: Vec<&str> = watch.description.lines().take(OPENING).collect();
        let long = watch.description.lines().count() > OPENING;

        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .id("video-title")
                        .role(Role::Heading)
                        .aria_level(1)
                        .aria_label(title.clone())
                        .text_size(theme.text(Text::Large))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(numbers),
                )
                .when_some(watch.channel.as_ref(), |this, channel| {
                    let name = SharedString::from(channel.name.clone());
                    let opened = SharedString::from(channel.id.clone());
                    let button = subscribe("video-subscribe", channel, &self.subscriptions, cx);
                    let saved = self.subscriptions.read(cx).saved(&watch.id);
                    let (kept, page) = (self.subscriptions.clone(), self.watching.clone());
                    let later = Button::new("video-later")
                        .icon(match saved {
                            true => "icons/check.svg",
                            false => "icons/clock.svg",
                        })
                        .label(t!("video-later"))
                        .secondary()
                        .selected(saved)
                        .tooltip(match saved {
                            true => "video-later-remove",
                            false => "video-later-add",
                        })
                        .on_click(move |_, _, cx| {
                            let Some(video) = page.read(cx).video() else {
                                return;
                            };
                            kept.update(cx, |kept, cx| kept.toggle_later(&video, cx));
                        });
                    this.child(
                        div()
                            .id("video-channel")
                            .role(Role::Link)
                            .aria_label(name.clone())
                            .flex()
                            .items_center()
                            .gap_3()
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                navigate(Destination::Channel(opened.clone()), cx);
                            })
                            .child(Avatar::new(channel.avatar.clone()).size(theme.metrics.control))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(theme.text(Text::Label))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(name),
                                    )
                                    .when_some(channel.subscribers.clone(), |this, count| {
                                        this.child(
                                            div()
                                                .text_size(theme.text(Text::Small))
                                                .text_color(theme.muted_foreground)
                                                .child(count),
                                        )
                                    }),
                            )
                            .child(button)
                            .child(later),
                    )
                })
                .when(!watch.description.is_empty(), |this| {
                    this.child(
                        div()
                            .p(theme.metrics.pad)
                            .rounded(theme.radius)
                            .corner_smoothing(theme.smoothing)
                            .bg(theme.secondary)
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_1()
                            .text_size(theme.text(Text::Label))
                            .child(SharedString::from(match long && !self.described {
                                true => opening.join("\n"),
                                false => watch.description.clone(),
                            }))
                            .when(long, |this| {
                                this.child(
                                    Button::new("video-description-more")
                                        .label(match self.described {
                                            true => t!("common-show-less"),
                                            false => t!("video-more"),
                                        })
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.described = !this.described;
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                }),
        )
    }

    fn comment(&self, comment: &Comment, place: (usize, usize), cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let watching = self.watching.read(cx);
        let replies = watching.replies(&comment.id);
        let byline = caption([
            Some(comment.author.name.clone()),
            comment.published.clone(),
            comment
                .creator
                .then(|| t!("video-comment-creator").to_string()),
        ]);
        let text = SharedString::from(comment.text.clone());
        let opened = SharedString::from(comment.author.id.clone());

        div()
            .id(SharedString::from(format!("video-comment-{}", comment.id)))
            .role(Role::Comment)
            .aria_label(byline.clone())
            .aria_description(text.clone())
            .aria_position_in_set(place.0 + 1)
            .aria_size_of_set(place.1)
            .flex()
            .gap_3()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "video-commenter-{}",
                        comment.id
                    )))
                    .role(Role::Link)
                    .aria_label(SharedString::from(comment.author.name.clone()))
                    .flex_none()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| navigate(Destination::Channel(opened.clone()), cx))
                    .child(Avatar::new(comment.avatar.clone()).size(theme.metrics.control)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_size(theme.text(Text::Small))
                            .text_color(theme.muted_foreground)
                            .child(byline),
                    )
                    .child(div().text_size(theme.text(Text::Label)).child(text))
                    .when_some(comment.likes.clone(), |this, likes| {
                        this.child(
                            div()
                                .text_size(theme.text(Text::Small))
                                .text_color(theme.muted_foreground)
                                .child(t!("video-likes", likes = likes.as_str())),
                        )
                    })
                    .when(comment.replies.is_some() && replies.is_none(), |this| {
                        let (watching, id) = (self.watching.clone(), comment.id.clone());
                        let label: SharedString = match comment.reply_count.as_deref() {
                            Some(count) => t!("video-replies-count", count = count),
                            None => t!("video-replies"),
                        };
                        this.child(
                            div().flex().child(
                                Button::new(SharedString::from(format!(
                                    "video-replies-{}",
                                    comment.id
                                )))
                                .label(label)
                                .ghost()
                                .small()
                                .on_click(move |_, _, cx| {
                                    watching
                                        .update(cx, |watching, cx| watching.load_replies(&id, cx));
                                }),
                            ),
                        )
                    })
                    .when_some(replies, |this, replies| {
                        let total = replies.len();
                        this.child(match total {
                            0 => waiting("video-replies-waiting").into_any_element(),
                            _ => {
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_4()
                                    .pt_2()
                                    .children(replies.iter().enumerate().map(|(place, reply)| {
                                        self.comment(reply, (place, total), cx)
                                    }))
                                    .into_any_element()
                            }
                        })
                    }),
            )
            .into_any_element()
    }

    /// The comments, under their count as a heading when `headed`.
    fn comments(&self, headed: bool, cx: &Context<Self>) -> impl IntoElement {
        let watching = self.watching.read(cx);
        let comments = watching.comments();
        let total = comments.len();
        let (loading, has_more) = (watching.comments_loading(), watching.has_more_comments());
        let title: SharedString = match watching.comment_total() {
            Some(total) => total.to_owned().into(),
            None => t!("video-comments"),
        };
        let quiet = total == 0 && !loading && !watching.is_loading();
        let next = self.watching.clone();

        div()
            .flex()
            .flex_col()
            .gap_4()
            .when(headed, |this| {
                this.child(
                    heading(title.clone(), cx)
                        .id("video-comments-heading")
                        .role(Role::Heading)
                        .aria_level(2)
                        .aria_label(title),
                )
            })
            .children(
                comments
                    .iter()
                    .enumerate()
                    .map(|(place, comment)| self.comment(comment, (place, total), cx)),
            )
            .when(quiet, |this| {
                let label = t!("video-comments-empty");
                this.child(
                    div()
                        .id("video-comments-empty")
                        .role(Role::Status)
                        .aria_label(label.clone())
                        .child(vacant(label, cx)),
                )
            })
            .when(loading, |this| {
                this.child(waiting("video-comments-waiting"))
            })
            .when(has_more && !loading, |this| {
                this.child(more("video-comments-more", move |cx| {
                    next.update(cx, |watching, cx| watching.load_comments(cx));
                }))
            })
    }

    fn related(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let related = &self.watching.read(cx).watch()?.related;
        if related.is_empty() {
            return None;
        }
        let total = related.len();
        let title = t!("video-related");
        let tiles = Tiles::new(self.width, cx);
        let videos = tiles.grid(
            related
                .iter()
                .enumerate()
                .map(|(place, video)| {
                    video_tile(
                        "related",
                        video,
                        (place, total),
                        tiles,
                        &self.subscriptions,
                        cx,
                    )
                })
                .collect(),
        );

        Some(
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    heading(title.clone(), cx)
                        .id("video-related-heading")
                        .role(Role::Heading)
                        .aria_level(2)
                        .aria_label(title),
                )
                .child(videos),
        )
    }
}

/// The picture on its own in a small window above the others. It is dragged by the picture, and
/// its buttons show while the pointer is over it.
pub(crate) struct PipView {
    watching: Entity<Watching>,
    picture: AnyView,
}

impl PipView {
    fn new(watching: Entity<Watching>, picture: AnyView, cx: &mut Context<Self>) -> Self {
        cx.observe(&watching, |_, _, cx| cx.notify()).detach();
        // However the window goes, the watch page gets its picture back.
        let back = watching.downgrade();
        cx.on_release(move |_, cx| {
            back.update(cx, |watching, cx| watching.set_floating(false, cx))
                .ok();
        })
        .detach();
        Self { watching, picture }
    }
}

impl Render for PipView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let watching = self.watching.read(cx);
        let paused = watching.player().is_none_or(|player| player.paused());
        let (toggled, closed) = (self.watching.clone(), self.watching.clone());

        div()
            .id("video-pip")
            .role(Role::Video)
            .aria_label(t!("video-pip"))
            .group("video-pip")
            .relative()
            .size_full()
            .bg(gpui::black())
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_, window, _| {
                window.start_window_move();
            })
            .child(self.picture.clone())
            .child(
                div()
                    .absolute()
                    .top_1()
                    .right_1()
                    .flex()
                    .gap_1()
                    .p_0p5()
                    .rounded(theme.radius)
                    .corner_smoothing(theme.smoothing)
                    .bg(theme.overlay)
                    .invisible()
                    .group_hover("video-pip", |this| this.visible())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        Button::new("video-pip-toggle")
                            .icon(match paused {
                                true => "icons/play-filled.svg",
                                false => "icons/pause-filled.svg",
                            })
                            .ghost()
                            .tooltip(match paused {
                                true => "video-play",
                                false => "video-pause",
                            })
                            .on_click(move |_, _, cx| {
                                toggled.update(cx, |watching, cx| watching.toggle(cx));
                            }),
                    )
                    .child(
                        Button::new("video-pip-back")
                            .icon("icons/x.svg")
                            .ghost()
                            .tooltip("video-pip-close")
                            .on_click(move |_, window, cx| {
                                closed.update(cx, |watching, cx| {
                                    watching.set_floating(false, cx);
                                });
                                window.remove_window();
                            }),
                    ),
            )
    }
}

impl Render for WatchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pad = cx.theme().metrics.inset;
        let gap = cx.theme().metrics.inset;
        let whole = cells::content_width(window, pad * 2., cx);
        let full = self.watching.read(cx).full();
        if full != self.entered {
            // Leaving only undoes a full screen this went into, so a window already there stays.
            if full != window.is_fullscreen() {
                window.toggle_fullscreen();
            }
            self.entered = full;
            self.stirred = Instant::now();
        }
        // A panel sits beside the picture only once it is asked for and there is room.
        let side = self.discussed || self.watching.read(cx).transcript_open();
        self.beside = side && Chrome::room(window, cx).fits(Room::Wide);
        let room = match self.beside {
            true => whole.min(WATCH_MAX) - NEXT_WIDTH - gap,
            false => whole.min(STAGE_MAX),
        };
        if (room - self.width).abs() >= STEADY {
            self.width = room;
        }
        if let Some(view) = self.watching.read(cx).view() {
            let radius = match full {
                true => Pixels::ZERO,
                false => cx.theme().radius,
            };
            view.update(cx, |view, cx| view.round(radius, cx));
        }
        let (beside, theater, discussed) = (self.beside, self.theater, self.discussed);
        let viewport = window.viewport_size();
        let stage = match theater {
            true => size(
                whole,
                (whole * 9. / 16.)
                    .min(viewport.height * THEATER_SHARE)
                    .floor(),
            ),
            false => size(self.width, (self.width * 9. / 16.).floor()),
        };
        // Beside the picture the panels run down to the foot of the window.
        let column = (viewport.height - cx.theme().metrics.title_bar - pad * 2.).max(SCRIPT_HEIGHT);
        let watching = self.watching.read(cx);
        let loading = watching.is_loading();
        let failure = watching.error().map(str::to_owned);
        let retried = self.watching.clone();
        let player = (!full).then(|| {
            div()
                .flex()
                .flex_col()
                .flex_none()
                .w(stage.width)
                .gap_2()
                .child(self.stage(stage, cx))
                .child(self.controls(cx))
        });
        let (above, within) = match theater {
            true => (player, None),
            false => (None, player),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .when(full, |this| this.child(self.screen(window, cx)))
            .child(
                Scroller::new("video-watch-page", &self.scrollbar)
                    .hold()
                    .bottom_inset(Chrome::bottom(cx))
                    .py(pad)
                    .px(pad + Chrome::margin(window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_6()
                            .children(above)
                            .child(
                                div()
                                    .flex()
                                    .items_start()
                                    .justify_center()
                                    .gap(gap)
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .flex_none()
                                            .w(self.width)
                                            .gap_6()
                                            .children(within)
                                            .when(!beside, |this| {
                                                this.children(self.panels(cx).into_iter().map(
                                                    |panel| {
                                                        div()
                                                            .flex()
                                                            .flex_col()
                                                            .h(SCRIPT_HEIGHT)
                                                            .child(panel)
                                                    },
                                                ))
                                            })
                                            .when(loading, |this| {
                                                this.child(waiting("video-watch-waiting"))
                                            })
                                            .when_some(failure, |this, reason| {
                                                this.child(
                                                    div()
                                                        .id("video-watch-failure")
                                                        .role(Role::Alert)
                                                        .child(trouble::lost(
                                                            "video-watch-lost",
                                                            t!("trouble-not-loaded"),
                                                            Some(&reason),
                                                            move |_, _, cx| {
                                                                retried.update(
                                                                    cx,
                                                                    |watching, cx| {
                                                                        watching.refresh(cx)
                                                                    },
                                                                );
                                                            },
                                                            cx,
                                                        )),
                                                )
                                            })
                                            .children(self.about(cx))
                                            .children(self.related(cx))
                                            .when(
                                                !discussed
                                                    && self.watching.read(cx).watch().is_some(),
                                                |this| this.child(self.comments(true, cx)),
                                            ),
                                    )
                                    .when(beside, |page| {
                                        page.child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .flex_none()
                                                .w(NEXT_WIDTH)
                                                .h(column)
                                                .gap(gap)
                                                .children(self.panels(cx)),
                                        )
                                    }),
                            ),
                    ),
            )
    }
}

/// The page of a channel or a playlist: a header and its videos.
pub(crate) struct VideoListView {
    list: Entity<VideoList>,
    subscriptions: Entity<Subscriptions>,
    scrollbar: Entity<Scrollbar>,
    width: Pixels,
}

impl VideoListView {
    pub(crate) fn new(
        list: Entity<VideoList>,
        subscriptions: Entity<Subscriptions>,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = cx.entity_id();
        cx.observe(&list, |_, _, cx| cx.notify()).detach();
        cx.observe(&subscriptions, |_, _, cx| cx.notify()).detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();

        Self {
            list,
            subscriptions,
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(id)),
            width: Pixels::ZERO,
        }
    }

    fn header(&self, cx: &Context<Self>) -> impl IntoElement {
        let list = self.list.read(cx);
        let channel = list.channel();
        let mut strip = HeroMetaStrip::new();
        if let Some(subscribers) = channel.and_then(|channel| channel.subscribers.clone()) {
            strip = strip.text(subscribers);
        }

        PageHero::new(
            "video-list-hero",
            SharedString::from(list.title().to_owned()),
        )
        .pending(list.title().is_empty() && list.is_loading())
        .cover(channel.and_then(|channel| channel.avatar.clone()))
        .map(|hero| match channel.is_some() {
            true => hero
                .circle()
                .fallback(CHANNEL_ICON)
                .eyebrow(t!("video-channel")),
            false => hero.fallback(PLAYLIST_ICON).eyebrow(t!("video-playlist")),
        })
        .meta(strip)
    }
}

impl Render for VideoListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = self.list.read(cx);
        if let Some(reason) = list.error().map(str::to_owned) {
            let list = self.list.clone();
            return div()
                .id("video-list-failure")
                .role(Role::Alert)
                .flex()
                .flex_col()
                .size_full()
                .child(
                    trouble::lost(
                        "video-list-lost",
                        t!("trouble-not-loaded"),
                        Some(&reason),
                        move |_, _, cx| list.update(cx, |list, cx| list.refresh(cx)),
                        cx,
                    )
                    .size_full(),
                )
                .into_any_element();
        }

        let pad = cx.theme().metrics.inset;
        let room = cells::content_width(window, pad * 2., cx);
        if (room - self.width).abs() >= STEADY {
            self.width = room;
        }
        let list = self.list.read(cx);
        let (loading, has_more) = (list.is_loading(), list.has_more());
        let tiles = Tiles::new(self.width, cx);
        let videos = list.videos();
        let total = videos.len();
        let cards: Vec<AnyElement> = videos
            .iter()
            .enumerate()
            .map(|(place, video)| {
                video_tile(
                    "list",
                    video,
                    (place, total),
                    tiles,
                    &self.subscriptions,
                    cx,
                )
            })
            .collect();
        let empty = cards.is_empty() && !loading;
        let next = self.list.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                Scroller::new("video-list-page", &self.scrollbar)
                    .hold()
                    .bottom_inset(Chrome::bottom(cx))
                    .py(pad)
                    .px(pad + Chrome::margin(window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_8()
                            .child(self.header(cx))
                            .when_some(list.channel(), |this, channel| {
                                this.child(div().flex().child(subscribe(
                                    "video-list-subscribe",
                                    channel,
                                    &self.subscriptions,
                                    cx,
                                )))
                            })
                            .child(tiles.grid(cards))
                            .when(empty, |this| {
                                let label = t!("video-list-empty");
                                this.child(
                                    div()
                                        .id("video-list-empty")
                                        .role(Role::Status)
                                        .aria_label(label.clone())
                                        .child(vacant(label, cx)),
                                )
                            })
                            .when(loading, |this| this.child(waiting("video-list-waiting")))
                            .when(has_more && !loading, |this| {
                                this.child(more("video-list-more", move |cx| {
                                    next.update(cx, |list, cx| list.load_more(cx));
                                }))
                            }),
                    ),
            )
            .into_any_element()
    }
}

/// Which of the two pages kept on this machine a [`VideoFeedView`] shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Feed {
    /// The newest uploads of the channels subscribed to.
    Subscriptions,
    /// The videos watched, the latest first.
    History,
    /// The videos saved to watch later.
    Later,
}

/// The home page and the watch history: a heading and a grid of videos.
pub(crate) struct VideoFeedView {
    subscriptions: Entity<Subscriptions>,
    feed: Feed,
    scrollbar: Entity<Scrollbar>,
    width: Pixels,
}

impl VideoFeedView {
    pub(crate) fn new(
        subscriptions: Entity<Subscriptions>,
        feed: Feed,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = cx.entity_id();
        cx.observe(&subscriptions, |_, _, cx| cx.notify()).detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();

        Self {
            subscriptions,
            feed,
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(id)),
            width: Pixels::ZERO,
        }
    }
}

impl VideoFeedView {
    /// A heading with what goes at its far end.
    fn titled(&self, title: SharedString, trailing: Option<AnyElement>, cx: &App) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(heading(title, cx))
            .children(trailing)
            .into_any_element()
    }

    /// The followed channels as one row of avatars over their names, each opening its page.
    fn followed(&self, cx: &App) -> Option<AnyElement> {
        let theme = cx.theme();
        let channels = self.subscriptions.read(cx).channels();
        if self.feed != Feed::Subscriptions || channels.is_empty() {
            return None;
        }
        let total = channels.len();
        let face = theme.metrics.control * FACE;

        Some(
            div()
                .id("video-home-channels")
                .flex()
                .gap_4()
                .overflow_x_scroll()
                .children(channels.iter().enumerate().map(|(place, channel)| {
                    let name = SharedString::from(channel.name.clone());
                    let destination = Destination::Channel(channel.id.clone().into());
                    div()
                        .id(SharedString::from(format!("home-channel-{}", channel.id)))
                        .role(Role::Link)
                        .aria_label(name.clone())
                        .aria_position_in_set(place + 1)
                        .aria_size_of_set(total)
                        .flex()
                        .flex_col()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .w(face * FACE_ROOM)
                        .p_2()
                        .rounded(theme.radius)
                        .corner_smoothing(theme.smoothing)
                        .cursor_pointer()
                        .hover(|this| this.bg(theme.secondary))
                        .on_click(move |_, _, cx| navigate(destination.clone(), cx))
                        .child(Avatar::new(channel.avatar.clone()).size(face))
                        .child(
                            div()
                                .w_full()
                                .truncate()
                                .text_center()
                                .text_size(theme.text(Text::Small))
                                .text_color(theme.muted_foreground)
                                .child(name),
                        )
                }))
                .into_any_element(),
        )
    }

    /// The first row of the videos saved for later, on the home page, with a way to the rest.
    fn saved(&self, tiles: Tiles, cx: &App) -> Vec<AnyElement> {
        let later = self.subscriptions.read(cx).later();
        if self.feed != Feed::Subscriptions || later.is_empty() {
            return Vec::new();
        }
        let shown = &later[..later.len().min(tiles.columns)];
        let total = shown.len();
        let all = Button::new("video-later-all")
            .label(t!("video-later-all"))
            .ghost()
            .on_click(|_, _, cx| navigate(Destination::Later, cx));
        let cards = shown
            .iter()
            .enumerate()
            .map(|(place, video)| {
                video_tile(
                    "saved",
                    video,
                    (place, total),
                    tiles,
                    &self.subscriptions,
                    cx,
                )
            })
            .collect();
        vec![
            self.titled(t!("video-later"), Some(all.into_any_element()), cx),
            tiles.grid(cards).into_any_element(),
        ]
    }
}

impl Render for VideoFeedView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pad = cx.theme().metrics.inset;
        let room = cells::content_width(window, pad * 2., cx).min(FEED_MAX);
        if (room - self.width).abs() >= STEADY {
            self.width = room;
        }
        let kept = self.subscriptions.read(cx);
        let tiles = Tiles::new(self.width, cx);
        let (kind, title, hint, videos) = match self.feed {
            Feed::Subscriptions => (
                "feed",
                t!("video-home-latest"),
                t!("video-feed-empty"),
                kept.feed(),
            ),
            Feed::History => (
                "watched",
                t!("video-history"),
                t!("video-history-empty"),
                kept.watched(),
            ),
            Feed::Later => (
                "later",
                t!("video-later"),
                t!("video-later-empty"),
                kept.later(),
            ),
        };
        let home = self.feed == Feed::Subscriptions;
        let loading = home && kept.is_loading();
        let total = videos.len();
        let cards: Vec<AnyElement> = videos
            .iter()
            .enumerate()
            .map(|(place, video)| {
                video_tile(kind, video, (place, total), tiles, &self.subscriptions, cx)
            })
            .collect();
        let empty = cards.is_empty() && !loading;
        let cleared = self.subscriptions.clone();
        let trailing = (self.feed == Feed::History && !empty).then(|| {
            Button::new("video-history-clear")
                .label(t!("video-history-clear"))
                .secondary()
                .on_click(move |_, _, cx| {
                    cleared.update(cx, |kept, cx| kept.forget_watched(cx));
                })
                .into_any_element()
        });

        div().flex().flex_col().size_full().child(
            Scroller::new("video-feed-page", &self.scrollbar)
                .hold()
                .bottom_inset(Chrome::bottom(cx))
                .py(pad)
                .px(pad + Chrome::margin(window, cx))
                .child(
                    div().flex().justify_center().child(
                        div()
                            .flex()
                            .flex_col()
                            .w(self.width)
                            .gap_6()
                            .children(self.followed(cx))
                            .children(self.saved(tiles, cx))
                            .child(self.titled(title, trailing, cx))
                            .child(tiles.grid(cards))
                            .when(empty, |this| {
                                this.child(
                                    div()
                                        .id("video-feed-empty")
                                        .role(Role::Status)
                                        .aria_label(hint.clone())
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .gap_3()
                                        .child(vacant(hint, cx))
                                        .when(home, |this| {
                                            this.child(
                                                Button::new("video-home-find")
                                                    .icon("icons/search.svg")
                                                    .label(t!("video-home-find"))
                                                    .secondary()
                                                    .on_click(|_, _, cx| {
                                                        navigate(Destination::Search, cx)
                                                    }),
                                            )
                                        }),
                                )
                            })
                            .when(loading, |this| this.child(waiting("video-feed-waiting"))),
                    ),
                ),
        )
    }
}
