//! The video screens: the search results, the page a video plays on and the page of a channel or
//! a playlist. All three lay videos out as the same thumbnail tiles.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::AnimationExt as _;
use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnyElement, AnyView, AnyWindowHandle, App, Bounds, BoxShadow, ClipboardItem, Context, Entity,
    FontWeight, Hsla, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ResizeEdge,
    ScrollHandle, SharedString, Size, Task, Window, WindowBounds, WindowControlArea, WindowKind,
    WindowOptions, anchored, deferred, div, point, px, rgb, size,
};
use i18n::t;
use router::{Destination, navigate};
use state::{Outcome, Subscriptions, Toasts, VideoList, VideoSearch, Watching};
use tube::{Channel, Comment, Found, Playlist, Video};
use ui::{
    ActiveTheme as _, Artwork, Avatar, Button, Input, Menu, MenuItem, Motion, Popup, Room,
    Scrollbar, Scroller, Scrubber, ScrubberState, Spinner, Submit, Text, clock, heading, vacant,
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
/// How thick the bar of a download under way is, and the least of it that shows.
const FETCH_BAR: Pixels = px(3.);
const FETCH_LEAST: f32 = 0.02;
/// How many videos the page of new ones stops looking for more at.
const FOUND_MOST: usize = 600;
/// How far the folded player buttons slide in from when they unfold.
const SLIDE: Pixels = px(16.);
/// How much of the window's height the picture may take in theater mode.
const THEATER_SHARE: f32 = 0.78;
/// How large the picture-in-picture window opens, and how far from the screen's corner.
const PIP_SIZE: Size<Pixels> = size(px(480.), px(270.));
const PIP_MARGIN: Pixels = px(24.);
/// How many lines of a description show before it is opened.
const OPENING: usize = 3;
const STEADY: Pixels = px(0.5);
/// How often the watch page redraws its clock and scrubber while a video plays.
const TICK: Duration = Duration::from_millis(250);
/// How large a channel's avatar is among search results, against a control's height.
const CHIP_AVATAR: f32 = 1.5;
/// How wide the volume slider under the video is.
/// How far up the picture a caption sits, as a share of its height.
const CAPTION_FOOT: f32 = 0.06;
/// The frame rate above which a quality's name says it.
const SMOOTH: u32 = 30;
/// The strip of controls that floats over a full screen picture: how wide it may be, how far
/// from the foot it sits, how see-through it is and how much it blurs what is under it.
const FLOAT_WIDTH: Pixels = px(860.);
const FLOAT_FOOT: Pixels = px(20.);
/// About how tall the full screen controls stand, which captions are lifted over.
const FLOAT_HEIGHT: Pixels = px(84.);
/// How near the top, and how much of the height from the bottom, the pointer has to come for
/// the full screen controls to show.
const WAKE_HEAD: Pixels = px(56.);
const WAKE_FOOT: f32 = 0.18;
const FLOAT_FILL: f32 = 0.55;
const FLOAT_BLUR: Pixels = px(20.);
/// How long the full screen controls stay after the pointer last moved.
/// How long the transcript stays where it was scrolled by hand before following again.
const BROWSE_HOLD: Duration = Duration::from_secs(4);
const FULL_LINGER: Duration = Duration::from_secs(2);
/// How wide the time over the seek bar stands when the video has no preview pictures.
const PEEK_BARE: Pixels = px(64.);
/// How tall a channel's banner stands, how much of the page shows through its top, and how far
/// down it the channel's name starts.
const BANNER: Pixels = px(440.);
const BANNER_VEIL: f32 = 0.1;
const BANNER_CLEAR: f32 = 0.62;
/// How strongly selected text in a description is backed.
const SELECTED: f32 = 0.4;
/// How long a press on the picture is held before it hurries the video along.
const HOLD: Duration = Duration::from_millis(350);
/// The speeds a video can be played at.
const SPEEDS: [f32; 8] = [0.25, 0.5, 0.75, 1., 1.25, 1.5, 1.75, 2.];
const VOLUME_WIDTH: Pixels = px(96.);
/// How far the picture's colours spread around it, and how strongly.
const GLOW_BLUR: Pixels = px(90.);
const GLOW_REACH: Pixels = px(40.);
/// How far down the glow sits, which keeps it off the top of the page where it would be cut.
const GLOW_DROP: Pixels = px(60.);
const GLOW_STRENGTH: f32 = 0.4;
/// How much of the picture's width a caption may take, and how dark its backing is.
const CAPTION_SHARE: f32 = 0.8;
const CAPTION_BACKING: f32 = 0.7;
/// The smallest the picture-in-picture window is dragged to, and the grip that drags it.
const PIP_MIN: Size<Pixels> = size(px(240.), px(135.));
const PIP_GRIP: Pixels = px(14.);

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
    /// The video a right click opens the menu of.
    asked: Option<Video>,
    /// How much of the video is downloaded, while it is.
    fetched: Option<f32>,
}

/// The one menu a right click on any video opens, drawn once at the root of the workspace.
pub(crate) struct VideoMenu {
    kept: Entity<Subscriptions>,
    open: Option<(Video, Point<Pixels>)>,
}

struct MenuHeld(Entity<VideoMenu>);

impl gpui::Global for MenuHeld {}

impl VideoMenu {
    /// Makes the menu, which saves to watch later through `kept`.
    pub(crate) fn install(kept: Entity<Subscriptions>, cx: &mut App) -> Entity<Self> {
        let menu = cx.new(|_| Self { kept, open: None });
        cx.set_global(MenuHeld(menu.clone()));
        menu
    }

    /// The right-click handler of a video, which opens its menu where the pointer is.
    fn opener(video: Video) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static {
        move |event, _, cx| {
            let Some(held) = cx.try_global::<MenuHeld>() else {
                return;
            };
            let (menu, at) = (held.0.clone(), event.position);
            cx.stop_propagation();
            menu.update(cx, |this, cx| {
                this.open = Some((video.clone(), at));
                cx.notify();
            });
        }
    }
}

