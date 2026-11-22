use std::rc::Rc;
use std::time::Instant;

use ui::{
    ActiveTheme as _, Avatar, Button, Card, Deck, DraggedPin, Edge, Menu, MenuItem, Panel, Picker,
    Pin, PinKind, Pinnable as _, Popup, SNUG, Scroller, Shield, Side, Spin, Spot, Text, Vacancy,
    drop_gap, drop_into, drop_marker,
};

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, DragMoveEvent, Entity, FontWeight, MouseButton, MouseDownEvent,
    Pixels, Point, Render, ScrollHandle, SharedString, svg,
};
use gpui::{Window, div, px};
use music::Shape;
use router::{Destination, NavEntry, Navigation, NavigationEvent, navigate, open_settings};
use state::{
    AppSettings, Leaving, Library, Origin, PinSort, Pins, Playback, Session, Shelf, Sonora,
    Subscriptions,
};

use super::title_bar::Busy;
use i18n::t;
use input::Refresh;

use crate::shared::menus::{ItemMenu, item_menu};
use crate::shared::nav::{entry_shown, nav_icon, nav_row};
use crate::shared::pins::leaving;
use crate::shared::playlist_editor::{Edit, PlaylistEditor};

/// The one drag list the pinned section keeps, so a pin dropped anywhere in it lands in order.
const PINS: &str = "sidebar-pins";

/// The sidebar's navigation rows in order: the entry that can hide each, its icon and where it goes.
/// Settings also lives in the account menu of the title bar and shows here only when added. It
/// opens on the settings page last visited rather than on the one named here.
pub(crate) const NAV: [(NavEntry, &str, Destination); 5] = [
    (NavEntry::Home, "icons/house.svg", Destination::Home),
    (NavEntry::Search, "icons/search.svg", Destination::Search),
    (
        NavEntry::Discover,
        "icons/compass.svg",
        Destination::Discover,
    ),
    (NavEntry::Later, "icons/clock.svg", Destination::Later),
    (
        NavEntry::History,
        "icons/rotate-ccw-clock.svg",
        Destination::History,
    ),
];

const MIN_WIDTH: Pixels = px(160.);
const MAX_WIDTH: Pixels = px(400.);
const HINT_HEIGHT: Pixels = px(42.);
const VACANCY_HEIGHT: Pixels = px(88.);
/// How far the pin mark on a library row falls back from the accent.
const PIN_MARK: f32 = 0.7;
/// The space between two pinned entries, the same as the `gap_1` between the rows above them.
const ROW_GAP: Pixels = px(4.);
/// The rail's side padding in rem, matching its `px_2`.
const RAIL_INSET: f32 = 0.5;
const BORDER: Pixels = px(1.);
/// The side padding of a navigation row, which the library heading lines up with. It matches
/// the regular `ui::Button`.
const ROW_INSET: Pixels = px(12.);

pub(crate) struct SidebarLeft {
    settings: Entity<AppSettings>,
    trail: Entity<Navigation>,
    width: Pixels,
    open: bool,
    /// Whether the listener folded the sidebar down to the rail.
    rail: bool,
    /// Whether the listener spread the rail out into rows with their names.
    wide: bool,
    /// The width a fold or an unfold set out from and when, which the width eases away from.
    slide: Option<(Pixels, Instant)>,
    rail_width: Pixels,
    /// Whether the window leaves too little room for the full width, so only the rail fits.
    narrow: bool,
    cramped: bool,
    forced: Option<bool>,
    /// The width and fold an edge drag started from, which a drag that closes the sidebar
    /// puts back so it reopens the way it was.
    grabbed: Option<(Pixels, bool)>,
    /// Whether an edge drag is held below half the rail, which closes the sidebar on release.
    closing: bool,
    pinned_open: bool,
    dropping: bool,
    drop_gap: Option<usize>,
    /// The playlist a dragged song is held over, which takes the song instead of a new pin.
    drop_into: Option<Pin>,
    playback: Entity<Playback>,
    pins: Entity<Pins>,
    library: Entity<Library>,
    session: Entity<Session>,
    subscriptions: Entity<Subscriptions>,
    track_menu: ItemMenu,
    context_menu: Option<(Pin, Point<Pixels>)>,
    scrollbar: Entity<ui::Scrollbar>,
    popovers: ui::Popovers,
    /// Set by a refresh and dropped once the page has its data, so the refresh button turns
    /// for exactly that long.
    busy: Option<Busy>,
    spin: Spin,
}

