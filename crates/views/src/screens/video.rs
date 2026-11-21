//! The video screens: the search results, the page a video plays on and the page of a channel or
//! a playlist. All three lay videos out as the same thumbnail tiles.

use std::time::Duration;

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, MouseUpEvent, Pixels, Render, ScrollHandle,
    SharedString, Task, Window, div, px,
};
use i18n::t;
use router::{Destination, navigate};
use state::{VideoList, VideoSearch, Watching};
use tube::{Channel, Comment, Found, Playlist, Video};
use ui::{
    ActiveTheme as _, Artwork, Avatar, Button, Input, Scrollbar, Scroller, Scrubber, ScrubberState,
    Spinner, Submit, Text, clock, heading, vacant,
};

use crate::chrome::Chrome;
use crate::shared::cells;
use crate::shared::hero::{HeroMetaStrip, PageHero};
use crate::shared::trouble;

const CHANNEL_ICON: &str = "icons/user.svg";
const PLAYLIST_ICON: &str = "icons/list.svg";
/// The narrowest a video tile gets before the grid drops a column.
const TILE_MIN: Pixels = px(220.);
/// How wide the picture and the text under it may grow on the watch page.
const STAGE_MAX: Pixels = px(1120.);
const STEADY: Pixels = px(0.5);
/// How often the watch page redraws its clock and scrubber while a video plays.
const TICK: Duration = Duration::from_millis(250);
/// How far the skip buttons move the video, in seconds.
const SKIP: f64 = 10.;

/// The width of one tile and how many fit across `available`.
#[derive(Clone, Copy)]
struct Tiles {
    width: Pixels,
    gap: Pixels,
}