impl Render for VideoMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((video, at)) = self.open.clone() else {
            return div();
        };
        let saved = self.kept.read(cx).saved(&video.id);
        let fetched = state::VideoDownloads::progress(&video.id, cx);
        let watched = Destination::Watch(video.id.clone().into());
        let address = format!("https://www.youtube.com/watch?v={}", video.id);
        let (kept, later) = (self.kept.clone(), video.clone());
        let fetch = video.clone();

        let menu = Menu::new("video-menu")
            .name(t!("video-menu"))
            .item(
                MenuItem::new("video-menu-open", t!("video-menu-open"))
                    .icon("icons/play.svg")
                    .on_click(move |_, _, cx| navigate(watched.clone(), cx)),
            )
            .item(
                MenuItem::new(
                    "video-menu-later",
                    match saved {
                        true => t!("video-later-remove"),
                        false => t!("video-later-add"),
                    },
                )
                .icon(match saved {
                    true => "icons/check.svg",
                    false => "icons/clock.svg",
                })
                .on_click(move |_, _, cx| {
                    kept.update(cx, |kept, cx| kept.toggle_later(&later, cx));
                }),
            )
            .when_some(video.channel.clone(), |menu, channel| {
                let opened = Destination::Channel(channel.id.into());
                menu.item(
                    MenuItem::new("video-menu-channel", t!("video-menu-channel"))
                        .icon("icons/user-round.svg")
                        .on_click(move |_, _, cx| navigate(opened.clone(), cx)),
                )
            })
            .item(
                MenuItem::new("video-menu-copy", t!("video-menu-copy"))
                    .icon("icons/link.svg")
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(address.clone()));
                        Toasts::show(Outcome::Done, "toast-link-copied", cx);
                    }),
            )
            .item(MenuItem::separator("video-menu-rule"))
            .item(
                MenuItem::new(
                    "video-menu-download",
                    match fetched {
                        Some(fetched) => t!(
                            "video-menu-downloading",
                            percent = (fetched * 100.).round() as u32
                        ),
                        None => t!("video-menu-download"),
                    },
                )
                .icon("icons/download.svg")
                .on_click(move |_, _, cx| {
                    let downloads = state::VideoDownloads::entity(cx);
                    downloads.update(cx, |downloads, cx| match fetched {
                        Some(_) => downloads.cancel(&fetch.id, cx),
                        None => downloads.start(&fetch, cx),
                    });
                }),
            )
            .item(
                MenuItem::new("video-menu-folder", t!("video-menu-folder"))
                    .icon("icons/folder-open.svg")
                    .on_click(|_, _, _| {
                        let Some(folder) = state::VideoDownloads::folder() else {
                            return;
                        };
                        if let Err(error) = std::fs::create_dir_all(&folder)
                            .and_then(|()| open::that_detached(&folder))
                        {
                            log::warn!("downloads: cannot show the folder: {error:#}");
                        }
                    }),
            );

        div().child(Popup::new(at, menu).on_close(cx.listener(|this, _, _, cx| {
            this.open = None;
            cx.notify();
        })))
    }
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
        asked,
        fetched,
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
        .when_some(asked, |this, video| {
            this.on_mouse_down(MouseButton::Right, VideoMenu::opener(video))
        })
        .child(
            div()
                .relative()
                .child(picture)
                .when_some(fetched, |this, fetched| {
                    // The share downloaded, as a bar along the foot of the picture.
                    this.child(
                        div()
                            .absolute()
                            .left_0()
                            .bottom_0()
                            .h(FETCH_BAR)
                            .w(tiles.width * fetched.max(FETCH_LEAST))
                            .rounded(radius)
                            .bg(theme.primary),
                    )
                })
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
            asked: Some(video.clone()),
            fetched: state::VideoDownloads::progress(&video.id, cx),
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

/// A speed as it is written, such as `1.5x`.
fn pace(speed: f32) -> SharedString {
    format!("{speed}x").into()
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
        .on_mouse_down(MouseButton::Right, VideoMenu::opener(video.clone()))
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
            asked: None,
            fetched: None,
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
/// The element fading in and rising a little into its place, once for each `id`, unless
/// motion is switched off.
fn settle(id: impl Into<gpui::ElementId>, element: impl IntoElement, cx: &App) -> AnyElement {
    let held = div().relative().child(element);
    match ui::motion::animates(cx) {
        true => held
            .with_animation(id, Motion::Base.animation(), |this, t| {
                this.top(SLIDE * (1. - t)).opacity(t)
            })
            .into_any_element(),
        false => held.into_any_element(),
    }
}

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
    /// Whether that window opened by itself when the page was left, so coming back closes it.
    drifted: bool,
    /// Whether the whole description shows rather than its opening lines.
    described: bool,
    /// Where the menu of picture qualities hangs from, while it is open.
    choosing: Option<Point<Pixels>>,
    /// Where the menu of speeds hangs from, while it is open.
    pacing: Option<Point<Pixels>>,
    /// Where the menu of caption languages hangs, while it is open.
    wording: Option<Point<Pixels>>,
    /// Whether the buttons folded behind the "more" one are laid out, which hovering does.
    spread: bool,
    /// Scrolls the transcript, which follows what is being said.
    script_scroll: ScrollHandle,
    /// The transcript line last scrolled to.
    said: Cell<Option<usize>>,
    /// When the transcript was last scrolled by hand, which pauses the following.
    browsed: Rc<Cell<Option<Instant>>>,
    /// Whether the full screen controls are asked for, and since when, which their fade runs from.
    strip: (bool, Instant),
    /// How many times the picture went into or out of full screen, which names the fade.
    turns: usize,
    /// The wait that turns a press on the picture into a hold.
    hold: Option<Task<()>>,
    /// Whether the press being let go was a hold, so it is not also a click.
    swallow: bool,
    /// How far along the seek bar the pointer rests, which shows that moment's picture.
    peek: Option<f32>,
    /// The stretch of the description that is selected, from where the drag began to where
    /// it stands, in bytes.
    picked: Option<(usize, usize)>,
    /// Whether a drag over the description is still held.
    picking: bool,
    /// Where the description's letters were last laid out, which turns a pointer into a place
    /// in the text.
    letters: std::rc::Rc<std::cell::RefCell<gpui::TextLayout>>,
    /// Holds the keyboard while the description has a selection to copy.
    reading: gpui::FocusHandle,
    /// Whether the comments show in a panel beside the picture rather than under the page.
    discussed: bool,
    /// The field a comment is written in.
    words: Entity<Input>,
    /// The comment a reply is being written to, and the field it is written in.
    replying: Option<String>,
    answer: Entity<Input>,
    /// Whether the picture spans the page with everything else under it.
    theater: bool,
    /// Whether going full screen put the window into the system's full screen, so leaving undoes it.
    entered: bool,
    /// When the pointer last moved over the full screen picture, which keeps its controls up.
    stirred: Option<Instant>,
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
            drifted: false,
            described: false,
            choosing: None,
            pacing: None,
            wording: None,
            spread: false,
            script_scroll: ScrollHandle::new(),
            said: Cell::new(None),
            browsed: Rc::new(Cell::new(None)),
            strip: (false, Instant::now()),
            turns: 0,
            hold: None,
            swallow: false,
            peek: None,
            picked: None,
            picking: false,
            letters: Default::default(),
            reading: cx.focus_handle(),
            discussed: false,
            words: cx.new(|cx| Input::new("video-comment-placeholder", cx)),
            replying: None,
            answer: cx.new(|cx| Input::new("video-comment-reply-placeholder", cx)),
            theater: false,
            entered: false,
            stirred: None,
            _tick: tick,
        }
    }

    /// Follows the page being left or come back to. A video that plays as the page is left
    /// carries on in picture-in-picture, and coming back puts it on the page again. Says
    /// whether the video plays on without the page.
    pub(crate) fn follow(&mut self, on_page: bool, cx: &mut Context<Self>) -> bool {
        let watching = self.watching.read(cx);
        let floating = watching.floating();
        let playing = watching.player().is_some_and(|player| !player.paused());
        match on_page {
            true => {
                if floating && self.drifted {
                    self.toggle_pip(cx);
                }
                false
            }
            false if floating => true,
            false if playing && watching.id().is_some() => {
                self.toggle_pip(cx);
                self.drifted = self.watching.read(cx).floating();
                self.drifted
            }
            false => false,
        }
    }

    /// Ends a press on the picture, and the hurry a held one began.
    fn release(&mut self, cx: &mut Context<Self>) {
        self.hold = None;
        self.watching
            .update(cx, |watching, cx| watching.hurry(false, cx));
    }

    /// Moves the picture into a small window that stays above the others, or brings it back.
    fn toggle_pip(&mut self, cx: &mut Context<Self>) {
        self.drifted = false;
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
                is_resizable: true,
                is_minimizable: false,
                window_min_size: Some(PIP_MIN),
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
        let spoken = self.watching.read(cx).caption().filter(|_| !floating);
        let hurried = self.watching.read(cx).hurried();
        let buffering = self.watching.read(cx).buffering();
        // The halves of the picture light the page on their own side.
        let glow = self
            .watching
            .read(cx)
            .glow()
            .filter(|_| !full && !floating)
            .map(|(left, right)| {
                [(left, -GLOW_REACH), (right, GLOW_REACH)]
                    .map(|(colour, across)| BoxShadow {
                        color: Hsla::from(rgb(colour)).opacity(GLOW_STRENGTH),
                        offset: point(across, GLOW_DROP),
                        blur_radius: GLOW_BLUR,
                        spread_radius: Pixels::ZERO,
                        inset: false,
                    })
                    .to_vec()
            });

        div()
            .id("video-stage")
            .relative()
            .when_some(glow, |this, glow| this.shadow(glow))
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
            // A press held hurries the video along until it is let go, and is no click then.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.swallow = false;
                    this.hold = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(HOLD).await;
                        this.update(cx, |this, cx| {
                            this.swallow = true;
                            this.watching
                                .update(cx, |watching, cx| watching.hurry(true, cx));
                        })
                        .ok();
                    }));
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.release(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.release(cx)),
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                if std::mem::take(&mut this.swallow) {
                    return;
                }
                toggled.update(cx, |watching, cx| match event.click_count() {
                    2 => watching.set_full(!full, cx),
                    _ => watching.toggle(cx),
                })
            }))
            .map(|stage| match floating {
                true => stage
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(theme.text(Text::Label))
                    .text_color(theme.overlay_foreground)
                    .child(t!("video-pip-playing")),
                // Until the new video has a picture the last one's would still be up.
                false => {
                    stage.when_some(view.filter(|_| !buffering), |this, view| this.child(view))
                }
            })
            .when(hurried, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_3()
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .px_2()
                                .py_0p5()
                                .rounded(theme.radius)
                                .bg(theme.overlay)
                                .text_size(theme.text(Text::Small))
                                .text_color(theme.overlay_foreground)
                                .child(pace(state::HURRY)),
                        ),
                )
            })
            .when(buffering && !floating, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Spinner::new("video-buffering")
                                .size(theme.metrics.control)
                                .text_color(gpui::white()),
                        ),
                )
            })
            .when_some(spoken, |this, spoken| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(match full && self.strip.0 {
                            true => {
                                (room.height * CAPTION_FOOT).max(FLOAT_FOOT * 2. + FLOAT_HEIGHT)
                            }
                            false => room.height * CAPTION_FOOT,
                        })
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .id("video-caption")
                                .role(Role::Caption)
                                .aria_label(SharedString::from(spoken.clone()))
                                .max_w(room.width * CAPTION_SHARE)
                                .px_2()
                                .py_0p5()
                                .rounded(theme.radius)
                                .bg(gpui::black().opacity(CAPTION_BACKING))
                                .text_color(gpui::white())
                                .text_size(match full {
                                    true => theme.text(Text::Large),
                                    false => theme.text(Text::Body),
                                })
                                .text_center()
                                .child(SharedString::from(spoken)),
                        ),
                )
            })
    }

    /// The picture of the moment under the pointer and its time, standing over the seek bar.
    fn preview(&self, total: f64, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let theme = cx.theme();
        let fraction = self.peek?;
        let seconds = f64::from(fraction) * total;
        let peek = self.watching.read(cx).peek(seconds);
        let width = match &peek {
            Some((_, (width, _))) => px(*width as f32),
            None => PEEK_BARE,
        };
        let travel = self.seek.travel(theme.metrics.pad);
        let left = (travel * fraction - width / 2.)
            .clamp(Pixels::ZERO, (travel - width).max(Pixels::ZERO));

        Some(
            div()
                .absolute()
                .bottom_full()
                .left(left)
                .w(width)
                .mb_2()
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .when_some(peek, |this, (peek, (width, height))| {
                    let (width, height) = (px(width as f32), px(height as f32));
                    this.child(
                        div()
                            .relative()
                            .w(width)
                            .h(height)
                            .overflow_hidden()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                            .bg(gpui::black())
                            .child(
                                gpui::img(SharedString::from(peek.sheet))
                                    .absolute()
                                    .left(-width * peek.column as f32)
                                    .top(-height * peek.row as f32)
                                    .w(width * peek.columns as f32)
                                    .h(height * peek.rows as f32),
                            ),
                    )
                })
                .child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded(theme.radius)
                        .bg(theme.overlay)
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.overlay_foreground)
                        .child(clock(Duration::from_secs_f64(seconds.max(0.)))),
                ),
        )
    }

    /// The description as text that can be selected with a drag and copied, its web addresses
    /// opening and its moments seeking when pressed.
    fn words(&self, shown: String, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let marks = tube::marks(&shown);
        let picked = self
            .picked
            .map(|(from, to)| from.min(to)..from.max(to))
            .filter(|picked| !picked.is_empty() && picked.end <= shown.len());
        // Every place the look changes, so that no two stretches overlap.
        let mut cuts: Vec<usize> = marks
            .iter()
            .flat_map(|(range, _)| [range.start, range.end])
            .chain(picked.iter().flat_map(|range| [range.start, range.end]))
            .filter(|cut| shown.is_char_boundary(*cut))
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        let looks = cuts.windows(2).filter_map(|pair| {
            let (from, to) = (pair[0], pair[1]);
            let marked = marks
                .iter()
                .any(|(range, _)| range.start <= from && to <= range.end);
            let chosen = picked
                .as_ref()
                .is_some_and(|range| range.start <= from && to <= range.end);
            (marked || chosen).then(|| {
                (
                    from..to,
                    gpui::HighlightStyle {
                        color: marked.then_some(theme.progress_bar),
                        background_color: chosen.then_some(theme.selection.opacity(SELECTED)),
                        underline: marked.then(|| gpui::UnderlineStyle {
                            thickness: px(1.),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
            })
        });
        let text = gpui::StyledText::new(shown.clone()).with_highlights(looks);
        // The layout this draw makes is the one the next press is read against.
        *self.letters.borrow_mut() = text.layout().clone();
        let place = move |letters: &gpui::TextLayout, at: Point<Pixels>| match letters
            .index_for_position(at)
        {
            Ok(place) | Err(place) => place,
        };
        let copied = picked.map(|range| shown[range].to_owned());

        div()
            .id("video-description-text")
            .track_focus(&self.reading)
            .w_full()
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    let at = place(&this.letters.borrow(), event.position);
                    this.picked = Some((at, at));
                    this.picking = true;
                    window.focus(&this.reading, cx);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if !this.picking || !event.dragging() {
                    return;
                }
                let at = place(&this.letters.borrow(), event.position);
                if let Some(picked) = &mut this.picked
                    && picked.1 != at
                {
                    picked.1 = at;
                    cx.notify();
                }
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.picking = false),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                    this.picking = false;
                    // A press that never moved is a click on whatever lies under it.
                    let Some((from, to)) = this.picked else {
                        return;
                    };
                    if from != to {
                        return;
                    }
                    this.picked = None;
                    cx.notify();
                    let pressed = marks
                        .iter()
                        .find(|(range, _)| range.contains(&from))
                        .map(|(_, mark)| mark.clone());
                    match pressed {
                        Some(tube::Mark::Link(address)) => cx.open_url(&address),
                        Some(tube::Mark::Moment(seconds)) => this
                            .watching
                            .update(cx, |watching, cx| watching.seek_to(seconds, cx)),
                        None => {}
                    }
                }),
            )
            .on_key_down(move |event, _, cx| {
                let keys = &event.keystroke;
                if keys.modifiers.secondary()
                    && keys.key == "c"
                    && let Some(copied) = &copied
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                    cx.stop_propagation();
                }
            })
            .child(text)
    }

    /// The menu of speeds, hanging over the button that opened it.
    fn speeds(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let at = self.pacing?;
        let now = self.watching.read(cx).speed();
        let menu = Menu::new("video-speeds").name(t!("video-speed")).items(
            SPEEDS.into_iter().enumerate().map(|(place, speed)| {
                let chosen = self.watching.clone();
                MenuItem::new(("video-speed", place), pace(speed))
                    .checked((speed - now).abs() < f32::EPSILON)
                    .on_click(move |_, _, cx| {
                        chosen.update(cx, |watching, cx| watching.set_speed(speed, cx));
                    })
            }),
        );
        Some(
            Popup::new(at, menu)
                .above()
                .on_close(cx.listener(|this, _, _, cx| {
                    this.pacing = None;
                    cx.notify();
                })),
        )
    }

    /// The menu of caption languages, hanging over the button that opened it: the ones the
    /// video carries, then the ones YouTube translates into.
    fn wordings(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let at = self.wording?;
        let watching = self.watching.read(cx);
        let (choices, shown) = (watching.caption_choices(), watching.caption_shown());
        let pick = |place: usize, choice: &tube::Caption| {
            let (chosen, wanted) = (self.watching.clone(), choice.clone());
            MenuItem::new(("video-caption-language", place), choice.name.clone())
                .checked(shown == Some(choice))
                .on_click(move |_, _, cx| {
                    chosen.update(cx, |watching, cx| {
                        watching.set_caption_language(wanted.clone(), cx);
                    });
                })
        };
        let carried = choices.iter().filter(|choice| !choice.translated).count();
        let menu = Menu::new("video-caption-languages").name(t!("video-caption-language"));
        let menu = match choices.is_empty() {
            true => menu.item(
                MenuItem::new(
                    "video-caption-language-none",
                    match watching.transcript_loading() {
                        true => t!("video-caption-language-loading"),
                        false => t!("video-caption-language-none"),
                    },
                )
                .disabled(),
            ),
            false => menu
                .items(
                    choices
                        .iter()
                        .take(carried)
                        .enumerate()
                        .map(|(place, choice)| pick(place, choice)),
                )
                .when(carried < choices.len(), |menu| {
                    menu.item(MenuItem::separator("video-caption-divider"))
                        .item(
                            MenuItem::new(
                                "video-caption-translated",
                                t!("video-caption-translated"),
                            )
                            .disabled(),
                        )
                })
                .items(
                    choices
                        .iter()
                        .enumerate()
                        .skip(carried)
                        .map(|(place, choice)| pick(place, choice)),
                ),
        };
        Some(
            Popup::new(at, menu)
                .above()
                .on_close(cx.listener(|this, _, _, cx| {
                    this.wording = None;
                    cx.notify();
                })),
        )
    }

    /// The menu of picture qualities, hanging over the button that opened it.
    fn qualities(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let at = self.choosing?;
        let watching = self.watching.read(cx);
        let ceiling = watching.ceiling();
        let playing = watching.quality().map(|quality| quality.height);
        let pick = |id: usize, label: SharedString, to: Option<u32>| {
            let chosen = self.watching.clone();
            MenuItem::new(("video-quality", id), label)
                .checked(ceiling == to)
                .on_click(move |_, _, cx| {
                    chosen.update(cx, |watching, cx| watching.set_ceiling(to, cx));
                })
        };
        let auto = match playing {
            Some(height) => t!("video-quality-auto-at", height = height),
            None => t!("video-quality-auto"),
        };
        let menu = Menu::new("video-qualities")
            .name(t!("video-quality"))
            .item(pick(0, auto, None))
            .items(watching.qualities().iter().map(|quality| {
                let label = match quality.rate > SMOOTH {
                    true => format!("{}p{}", quality.height, quality.rate),
                    false => format!("{}p", quality.height),
                };
                pick(quality.height as usize, label.into(), Some(quality.height))
            }));
        Some(
            Popup::new(at, menu)
                .above()
                .on_close(cx.listener(|this, _, _, cx| {
                    this.choosing = None;
                    cx.notify();
                })),
        )
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
        let pad = theme.metrics.pad;
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
        let toggled = self.watching.clone();
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
        let spread = self.spread
            || self.choosing.is_some()
            || self.pacing.is_some()
            || self.wording.is_some();
        let tray = div()
            .id("video-extras")
            .flex()
            .flex_none()
            .items_center()
            .on_hover(cx.listener(|this, over: &bool, _, cx| {
                if this.spread != *over {
                    this.spread = *over;
                    cx.notify();
                }
            }))
            .map(|tray| match spread {
                true => {
                    let extras = div()
                        .relative()
                        .flex()
                        .items_center()
                        .gap_2()
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
                                    script
                                        .update(cx, |watching, cx| watching.toggle_transcript(cx));
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
                            // A right click offers the languages the captions come in.
                            div()
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                                        this.wording = Some(event.position);
                                        this.watching.update(cx, |watching, cx| {
                                            watching.want_caption_choices(cx);
                                        });
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
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
                                            captioned.update(cx, |watching, cx| {
                                                watching.toggle_captions(cx)
                                            });
                                        }),
                                ),
                        )
                        .child(
                            Button::new("video-speed")
                                .label(pace(watching.speed()))
                                .ghost()
                                .selected(self.pacing.is_some())
                                .tooltip("video-speed")
                                .on_click(cx.listener(|this, event: &gpui::ClickEvent, _, cx| {
                                    this.pacing = match this.pacing {
                                        Some(_) => None,
                                        None => Some(event.position()),
                                    };
                                    cx.notify();
                                })),
                        )
                        .when(!watching.qualities().is_empty(), |this| {
                            this.child(
                                Button::new("video-quality")
                                    .label(SharedString::from(match watching.quality() {
                                        Some(quality) => format!("{}p", quality.height),
                                        None => String::new(),
                                    }))
                                    .ghost()
                                    .selected(self.choosing.is_some())
                                    .tooltip("video-quality")
                                    .on_click(cx.listener(
                                        |this, event: &gpui::ClickEvent, _, cx| {
                                            this.choosing = match this.choosing {
                                                Some(_) => None,
                                                None => Some(event.position()),
                                            };
                                            cx.notify();
                                        },
                                    )),
                            )
                        })
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
                        });
                    match ui::motion::animates(cx) {
                        true => tray.child(extras.with_animation(
                            "video-extras-slide",
                            Motion::Quick.animation(),
                            |this, t| this.left(SLIDE * (1. - t)).opacity(t),
                        )),
                        false => tray.child(extras),
                    }
                }
                false => tray.child(
                    Button::new("video-more")
                        .icon("icons/ellipsis.svg")
                        .ghost()
                        .tooltip("video-more"),
                ),
            });
        let buttons = div()
            .flex()
            .w_full()
            .items_center()
            .gap_2()
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
                div().flex_none().w(VOLUME_WIDTH).child(
                    Scrubber::new(&self.volume, level)
                        .name(t!("video-volume"))
                        .value(percent(level))
                        .colors(theme.progress_bar, theme.muted, theme.foreground)
                        .on_move(cx.listener(|this, fraction: &f32, _, cx| {
                            let level = *fraction;
                            this.watching
                                .update(cx, |watching, cx| watching.set_volume(level, cx));
                        })),
                ),
            )
            .child(time(ends))
            .child(div().flex_1())
            .child(tray)
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
            );

        div()
            .flex()
            .flex_col()
            .w_full()
            .gap_1()
            .child(
                div()
                    .id("video-seek-row")
                    .relative()
                    .w_full()
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                        let peek = this.seek.hovered(event.position, pad).filter(|_| seekable);
                        if peek != this.peek {
                            this.peek = peek;
                            cx.notify();
                        }
                    }))
                    .on_hover(cx.listener(|this, over: &bool, _, cx| {
                        if !*over && this.peek.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .children(self.preview(total, cx))
                    .child(
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
                self.keep_up(spoken);
                div()
                    .id("video-transcript-lines")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .p_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.script_scroll)
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
                // Scrolling to an end of the panel stops there rather than moving the page.
                .on_scroll_wheel({
                    let browsed = self.browsed.clone();
                    move |_, _, cx| {
                        // Reading elsewhere in the transcript holds it there for a while.
                        browsed.set(Some(Instant::now()));
                        cx.stop_propagation();
                    }
                })
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
                .child(div().flex().flex_col().flex_1().min_h_0().child(body))
                .into_any_element(),
        )
    }

    /// Brings the line being said to the middle of the transcript when it changes, unless the
    /// transcript was scrolled by hand a moment ago.
    fn keep_up(&self, spoken: Option<usize>) {
        if self.said.replace(spoken) == spoken {
            return;
        }
        if self
            .browsed
            .get()
            .is_some_and(|browsed| browsed.elapsed() < BROWSE_HOLD)
        {
            return;
        }
        let scroll = &self.script_scroll;
        let Some(line) = spoken.and_then(|place| scroll.bounds_for_item(place)) else {
            return;
        };
        let (view, at) = (scroll.bounds(), scroll.offset());
        // The bounds are the ones laid out, before the scrolling moves them.
        let top = line.top() - view.top();
        let wanted = top - (view.size.height - line.size.height) / 2.;
        let wanted = wanted.clamp(Pixels::ZERO, scroll.max_offset().y.max(Pixels::ZERO));
        scroll.set_offset(gpui::point(at.x, -wanted));
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
                // Scrolling to an end of the panel stops there rather than moving the page.
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
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

    /// Whether the controls stand over a full screen picture: while the video is paused, a menu
    /// of theirs is open, or the pointer was lately at an edge of the screen.
    fn strip_shown(&self, cx: &Context<Self>) -> bool {
        let paused = self
            .watching
            .read(cx)
            .player()
            .is_none_or(|player| player.paused());
        paused
            || self.choosing.is_some()
            || self.pacing.is_some()
            || self.wording.is_some()
            || self
                .stirred
                .is_some_and(|stirred| stirred.elapsed() < FULL_LINGER)
    }

    /// How far the full screen controls have come in, from 0 to 1, as they rise and sink.
    fn strip_lit(&self, cx: &Context<Self>) -> f32 {
        let (shown, since) = self.strip;
        let done = match ui::motion::animates(cx) {
            true => (since.elapsed().as_secs_f32() / Motion::Base.span().as_secs_f32()).min(1.),
            false => 1.,
        };
        match shown {
            true => ui::ease_out_cubic(done),
            false => 1. - ui::ease_out_cubic(done),
        }
    }

    /// The picture over the whole window, its controls coming up while the pointer moves or the
    /// video is paused.
    fn screen(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let room = window.viewport_size();
        let lit = self.strip_lit(cx);
        let title = self
            .watching
            .read(cx)
            .watch()
            .map(|watch| SharedString::from(watch.title.clone()));
        let left = self.watching.clone();

        deferred(
            anchored().position(Point::default()).child(
                div()
                    .id("video-screen")
                    .relative()
                    .w(room.width)
                    .h(room.height)
                    .bg(gpui::black())
                    .occlude()
                    .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                        // Only a pointer brought to the top or the bottom asks for the controls.
                        let at = event.position.y;
                        if at > WAKE_HEAD && at < room.height - room.height * WAKE_FOOT {
                            return;
                        }
                        let hidden = !this
                            .stirred
                            .is_some_and(|stirred| stirred.elapsed() < FULL_LINGER);
                        this.stirred = Some(Instant::now());
                        if hidden {
                            cx.notify();
                        }
                    }))
                    .child(settle(
                        ("video-screen-in", self.turns),
                        self.stage(room, cx),
                        cx,
                    ))
                    .when(lit > 0., |this| {
                        this.child(
                            div()
                                .absolute()
                                .opacity(lit)
                                .top(-SLIDE * (1. - lit))
                                .left_0()
                                .right_0()
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_4()
                                .py_3()
                                .bg(gpui::linear_gradient(
                                    180.,
                                    gpui::linear_color_stop(
                                        theme.background.opacity(FLOAT_FILL),
                                        0.,
                                    ),
                                    gpui::linear_color_stop(theme.background.opacity(0.), 1.),
                                ))
                                .child(
                                    Button::new("video-full-back")
                                        .icon("icons/chevron-left.svg")
                                        .ghost()
                                        .tooltip("video-full-leave")
                                        .on_click(move |_, _, cx| {
                                            left.update(cx, |watching, cx| {
                                                watching.set_full(false, cx)
                                            });
                                        }),
                                )
                                .when_some(title, |this, title| {
                                    this.child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(theme.text(Text::Body))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(title),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .absolute()
                                .opacity(lit)
                                .bottom(FLOAT_FOOT - SLIDE * (1. - lit))
                                .left_0()
                                .right_0()
                                .flex()
                                .justify_center()
                                .child(
                                    div()
                                        .id("video-full-controls")
                                        .occlude()
                                        .w(FLOAT_WIDTH.min(room.width - FLOAT_FOOT * 2.))
                                        .px_4()
                                        .py_2()
                                        .rounded(theme.radius)
                                        .corner_smoothing(theme.smoothing)
                                        .border_1()
                                        .border_color(theme.border.opacity(FLOAT_FILL))
                                        .bg(theme.background.opacity(FLOAT_FILL))
                                        .backdrop_blur(FLOAT_BLUR)
                                        .child(self.controls(cx)),
                                ),
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
                    let liked = self.watching.read(cx).liked();
                    let page = self.watching.clone();
                    let like = Button::new("video-like")
                        .icon("icons/thumbs-up.svg")
                        .label(match &watch.likes {
                            Some(likes) => SharedString::from(likes.clone()),
                            None => t!("video-like"),
                        })
                        .map(|button| match liked {
                            true => button.primary(),
                            false => button.secondary(),
                        })
                        .tooltip(match liked {
                            true => "video-like-remove",
                            false => "video-like",
                        })
                        .on_click(move |_, _, cx| {
                            page.update(cx, |watching, cx| watching.toggle_like(cx));
                        });
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
                            .child(like)
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
                            .child(self.words(
                                match long && !self.described {
                                    true => opening.join("\n"),
                                    false => watch.description.clone(),
                                },
                                cx,
                            ))
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
                                            this.picked = None;
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
        let shown = replies.is_some() && !watching.replies_folded(&comment.id);
        let liked = watching.comment_liked(comment);
        let answering = self.replying.as_deref() == Some(comment.id.as_str());
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child({
                                let (page, liked_one) = (self.watching.clone(), comment.clone());
                                Button::new(SharedString::from(format!(
                                    "video-comment-like-{}",
                                    comment.id
                                )))
                                .icon("icons/thumbs-up.svg")
                                .when_some(comment.likes.clone(), |button, likes| {
                                    button.label(likes)
                                })
                                .ghost()
                                .small()
                                .tint(match liked {
                                    true => theme.progress_bar,
                                    false => theme.muted_foreground,
                                })
                                .tooltip(match liked {
                                    true => "video-like-remove",
                                    false => "video-like",
                                })
                                .on_click(move |_, _, cx| {
                                    page.update(cx, |watching, cx| {
                                        watching.toggle_comment_like(&liked_one, cx)
                                    });
                                })
                            })
                            .when(comment.reply.is_some(), |row| {
                                let id = comment.id.clone();
                                row.child(
                                    Button::new(SharedString::from(format!(
                                        "video-comment-reply-{}",
                                        comment.id
                                    )))
                                    .label(t!("video-comment-reply"))
                                    .ghost()
                                    .small()
                                    .selected(answering)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.replying =
                                            match this.replying.as_deref() == Some(id.as_str()) {
                                                true => None,
                                                false => Some(id.clone()),
                                            };
                                        if this.replying.is_some() {
                                            this.answer.update(cx, |input, cx| {
                                                input.set_text(String::new(), cx);
                                                input.focus(window, cx);
                                            });
                                        }
                                        cx.notify();
                                    })),
                                )
                            })
                            .when(comment.replies.is_some(), |row| {
                                let (watching, id) = (self.watching.clone(), comment.id.clone());
                                let label: SharedString =
                                    match (shown, comment.reply_count.as_deref()) {
                                        (true, _) => t!("video-replies-hide"),
                                        (false, Some(count)) => {
                                            t!("video-replies-count", count = count)
                                        }
                                        (false, None) => t!("video-replies"),
                                    };
                                row.child(
                                    Button::new(SharedString::from(format!(
                                        "video-replies-{}",
                                        comment.id
                                    )))
                                    .label(label)
                                    .trailing(match shown {
                                        true => "icons/chevron-up.svg",
                                        false => "icons/chevron-down.svg",
                                    })
                                    .ghost()
                                    .small()
                                    .expanded(shown)
                                    .on_click(
                                        move |_, _, cx| {
                                            watching.update(cx, |watching, cx| {
                                                watching.toggle_replies(&id, cx)
                                            });
                                        },
                                    ),
                                )
                            }),
                    )
                    .when(answering, |this| {
                        let answered = comment.clone();
                        let send = cx.listener(move |this, _: &Submit, _, cx| {
                            let words = this.answer.read(cx).text().to_owned();
                            if words.trim().is_empty() {
                                return;
                            }
                            this.watching
                                .update(cx, |watching, cx| watching.reply(&answered, &words, cx));
                            this.replying = None;
                            cx.notify();
                        });
                        let pressed = cx.entity();
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .on_action(send)
                                .child(div().flex_1().min_w_0().child(self.answer.clone()))
                                .child(
                                    Button::new("video-comment-reply-send")
                                        .label(t!("video-comment-reply"))
                                        .secondary()
                                        .on_click(move |_, window, cx| {
                                            pressed.update(cx, |_, cx| {
                                                window.dispatch_action(Box::new(Submit), cx);
                                                cx.notify();
                                            });
                                        }),
                                ),
                        )
                    })
                    .when_some(replies.filter(|_| shown), |this, replies| {
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
    /// Enter in the comment field or its button: posts what the field holds and empties it.
    fn post(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        let words = self.words.read(cx).text().to_owned();
        if words.trim().is_empty() || self.watching.read(cx).posting() {
            return;
        }
        self.watching
            .update(cx, |watching, cx| watching.post(&words, cx));
        self.words
            .update(cx, |input, cx| input.set_text(String::new(), cx));
    }

    /// The field a signed-in account writes a comment in, with the button that posts it.
    fn compose(&self, cx: &Context<Self>) -> Option<impl IntoElement + use<>> {
        let watching = self.watching.read(cx);
        if !watching.can_comment() {
            return None;
        }
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .on_action(cx.listener(Self::post))
                .child(div().flex_1().min_w_0().child(self.words.clone()))
                .child(
                    Button::new("video-comment-post")
                        .label(t!("video-comment-post"))
                        .secondary()
                        .disabled(watching.posting())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.post(&Submit, window, cx)),
                        ),
                ),
        )
    }

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
            .children(self.compose(cx))
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
        let (toggled, closed, opened) = (
            self.watching.clone(),
            self.watching.clone(),
            self.watching.clone(),
        );

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
                // The corner the window is dragged larger or smaller by.
                div()
                    .id("video-pip-grip")
                    .absolute()
                    .bottom_0()
                    .right_0()
                    .size(PIP_GRIP)
                    .cursor(gpui::CursorStyle::ResizeUpLeftDownRight)
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        cx.stop_propagation();
                        window.start_window_resize(ResizeEdge::BottomRight);
                    }),
            )
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
                        Button::new("video-pip-expand")
                            .icon("icons/maximize.svg")
                            .ghost()
                            .tooltip("video-pip-expand")
                            .on_click(move |_, window, cx| {
                                let id = opened.update(cx, |watching, cx| {
                                    watching.set_floating(false, cx);
                                    watching.id().map(str::to_owned)
                                });
                                window.remove_window();
                                if let Some(id) = id {
                                    navigate(Destination::Watch(id.into()), cx);
                                }
                                cx.activate(true);
                            }),
                    )
                    .child(
                        Button::new("video-pip-back")
                            .icon("icons/x.svg")
                            .ghost()
                            .tooltip("video-pip-close")
                            .on_click(move |_, window, cx| {
                                // Closed away from its page the video has nowhere to show, so
                                // it stops. On its page the picture goes back there.
                                let away = !matches!(
                                    router::trail(cx).read(cx).current(),
                                    Destination::Watch(_)
                                );
                                closed.update(cx, |watching, cx| {
                                    watching.set_floating(false, cx);
                                    if away {
                                        watching.set_paused(true, cx);
                                    }
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
            self.stirred = None;
            self.turns += 1;
        }
        if full {
            let shown = self.strip_shown(cx);
            if shown != self.strip.0 {
                self.strip = (shown, Instant::now());
            }
            if self.strip.1.elapsed() < Motion::Base.span() {
                window.request_animation_frame();
            }
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
                .child(match self.turns {
                    0 => self.stage(stage, cx).into_any_element(),
                    turns => settle(("video-screen-out", turns), self.stage(stage, cx), cx),
                })
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
            .children(self.qualities(cx))
            .children(self.speeds(cx))
            .children(self.wordings(cx))
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
        let banner = list
            .banner()
            .map(|banner| SharedString::from(banner.to_owned()));
        let bleed = pad + Chrome::margin(window, cx);
        let fill = cx.theme().background;
        let title = list.title().to_owned();

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
                    .child(settle(
                        SharedString::from(format!("video-list-in-{title}")),
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .gap_8()
                            .when_some(banner, |this, banner| {
                                // The banner runs to the edges of the page and fades into it,
                                // the channel's name standing on the faded part.
                                this.child(
                                    div()
                                        .absolute()
                                        .top(-pad)
                                        .left(-bleed)
                                        .right(-bleed)
                                        .h(BANNER)
                                        .child(
                                            gpui::img(banner)
                                                .size_full()
                                                .object_fit(gpui::ObjectFit::Cover),
                                        )
                                        .child(div().absolute().inset_0().bg(
                                            gpui::linear_gradient(
                                                180.,
                                                gpui::linear_color_stop(
                                                    fill.opacity(BANNER_VEIL),
                                                    0.,
                                                ),
                                                gpui::linear_color_stop(fill, 1.),
                                            ),
                                        )),
                                )
                                .child(div().h(BANNER * BANNER_CLEAR))
                            })
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
                        cx,
                    )),
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
    /// Videos from channels not followed, picked from what was watched.
    Discover,
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
            Feed::Discover => (
                "found",
                t!("video-discover"),
                t!("video-discover-empty"),
                kept.found(),
            ),
        };
        let home = self.feed == Feed::Subscriptions;
        let loading = match self.feed {
            Feed::Discover => kept.is_finding(),
            _ => home && kept.is_loading(),
        };
        let total = videos.len();
        let cards: Vec<AnyElement> = videos
            .iter()
            .enumerate()
            .map(|(place, video)| {
                video_tile(kind, video, (place, total), tiles, &self.subscriptions, cx)
            })
            .collect();
        let empty = cards.is_empty() && !loading;
        if self.feed == Feed::Discover && !loading && (1..FOUND_MOST).contains(&total) {
            // Near the foot of the page more are looked for, next to the ones found.
            let scroll = self.scrollbar.read(cx).scroll().clone();
            let left = scroll.max_offset().y + scroll.offset().y;
            if left < window.viewport_size().height {
                let further = self.subscriptions.clone();
                cx.defer(move |cx| further.update(cx, |kept, cx| kept.find_more(cx)));
            }
        }
        let cleared = self.subscriptions.clone();
        let again = self.subscriptions.clone();
        let trailing = match self.feed {
            Feed::History if !empty => Some(
                Button::new("video-history-clear")
                    .label(t!("video-history-clear"))
                    .secondary()
                    .on_click(move |_, _, cx| {
                        cleared.update(cx, |kept, cx| kept.forget_watched(cx));
                    })
                    .into_any_element(),
            ),
            Feed::Discover if !loading => Some(
                Button::new("video-discover-again")
                    .icon("icons/refresh-cw.svg")
                    .label(t!("video-discover-again"))
                    .secondary()
                    .on_click(move |_, _, cx| {
                        again.update(cx, |kept, cx| kept.find_again(cx));
                    })
                    .into_any_element(),
            ),
            _ => None,
        };

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