impl SidebarLeft {
    pub fn new(subscriptions: Entity<Subscriptions>, cx: &mut Context<Self>) -> Self {
        cx.observe(&subscriptions, |_, _, cx| cx.notify()).detach();
        let settings = Sonora::global(cx).settings.clone();
        let playback = Sonora::global(cx).playback.clone();
        let pins = Sonora::global(cx).pins.clone();
        let library = Sonora::global(cx).library.clone();
        let session = Sonora::global(cx).session.clone();
        cx.observe(&session, |_, _, cx| cx.notify()).detach();
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        cx.observe(&pins, |_, _, cx| cx.notify()).detach();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        let chrome = super::Chrome::entity(cx);
        cx.observe(&chrome, |_, _, cx| cx.notify()).detach();
        let me = cx.entity_id();
        let playlist_scrollbar = cx.new(|_| ui::Scrollbar::inset().watching(me));
        let scrollbar = cx.new(|_| {
            ui::Scrollbar::new(ScrollHandle::new())
                .passive()
                .watching(me)
        });
        let width = sidebar_width(settings.read(cx));
        let open = settings.read(cx).sidebar_open();
        let rail = settings.read(cx).sidebar_rail();
        let wide = settings.read(cx).sidebar_wide();
        let pinned_open = settings.read(cx).sidebar_pinned_open();
        let trail = router::trail(cx);

        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&trail, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&trail, |this, _, _: &NavigationEvent, cx| {
            this.dismiss(cx);
            cx.notify();
        })
        .detach();