impl Tiles {
    fn new(available: Pixels, cx: &App) -> Self {
        let gap = cx.theme().metrics.pad;
        let available = available.max(TILE_MIN);
        let columns = ((available + gap) / (TILE_MIN + gap)).floor().max(1.);
        Self {
            width: ((available - gap * (columns - 1.)) / columns).floor(),
            gap,
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

/// A tile: a picture, a two-line title and a caption, opening `destination` when pressed.
fn tile(
    id: SharedString,
    picture: Artwork,
    title: &str,
    caption: SharedString,
    place: (usize, usize),
    destination: Destination,
    tiles: Tiles,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let title = SharedString::from(title.to_owned());

    div()
        .id(id)
        .role(Role::Link)
        .aria_label(title.clone())
        .aria_description(caption.clone())
        .aria_position_in_set(place.0 + 1)
        .aria_size_of_set(place.1)
        .flex()
        .flex_col()
        .flex_none()
        .w(tiles.width)
        .gap_2()
        .cursor_pointer()
        .on_click(move |_, _, cx| navigate(destination.clone(), cx))
        .child(picture)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .line_clamp(2)
                        .text_size(theme.text(Text::Label))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(title),
                )
                .child(
                    div()
                        .line_clamp(2)
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(caption),
                ),
        )
        .into_any_element()
}

/// The tile of one video: its thumbnail with the length in the caption.
fn video_tile(
    scope: &str,
    video: &Video,
    place: (usize, usize),
    tiles: Tiles,
    cx: &App,
) -> AnyElement {
    let picture = Artwork::new(Some(video.thumbnail()))
        .size(tiles.width)
        .wide((tiles.width * 9. / 16.).floor());
    let caption = caption([
        video.channel.as_ref().map(|channel| channel.name.clone()),
        video.views.clone(),
        video.published.clone(),
        video.length.clone(),
    ]);
    tile(
        format!("{scope}-video-{}", video.id).into(),
        picture,
        &video.title,
        caption,
        place,
        Destination::Watch(video.id.clone().into()),
        tiles,
        cx,
    )
}

fn channel_tile(
    scope: &str,
    channel: &Channel,
    place: (usize, usize),
    tiles: Tiles,
    cx: &App,
) -> AnyElement {
    let side = (tiles.width * 9. / 16.).floor();
    let picture = Artwork::new(channel.avatar.clone())
        .size(side)
        .circle()
        .fallback(CHANNEL_ICON);
    tile(
        format!("{scope}-channel-{}", channel.id).into(),
        picture,
        &channel.name,
        caption([
            Some(t!("video-channel").to_string()),
            channel.subscribers.clone(),
        ]),
        place,
        Destination::Channel(channel.id.clone().into()),
        tiles,
        cx,
    )
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
        .fallback(PLAYLIST_ICON);
    tile(
        format!("{scope}-playlist-{}", playlist.id).into(),
        picture,
        &playlist.title,
        caption([playlist.detail.clone()]),
        place,
        Destination::Videos(playlist.id.clone().into()),
        tiles,
        cx,
    )
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

/// The results of a video search.
pub(crate) struct VideoSearchView {
    search: Entity<VideoSearch>,
    input: Entity<Input>,
    scrollbar: Entity<Scrollbar>,
    width: Pixels,
}

impl VideoSearchView {
    pub(crate) fn new(search: Entity<VideoSearch>, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();

        let input = cx.new(|cx| {
            Input::new("video-search-placeholder", cx)
                .icon("icons/search.svg")
                .clearable()
        });

        Self {
            search,
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
        let total = found.len();

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

        let cards = found
            .iter()
            .enumerate()
            .map(|(place, found)| match found {
                Found::Video(video) => video_tile("search", video, (place, total), tiles, cx),
                Found::Channel(channel) => {
                    channel_tile("search", channel, (place, total), tiles, cx)
                }
                Found::Playlist(playlist) => {
                    playlist_tile("search", playlist, (place, total), tiles, cx)
                }
            })
            .collect();
        tiles.grid(cards).into_any_element()
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
                            .child(self.input.clone())
                            .child(
                                heading(title.clone(), cx)
                                    .id("video-search-heading")
                                    .role(Role::Heading)
                                    .aria_level(1)
                                    .aria_label(title),
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
    scrollbar: Entity<Scrollbar>,
    seek: ScrubberState,
    /// Where a drag of the scrubber stands, until its release seeks there.
    pending: Option<f32>,
    width: Pixels,
    _tick: Task<()>,
}

impl WatchView {
    pub(crate) fn new(watching: Entity<Watching>, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        cx.observe(&watching, |_, _, cx| cx.notify()).detach();
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
            scrollbar: cx.new(|_| Scrollbar::new(ScrollHandle::new()).watching(id)),
            seek: ScrubberState::new("video-seek"),
            pending: None,
            width: Pixels::ZERO,
            _tick: tick,
        }
    }

    /// The picture, letterboxed in a 16:9 box as wide as the page allows.
    fn stage(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let view = self.watching.read(cx).view();
        let toggled = self.watching.clone();

        div()
            .id("video-stage")
            .role(Role::Video)
            .aria_label(t!("video-picture"))
            .w(self.width)
            .h((self.width * 9. / 16.).floor())
            .rounded(theme.radius)
            .overflow_hidden()
            .bg(gpui::black())
            .on_click(move |_, _, cx| toggled.update(cx, |watching, cx| watching.toggle(cx)))
            .when_some(view, |this, view| this.child(view))
    }

    fn controls(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let watching = self.watching.read(cx);
        let player = watching.player();
        let paused = player.is_none_or(|player| player.paused());
        let position = player.and_then(|player| player.position()).unwrap_or(0.);
        let total = player.and_then(|player| player.duration()).unwrap_or(0.);
        let seekable = total > 0.;
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

        div()
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
            .child(time(elapsed.clone()))
            .child(
                div().flex_1().min_w_0().child(
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
            .child(time(clock(Duration::from_secs_f64(total.max(0.)))))
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
                            ),
                    )
                })
                .when(!watch.description.is_empty(), |this| {
                    this.child(
                        div()
                            .p(theme.metrics.pad)
                            .rounded(theme.radius)
                            .bg(theme.secondary)
                            .text_size(theme.text(Text::Label))
                            .line_clamp(6)
                            .child(SharedString::from(watch.description.clone())),
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

    fn comments(&self, cx: &Context<Self>) -> impl IntoElement {
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
            .child(
                heading(title.clone(), cx)
                    .id("video-comments-heading")
                    .role(Role::Heading)
                    .aria_level(2)
                    .aria_label(title),
            )
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
        let tiles = Tiles::new(self.width, cx);
        let total = related.len();
        let title = t!("video-related");

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
                .child(
                    tiles.grid(
                        related
                            .iter()
                            .enumerate()
                            .map(|(place, video)| {
                                video_tile("related", video, (place, total), tiles, cx)
                            })
                            .collect(),
                    ),
                ),
        )
    }
}

impl Render for WatchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pad = cx.theme().metrics.inset;
        let room = cells::content_width(window, pad * 2., cx).min(STAGE_MAX);
        if (room - self.width).abs() >= STEADY {
            self.width = room;
        }
        let watching = self.watching.read(cx);
        let loading = watching.is_loading();
        let failure = watching.error().map(str::to_owned);
        let retried = self.watching.clone();

        div().flex().flex_col().size_full().child(
            Scroller::new("video-watch-page", &self.scrollbar)
                .hold()
                .bottom_inset(Chrome::bottom(cx))
                .py(pad)
                .px(pad + Chrome::margin(window, cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(self.width)
                        .mx_auto()
                        .gap_6()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(self.stage(cx))
                                .child(self.controls(cx)),
                        )
                        .when(loading, |this| this.child(waiting("video-watch-waiting")))
                        .when_some(failure, |this, reason| {
                            this.child(div().id("video-watch-failure").role(Role::Alert).child(
                                trouble::lost(
                                    "video-watch-lost",
                                    t!("trouble-not-loaded"),
                                    Some(&reason),
                                    move |_, _, cx| {
                                        retried.update(cx, |watching, cx| watching.refresh(cx));
                                    },
                                    cx,
                                ),
                            ))
                        })
                        .children(self.about(cx))
                        .when(self.watching.read(cx).watch().is_some(), |this| {
                            this.child(self.comments(cx))
                        })
                        .children(self.related(cx)),
                ),
        )
    }
}

/// The page of a channel or a playlist: a header and its videos.
pub(crate) struct VideoListView {
    list: Entity<VideoList>,
    scrollbar: Entity<Scrollbar>,
    width: Pixels,
}

impl VideoListView {
    pub(crate) fn new(list: Entity<VideoList>, cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        cx.observe(&list, |_, _, cx| cx.notify()).detach();
        let chrome = Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();

        Self {
            list,
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
            .map(|(place, video)| video_tile("list", video, (place, total), tiles, cx))
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