        Self {
            settings,
            trail,
            width,
            open,
            rail,
            wide,
            slide: None,
            rail_width: Pixels::ZERO,
            narrow: false,
            forced: None,
            cramped: false,
            grabbed: None,
            closing: false,
            pinned_open,
            dropping: false,
            drop_gap: None,
            drop_into: None,
            playback,
            pins,
            library,
            session,
            subscriptions,
            track_menu: ItemMenu::new(playlist_scrollbar, cx),
            context_menu: None,
            scrollbar,
            popovers: ui::Popovers::default(),
            busy: None,
            spin: Spin::default(),
        }
    }

    fn dismiss_menu(&mut self, cx: &mut Context<Self>) {
        self.track_menu.reset(cx);
        self.context_menu = None;
        cx.notify();
    }

    fn menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (pin, position) = self.context_menu.clone()?;
        let menu = item_menu(&pin, &self.track_menu, self.playback.clone(), cx);

        Some(
            Popup::new(position, menu)
                .on_close(cx.listener(|this, _, _, cx| this.dismiss_menu(cx))),
        )
    }

    /// The rail holds the only way around the app, so it is always there.
    pub fn is_open(&self) -> bool {
        true
    }

    pub fn overlays(&self) -> bool {
        false
    }

    /// Whether the window has folded a full sidebar onto the rail by itself. An overlaid one
    /// floats over the content, so it keeps the listener's own width.
    fn squeezed(&self) -> bool {
        self.narrow && !self.cramped
    }

    /// Whether the sidebar is drawn as the rail, by choice or because the window is narrow.
    fn folded(&self) -> bool {
        !self.wide || self.squeezed()
    }

    /// Spreads the rail out into rows with their names, or folds it back to the icons.
    fn set_wide(&mut self, wide: bool, cx: &mut Context<Self>) {
        self.slide = ui::motion::animates(cx).then(|| (self.breadth(), Instant::now()));
        self.wide = wide;
        self.settings
            .update(cx, |settings, cx| settings.set_sidebar_wide(wide, cx));
        cx.notify();
        cx.refresh_windows();
    }

    fn breadth(&self) -> Pixels {
        let to = match (self.closing, self.folded()) {
            (true, _) => Pixels::ZERO,
            (false, true) => self.rail_width,
            (false, false) => self.width,
        };
        match self.sliding() {
            Some((from, done)) => from + (to - from) * ui::ease_in_out_cubic(done),
            None => to,
        }
    }

    /// The width a fold or an unfold set out from and how much of the way it has come, while
    /// it is under way.
    fn sliding(&self) -> Option<(Pixels, f32)> {
        let (from, since) = self.slide?;
        let done = since.elapsed().as_secs_f32() / ui::Motion::Base.span().as_secs_f32();
        (done < 1.).then_some((from, done))
    }

    pub fn overlay_width(&self) -> Pixels {
        match self.overlays() {
            true => self.breadth(),
            false => Pixels::ZERO,
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if !self.overlays() {
            return;
        }
        self.forced = Some(false);
        cx.notify();
    }

    pub fn occupied_width(&self) -> Pixels {
        match self.is_open() && !self.overlays() {
            true => self.breadth(),
            false => Pixels::ZERO,
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        match self.cramped {
            true => self.forced = Some(!self.is_open()),
            false => {
                self.open = !self.open;
                self.persist(cx);
            }
        }
        cx.notify();
    }

    fn ceiling(&self, window: &Window, cx: &Context<Self>) -> Pixels {
        let reserved = match self.overlays() {
            true => Pixels::ZERO,
            // Held to the room that keeps it unfolded, so a drag never widens it onto the rail.
            false => SNUG + super::Chrome::reserved_right(cx),
        };

        super::cap(MIN_WIDTH, MAX_WIDTH, reserved, window)
    }

    /// Steps the sidebar down as the window narrows, from the room left once `right` pixels
    /// are held back for the right sidebar: a full sidebar folds onto the rail once it would
    /// leave the content less than `SNUG`, and either one goes cramped when even the rail
    /// leaves less than that. This runs inside a render, where a notify schedules nothing,
    /// so a flip asks for a full window refresh too. That effect lands once the draw is over.
    pub fn adapt(&mut self, right: Pixels, window: &Window, cx: &mut Context<Self>) {
        self.width = ui::snapped(self.width, window);
        self.rail_width = rail_width(window, cx);

        let room = window.viewport_size().width - right;
        let narrow = room - self.width < SNUG;
        let cramped = room - self.rail_width < SNUG;
        if narrow == self.narrow && cramped == self.cramped {
            return;
        }
        if cramped != self.cramped {
            self.forced = None;
        }
        self.narrow = narrow;
        self.cramped = cramped;
        cx.notify();
        cx.refresh_windows();
    }

    /// Follows a drag of the edge. Below half the rail the sidebar shrinks to nothing but keeps
    /// its edge, so the drag can still bring it back, and it closes once the drag lets go.
    /// Otherwise it folds to the rail below halfway between the rail and `MIN_WIDTH` and
    /// unfolds above it, except while the window holds the sidebar on the rail.
    fn resize(&mut self, width: Pixels, cx: &mut Context<Self>) {
        if self.folded() {
            return;
        }
        // Dragged most of the way to the rail it folds onto it. It never closes, since it holds
        // the only way around the app.
        match width < (self.rail_width + MIN_WIDTH) / 2. {
            true => self.set_wide(false, cx),
            false => {
                self.width = width.max(MIN_WIDTH);
                self.persist(cx);
                cx.notify();
            }
        }
    }

    /// Ends an edge drag. One let go of below half the rail closes the sidebar with the width
    /// and fold the drag started from, so the toggle reopens it as it was. Runs inside a render,
    /// so it asks for a window refresh for the title bar's toggle to follow.
    fn settle(&mut self, cx: &mut Context<Self>) {
        let grabbed = self.grabbed.take();
        if !std::mem::take(&mut self.closing) {
            return;
        }
        if let Some((width, rail)) = grabbed {
            self.width = width;
            self.rail = rail;
        }
        match self.cramped {
            true => self.forced = Some(false),
            false => self.open = false,
        }
        self.persist(cx);
        cx.refresh_windows();
    }

    /// The navigation entries the listener has not hidden, on the rail and in the full sidebar alike.
    fn navigation(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let stations = self.session.read(cx).capabilities().stations;
        let shown = |range: std::ops::Range<usize>, cx: &App| -> Vec<usize> {
            range
                .filter(|index| entry_shown(NAV[*index].0, self.settings.read(cx)))
                .filter(|index| stations || NAV[*index].0 != NavEntry::Radio)
                .collect()
        };
        let top = shown(0..NAV.len(), cx);
        let library = shown(NAV.len()..NAV.len(), cx);
        let mut rows = Vec::new();
        if self.folded() {
            for (group, indices) in [top, library]
                .into_iter()
                .filter(|indices| !indices.is_empty())
                .enumerate()
            {
                if group > 0 {
                    rows.push(self.divider(cx));
                }
                for index in indices {
                    rows.push(self.nav(index, cx));
                }
            }
            return rows;
        }
        for index in top {
            rows.push(self.nav(index, cx));
        }
        if !library.is_empty() {
            rows.push(self.section("sidebar-library", cx));
            for index in library {
                rows.push(self.nav(index, cx));
            }
            rows.push(self.gap(cx));
        }
        rows
    }

    /// A heading over a group of entries, set like a settings group's heading. It starts where
    /// the glyphs of the rows below it start.
    fn section(&self, key: &'static str, cx: &App) -> AnyElement {
        let theme = cx.theme();

        div()
            .flex()
            .flex_none()
            .items_center()
            .h(theme.metrics.control_small)
            .pl(ROW_INSET)
            .mt(theme.metrics.pad)
            .child(ui::eyebrow(i18n::lookup(key, None), cx).font_weight(FontWeight::MEDIUM))
            .into_any_element()
    }

    /// The space between the library rows and the pinned section under them.
    fn gap(&self, cx: &App) -> AnyElement {
        div().h(cx.theme().metrics.pad).into_any_element()
    }

    /// A rule between two groups on the rail, where there is no room for a heading.
    fn divider(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();

        div()
            .h(BORDER)
            .mx_2()
            .my_1()
            .bg(theme.sidebar_border)
            .into_any_element()
    }

    /// The channels the viewer follows, each opening its page: a heading and a row apiece in the
    /// full sidebar, the avatars alone under a rule on the rail.
    fn channels(&self, cx: &App) -> Vec<AnyElement> {
        let theme = *cx.theme();
        let accent = theme.sidebar_accent;
        let followed = self.subscriptions.read(cx).channels();
        if followed.is_empty() {
            return Vec::new();
        }
        let current = self.trail.read(cx).current();
        let folded = self.folded();
        let total = followed.len();
        let lead = match folded {
            true => self.divider(cx),
            false => self.section("video-feed-channels", cx),
        };
        let rows = followed.iter().enumerate().map(|(place, channel)| {
            let destination = Destination::Channel(channel.id.clone().into());
            let active = destination == current;
            let name = SharedString::from(channel.name.clone());
            let avatar = Avatar::new(channel.avatar.clone()).size(theme.metrics.control_small);
            let row = Button::new(SharedString::from(format!(
                "sidebar-channel-{}",
                channel.id
            )))
            .ghost()
            .leading(avatar)
            .name(name.clone())
            .tint(match active {
                true => theme.foreground,
                false => theme.muted_foreground,
            })
            .lit(active, accent)
            .active(move |style| style.bg(accent))
            .when(active, |button| button.aria_selected(true))
            .aria_position_in_set(place + 1)
            .aria_size_of_set(total)
            .on_click(move |_, _, cx| navigate(destination.clone(), cx));
            match folded {
                true => row.w_full(),
                false => row.label(name).gap_2p5().justify_start(),
            }
            .into_any_element()
        });
        std::iter::once(lead).chain(rows).collect()
    }

    /// Turns the refresh button until `busy` says the reloaded page has its data.
    pub fn refreshing(&mut self, busy: Busy, cx: &mut Context<Self>) {
        self.busy = Some(busy);
        cx.notify();
    }

    /// One button of the rail outside the page list.
    fn tool(id: &'static str, icon: &'static str, key: &'static str, cx: &App) -> Button {
        let theme = cx.theme();
        nav_icon(id, key, icon, theme.muted_foreground, theme.sidebar_accent)
    }

    /// A rail button sized for a row of them, which is how the spread sidebar lays them out.
    fn fit(&self, button: Button, cx: &App) -> Button {
        match self.folded() {
            true => button.flex_1().min_w_0().px_0(),
            false => button.w(cx.theme().metrics.control).flex_none(),
        }
    }

    /// Back, forward and refresh, at the head of the rail.
    fn history(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if self.busy.as_ref().is_some_and(|busy| !busy(cx)) {
            self.busy = None;
        }
        match self.busy.is_some() {
            true => self.spin.start(),
            false => self.spin.stop(),
        }
        let turn = self.spin.turn(window, cx);
        let trail = self.trail.read(cx);
        let (can_back, can_forward) = (trail.can_go_back(), trail.can_go_forward());
        let refreshes = trail.current().refreshes();
        let (back, forward) = (self.trail.clone(), self.trail.clone());
        let folded = self.folded();
        // A window too narrow for the rows keeps the rail whatever was chosen.
        let fixed = self.squeezed();
        let spread = Self::tool(
            "sidebar-spread",
            match folded {
                true => "icons/panel-left-open.svg",
                false => "icons/panel-left-close.svg",
            },
            match folded {
                true => "sidebar-expand",
                false => "sidebar-collapse",
            },
            cx,
        )
        .disabled(fixed)
        .on_click(cx.listener(|this, _, _, cx| this.set_wide(!this.wide, cx)));
        let spread = match folded {
            true => spread,
            false => self.fit(spread, cx),
        };
        let tools = div()
            .id("sidebar-history")
            .role(Role::Toolbar)
            .aria_label(i18n::lookup("nav-trail", None))
            .flex()
            .flex_none()
            .gap_1()
            .map(|tools| match folded {
                true => tools.flex_col(),
                false => tools.flex_row(),
            });

        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_1()
            .px_2()
            .child(
                tools
                    .child(spread)
                    .when(!folded, |tools| tools.child(div().flex_1()))
                    .child(
                        // Side by side on the rail too, under the fold button.
                        div()
                            .flex()
                            .flex_row()
                            .when(folded, |row| row.w_full())
                            .when(!folded, |row| row.flex_none().gap_1())
                            .child(
                                self.fit(
                                    Self::tool(
                                        "history-back",
                                        "icons/chevron-left.svg",
                                        "nav-back",
                                        cx,
                                    ),
                                    cx,
                                )
                                .disabled(!can_back)
                                .on_click(move |_, _, cx| {
                                    back.update(cx, |trail, cx| trail.back(cx))
                                }),
                            )
                            .child(
                                self.fit(
                                    Self::tool(
                                        "history-forward",
                                        "icons/chevron-right.svg",
                                        "nav-forward",
                                        cx,
                                    ),
                                    cx,
                                )
                                .disabled(!can_forward)
                                .on_click(move |_, _, cx| {
                                    forward.update(cx, |trail, cx| trail.forward(cx))
                                }),
                            )
                            .child(
                                self.fit(
                                    Self::tool(
                                        "history-refresh",
                                        "icons/refresh-cw.svg",
                                        "app-refresh",
                                        cx,
                                    ),
                                    cx,
                                )
                                .turned(turn)
                                .disabled(!refreshes)
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(Refresh), cx)
                                }),
                            ),
                    ),
            )
            .child(self.divider(cx))
            .into_any_element()
    }

    /// The account menu and settings, at the foot of the rail.
    fn foot(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut items: Vec<MenuItem> = Vec::new();
        if let Some(account) = state::VideoAccount::entity(cx) {
            let held = account.read(cx);
            items.push(match held.signed_in() {
                true => MenuItem::new("account-video-out", t!("account-video-sign-out"))
                    .icon("icons/log-out.svg")
                    .on_click(move |_, _, cx| {
                        account.update(cx, |account, cx| account.sign_out(cx));
                    }),
                false => {
                    let idle = held.supported() && !held.signing_in();
                    let item = MenuItem::new("account-video-in", t!("account-video-sign-in"))
                        .icon("icons/log-in.svg")
                        .on_click(move |_, _, cx| {
                            let title = t!("login-choose-title", provider = "YouTube").to_string();
                            account.update(cx, |account, cx| account.sign_in(title, cx));
                        });
                    match idle {
                        true => item,
                        false => item.disabled(),
                    }
                }
            });
        }
        let settings = self.trail.read(cx).current().in_settings();
        let accent = cx.theme().sidebar_accent;

        let folded = self.folded();
        let buttons = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .map(|buttons| match folded {
                true => buttons.flex_col(),
                false => buttons.flex_row(),
            });

        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_1()
            .px_2()
            .pb_3()
            .child(self.divider(cx))
            .child(
                buttons
                    .child(
                        Picker::icon("account", &self.popovers, "icons/circle-user-round.svg")
                            .tooltip("tool-account")
                            .width(Picker::REGULAR)
                            .left()
                            .menu(Menu::new("account-menu").w(Picker::REGULAR).beside(ROW_GAP))
                            .items(items),
                    )
                    .when(!folded, |buttons| buttons.child(div().flex_1()))
                    .child(
                        self.fit(
                            Self::tool(
                                "sidebar-settings",
                                "icons/settings.svg",
                                "nav-settings",
                                cx,
                            ),
                            cx,
                        )
                        .lit(settings, accent)
                        .on_click(|_, _, cx| open_settings(cx)),
                    ),
            )
            .into_any_element()
    }

    /// The pinned section: its header, and its entries once it is expanded.
    #[allow(dead_code)]
    fn pins(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if !self.settings.read(cx).nav_shown(NavEntry::Pins.id()) {
            return Vec::new();
        }

        let header = self.pins_header(cx);
        if !self.pinned_open {
            return vec![header];
        }

        std::iter::once(header)
            .chain(self.pin_entries(window, cx))
            .collect()
    }

    /// The pinned entries, as a `Deck` so only the ones on screen are ever built, or what stands
    /// in for them while there are none.
    fn pin_entries(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pins = self.pins.read(cx);
        let pinned = pins.entries(cx);
        let rest = match self.settings.read(cx).sidebar_full_library() {
            true => pins.library(cx),
            false => Rc::new(Vec::new()),
        };
        if pinned.is_empty() && rest.is_empty() {
            return (!self.folded()).then(|| match self.dropping {
                true => hint(cx),
                false => vacancy(),
            });
        }

        let held = pinned.len();
        let count = held + rest.len();
        let row = ui::snapped(cx.theme().metrics.list_row, window);
        Some(
            Deck::new("sidebar-pins-deck")
                .rows((0..count).map(|_| row))
                .gap(ROW_GAP)
                .draw(cx.processor(move |this, index: usize, _, cx| {
                    let pin = match index < held {
                        true => pinned.get(index),
                        false => rest.get(index - held),
                    };
                    match pin {
                        Some(pin) => this.pin_row(index, pin.clone(), held, count, cx),
                        None => div().into_any_element(),
                    }
                }))
                .into_any_element(),
        )
    }

    fn flip_pins(&mut self, cx: &mut Context<Self>) {
        self.pinned_open = !self.pinned_open;
        let open = self.pinned_open;
        self.settings.update(cx, |settings, cx| {
            settings.set_sidebar_pinned_open(open, cx)
        });
        cx.notify();
    }

    fn pins_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let open = self.pinned_open;
        let folded = self.folded();
        let toggle = match folded {
            true => nav_icon(
                "sidebar-pins-toggle",
                "nav-pinned",
                "icons/pin.svg",
                match open {
                    true => theme.foreground,
                    false => theme.muted_foreground,
                },
                theme.sidebar_accent,
            )
            .aria_expanded(open)
            .on_click(cx.listener(|this, _, _, cx| this.flip_pins(cx)))
            .into_any_element(),
            false => self.pins_toggle(cx),
        };

        div()
            .id("sidebar-pins-header")
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .w_full()
            .min_w_0()
            .when(!folded, |header| {
                header
                    .h(theme.metrics.control_small)
                    .pl_2()
                    // Narrower on the right than on the left, so the sort glyph sits where a
                    // trailing glyph of the rows above would.
                    .pr_1()
            })
            .mt(theme.metrics.pad)
            .mb(px(2.))
            .child(toggle)
            .when(open && !folded, |header| header.child(self.sort_picker(cx)))
            // Dragging over the header aims at the top of the list, which is otherwise
            // out of reach above the first entry.
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<DraggedPin>, _, cx| {
                    if !event.bounds.contains(&event.event.position) || this.drop_gap == Some(0) {
                        return;
                    }
                    this.drop_gap = Some(0);
                    this.drop_into = None;
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn pins_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let open = self.pinned_open;

        div()
            .id("sidebar-pins-toggle")
            .role(Role::Button)
            .aria_label(i18n::lookup("nav-pinned", None))
            .aria_expanded(open)
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .child(
                svg()
                    .path(icons::path(chevron(open)))
                    .flex_none()
                    .size(theme.text(Text::Small))
                    .text_color(theme.muted_foreground),
            )
            .child(
                ui::eyebrow(i18n::lookup("nav-pinned", None), cx)
                    .min_w_0()
                    .truncate(),
            )
            .on_click(cx.listener(|this, _, _, cx| this.flip_pins(cx)))
            .into_any_element()
    }

    fn sort_picker(&self, cx: &App) -> AnyElement {
        let chosen = self.pins.read(cx).sort(cx);
        let reversed = self.pins.read(cx).reversed(cx);
        let full = self.settings.read(cx).sidebar_full_library();
        let arrow = match reversed {
            true => "icons/chevron-down.svg",
            false => "icons/chevron-up.svg",
        };
        let pins = self.pins.clone();

        Picker::icon(
            "sidebar-pins-sort",
            &self.popovers,
            "icons/arrow-up-down.svg",
        )
        .tooltip("tool-sort")
        .sticky()
        .width(Picker::NARROW)
        .tint(match self.pins.read(cx).sorted(cx) {
            true => cx.theme().primary,
            false => cx.theme().muted_foreground,
        })
        .items(PinSort::ALL.into_iter().map(move |sort| {
            let pins = pins.clone();

            MenuItem::new(sort.id(), i18n::lookup(sort.key(), None))
                .selected(Some(sort) == chosen)
                .when(Some(sort) == chosen, |item| item.icon(arrow))
                .on_click(move |_, _, cx| {
                    pins.update(cx, |pins, cx| pins.choose(sort, cx));
                })
        }))
        .item(MenuItem::separator("sidebar-pins-scope"))
        .item(
            MenuItem::new(
                "sidebar-full-library",
                i18n::lookup("nav-show-full-library", None),
            )
            .checked(full)
            .on_click({
                let settings = self.settings.clone();
                move |_, _, cx| {
                    settings.update(cx, |settings, cx| {
                        settings.set_sidebar_full_library(!full, cx)
                    });
                }
            }),
        )
        .into_any_element()
    }

    /// One entry. The first `count` of them are the pins, which carry the drop slots. The rest
    /// is the library underneath, which can be dragged up into the pins but holds no slot.
    fn pin_row(
        &self,
        index: usize,
        pin: Pin,
        count: usize,
        total: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.theme();
        let accent = theme.sidebar_accent;
        let destination = Destination::from(&pin);
        let active = destination == self.trail.read(cx).current();
        let opened = pin.clone();
        let target = pin.clone();
        let aimed = self.drop_into.as_ref() == Some(&pin);
        let held = index < count;
        // Only worth marking when the library sits alongside, since otherwise every row is a pin.
        let marked = held && self.settings.read(cx).sidebar_full_library();
        let edge = match self.drop_gap {
            Some(gap) if held && gap == index => Some(Edge::Above),
            Some(gap) if gap == count && index + 1 == count => Some(Edge::Below),
            _ => None,
        };

        let origin = Origin::from(&pin);
        let playing = self.playback.read(cx).engaged(&origin, cx);
        let warmed = origin.clone();

        let key = format!("{}-{}", pin.kind.key(), pin.id);
        let card = Card::new(SharedString::from(format!("pinned-{key}")), pin.label())
            .aria_description(i18n::lookup(pin.kind.key(), None))
            .cover(pin.cover.clone())
            .fallback(pin.kind.icon())
            .when(pin.kind.round(), Card::circle)
            .when(pin.kind == PinKind::Liked, Card::accent)
            .when(self.folded(), Card::glance)
            .play(
                playing,
                cx.listener(move |this, _, _, cx| {
                    this.playback
                        .update(cx, |playback, cx| playback.toggle_origin(&origin, cx));
                }),
            )
            .warm({
                let playback = self.playback.clone();
                move |_, cx| playback.update(cx, |playback, cx| playback.warm_origin(&warmed, cx))
            })
            .tint(match active {
                true => theme.foreground,
                false => theme.muted_foreground,
            })
            .meta(caption(pin.kind.key(), marked, cx))
            .lit(active && !aimed, accent)
            .chosen(aimed)
            .press(move |_, _, cx| navigate(destination.clone(), cx))
            .menu(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.track_menu.reset(cx);
                this.context_menu = Some((opened.clone(), event.position));
                cx.notify();
            }))
            .when_else(
                held,
                |card| card.pin_from(pin.clone(), Spot::new(PINS, index)),
                |card| card.pin(pin.clone()),
            )
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<DraggedPin>, _, cx| {
                    let Some(gap) = drop_gap(event.bounds, event.event.position, index) else {
                        // A fast move can skip the edges, so the row lets go of its aim itself.
                        if this.drop_into.as_ref() == Some(&target) {
                            this.drop_into = None;
                            cx.notify();
                        }
                        return;
                    };
                    let into = (drop_into(event.bounds, event.event.position)
                        && this.accepts(&event.drag(cx).pin, &target, cx))
                    .then(|| target.clone());
                    let gap = match (&into, held) {
                        (Some(_), _) => None,
                        (None, true) => Some(gap),
                        (None, false) => Some(count),
                    };
                    if this.drop_gap != gap || this.drop_into != into {
                        this.drop_gap = gap;
                        this.drop_into = into;
                        cx.notify();
                    }
                }),
            );

        div()
            .id(SharedString::from(format!("pinned-slot-{key}")))
            .role(Role::ListItem)
            .aria_position_in_set(index + 1)
            .aria_size_of_set(total)
            .relative()
            .flex_none()
            .w_full()
            .min_w_0()
            .child(card)
            .when_some(edge, |this, edge| this.child(drop_marker(edge, cx)))
            .into_any_element()
    }

    /// One navigation entry, which navigates and is lit while its page is current.
    fn nav(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let accent = theme.sidebar_accent;
        let (entry, icon, destination) = NAV[index].clone();
        let key = entry.key();
        let current = self.trail.read(cx).current();
        let active = destination.same_section(&current);
        let tint = match active {
            true => theme.foreground,
            false => theme.muted_foreground,
        };
        let row = match self.folded() {
            true => nav_icon(index, key, icon, tint, accent),
            false => nav_row(index, key, tint, accent).icon(icon),
        };

        row.lit(active, accent)
            .when(active, |button| button.aria_selected(true))
            .on_click(move |_, _, cx| match destination.in_settings() {
                true => open_settings(cx),
                false => navigate(destination.clone(), cx),
            })
            .into_any_element()
    }

    /// Whether `target` takes the dragged `song`: a playlist the listener may edit, on the same
    /// shelf as the song, since a provider adds only its own tracks.
    fn accepts(&self, song: &Pin, target: &Pin, cx: &App) -> bool {
        song.kind == PinKind::Song
            && target.kind == PinKind::Playlist
            && Shelf::of(&song.id) == Shelf::of(&target.id)
            && self.library.read(cx).is_editable(&target.id)
    }

    /// Adds a dropped song to a playlist, and asks first when the playlist holds the song already.
    /// Shift held on the drop moves the song instead.
    fn add_into(&self, target: &Pin, song: &Pin, window: &mut Window, cx: &mut Context<Self>) {
        let library = self.library.read(cx);
        let Some(playlist) = library.playlist(&target.id).cloned() else {
            return;
        };
        let track = song.id.clone();
        let leaving = window
            .modifiers()
            .shift
            .then(|| self.leaves(song, &playlist.id, cx))
            .flatten();
        match library.holds(&playlist.id, &track) == Some(true) {
            true => PlaylistEditor::open(
                Edit::Again {
                    playlist,
                    track,
                    leaving,
                },
                window,
                cx,
            ),
            false => self.library.update(cx, |library, cx| match leaving {
                Some(leaving) => library.move_to_playlist(playlist.id, track, leaving, cx),
                None => library.add_to_playlist(playlist.id, track, cx),
            }),
        }
    }

    /// What a moved song leaves. The favorites count only on a `Shape::Saved` shelf, since a
    /// catalog page lists every song.
    fn leaves(&self, song: &Pin, target: &str, cx: &App) -> Option<Leaving> {
        let library = self.library.read(cx);
        match leaving(song.origin.as_deref()?) {
            Leaving::Liked => {
                (library.shape(Shelf::Streaming) == Shape::Saved).then_some(Leaving::Liked)
            }
            Leaving::Playlist(id) => {
                (id != target && library.is_editable(&id)).then_some(Leaving::Playlist(id))
            }
        }
    }

    fn persist(&self, cx: &mut Context<Self>) {
        let width = self.width / px(1.);
        let open = self.open;
        let rail = self.rail;
        self.settings.update(cx, |settings, cx| {
            settings.set_sidebar(width, open, rail, cx)
        });
    }
}

impl Render for SidebarLeft {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let sidebar_bg = theme.sidebar;
        let sidebar_border = theme.sidebar_border;

        self.adapt(super::Chrome::reserved_right(cx), window, cx);
        match self.sliding() {
            // The workspace lays the page out beside this width, so the whole window follows.
            Some(_) => window.refresh(),
            None => self.slide = None,
        }

        if !cx.has_active_drag() {
            self.dropping = false;
            self.drop_gap = None;
            self.drop_into = None;
            self.settle(cx);
        }
        let closing = self.closing;

        let pages = self.navigation(cx);
        let rows = self.channels(cx);
        let history = self.history(window, cx);
        let foot = self.foot(cx);

        let overlaid = self.overlays();
        let rail = self.folded();
        // The edge drags past the rail to zero, so a drag can close the sidebar.
        let panel = Panel::new("sidebar-left", Side::Left, self.breadth())
            .limits(Pixels::ZERO, MAX_WIDTH)
            .reach(self.ceiling(window, cx))
            .grip_above(super::Chrome::bottom(cx))
            .on_resize(cx.listener(|this, width: &Pixels, _, cx| this.resize(*width, cx)))
            .on_drag_move(cx.listener(|this, _: &DragMoveEvent<DraggedPin>, _, cx| {
                if this.dropping {
                    return;
                }
                this.dropping = true;
                // A drag has nowhere to land while the section is closed, so open it.
                if !this.pinned_open {
                    this.pinned_open = true;
                    this.settings.update(cx, |settings, cx| {
                        settings.set_sidebar_pinned_open(true, cx)
                    });
                }
                cx.notify();
            }))
            .on_drop(cx.listener(|this, dragged: &DraggedPin, window, cx| {
                let gap = this.drop_gap.take();
                this.dropping = false;
                match this.drop_into.take() {
                    Some(playlist) => this.add_into(&playlist, &dragged.pin, window, cx),
                    None => {
                        let pin = dragged.pin.clone().origin(None);
                        this.pins.update(cx, |pins, cx| pins.place(pin, gap, cx));
                    }
                }
                cx.notify();
            }))
            .when(!self.is_open(), |this| this.hidden())
            .when(!theme.transparent, |this| this.bg(sidebar_bg))
            .border_color(sidebar_border)
            // A sidebar dragged shut keeps only its edge until the drag lets go.
            .when(closing, |this| this.border_r_0())
            .when(overlaid, |this| {
                this.occlude().absolute().left_0().top_0().bottom_0()
            })
            .child(div().flex_none().pt_1().child(history))
            // The pages stay where they are: only the channels under them scroll.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap_1()
                    .pb_1()
                    .when(closing, |this| this.hidden())
                    .map(|column| match rail {
                        true => column.px_2(),
                        false => column.px_3(),
                    })
                    .children(pages),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .when(closing, |this| this.hidden())
                    .child(
                        Scroller::new("sidebar-left-rows", &self.scrollbar)
                            .bottom_inset(super::Chrome::bottom(cx))
                            .size_full()
                            .child(
                                div()
                                    .id("sidebar-navigation")
                                    .role(Role::Navigation)
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .w_full()
                                    .pb_3()
                                    .map(|column| match rail {
                                        true => column.px_2(),
                                        false => column.px_3(),
                                    })
                                    // Leave the last row somewhere to go, clear of the
                                    // button that floats over the bottom.
                                    .pb(ui::perch_room(cx))
                                    .children(rows),
                            ),
                    )
                    .children(
                        ui::return_top("sidebar-return-top", &self.scrollbar, cx).map(|perch| {
                            match super::Chrome::bottom(cx) > Pixels::ZERO {
                                true => perch
                                    .bottom(super::Chrome::bottom(cx) + theme.metrics.pad * 1.5),
                                false => perch,
                            }
                        }),
                    ),
            )
            .child(foot)
            .children(self.menu(cx));

        match overlaid {
            false => panel.into_any_element(),
            true => div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .child(
                    Shield::new("sidebar-shield")
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| this.dismiss(cx)),
                        ),
                )
                .child(panel)
                .into_any_element(),
        }
    }
}

/// How wide the left sidebar stands from its saved width, which the settings rail matches.
pub(crate) fn sidebar_width(settings: &AppSettings) -> Pixels {
    px(settings.sidebar_width()).clamp(MIN_WIDTH, MAX_WIDTH)
}

/// One pinned row's width plus the rail's padding and border.
fn rail_width(window: &Window, cx: &App) -> Pixels {
    let row = cx.theme().metrics.list_row;
    ui::snapped(row + window.rem_size() * (RAIL_INSET * 2.) + BORDER, window)
}

/// The kind of an entry, behind a dimmed pin when it is one of the pinned ones.
fn caption(kind: &'static str, marked: bool, cx: &App) -> impl IntoElement {
    let theme = *cx.theme();

    div()
        .flex()
        .items_center()
        .gap(theme.metrics.pad / 4.)
        .min_w_0()
        .when(marked, |row| {
            row.child(
                svg()
                    .path(icons::path("icons/pin.svg"))
                    .flex_none()
                    .size(theme.text(Text::Tiny))
                    .text_color(theme.primary.opacity(PIN_MARK)),
            )
        })
        .child(div().min_w_0().truncate().child(i18n::lookup(kind, None)))
}

fn hint(cx: &App) -> AnyElement {
    let theme = *cx.theme();

    div()
        .flex()
        .flex_none()
        .min_w_0()
        .items_center()
        .justify_center()
        // A narrow sidebar wraps the line, so the box grows rather than the text spilling out.
        .min_h(HINT_HEIGHT)
        .mx_2()
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .corner_smoothing(theme.smoothing)
        .border_1()
        .border_dashed()
        .border_color(theme.sidebar_border)
        .text_size(theme.text(Text::Small))
        .text_color(theme.muted_foreground)
        .text_center()
        .child(ui::spoken("sidebar-pin-hint", i18n::lookup("nav-pin-hint", None)).min_w_0())
        .into_any_element()
}

fn vacancy() -> AnyElement {
    Vacancy::new(i18n::lookup("nav-nothing-pinned", None))
        .spoken("sidebar-nothing-pinned")
        .icon("icons/pin.svg")
        .compact()
        .flex_none()
        .h(VACANCY_HEIGHT)
        .px_2()
        .into_any_element()
}

fn chevron(open: bool) -> &'static str {
    match open {
        true => "icons/chevron-down.svg",
        false => "icons/chevron-right.svg",
    }
}
