mod link;
mod navigation;

pub use link::Link;
pub use navigation::{Navigation, NavigationEvent};

use gpui::{App, AppContext as _, Entity, Global, SharedString};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryTab {
    Songs,
    Albums,
    Playlists,
    Artists,
}

impl LibraryTab {
    /// The glyph of what the tab lists, the same one a pin of that kind falls back to.
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Songs => ui::PinKind::Song.icon(),
            Self::Albums => ui::PinKind::Album.icon(),
            Self::Playlists => ui::PinKind::Playlist.icon(),
            Self::Artists => ui::PinKind::Artist.icon(),
        }
    }
}

/// A row the sidebar can hide from Settings. The four library pages are rows of their own,
/// each opening its page of the merged streaming and local library. Radio follows them, and only
/// for a provider that lists stations. History and Settings always sit in the account menu, so
/// their rows are optional and stay out of the sidebar until asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavEntry {
    Home,
    Search,
    Songs,
    Albums,
    Artists,
    Playlists,
    Radio,
    History,
    /// The videos saved to watch later.
    Later,
    /// Videos from channels not followed yet, picked from what was watched.
    Discover,
    /// The Shorts gathered from the followed channels and a search.
    Shorts,
    Settings,
    Pins,
}

impl NavEntry {
    pub const ALL: [Self; 13] = [
        Self::Home,
        Self::Search,
        Self::Shorts,
        Self::Discover,
        Self::Later,
        Self::History,
        Self::Settings,
        Self::Songs,
        Self::Albums,
        Self::Artists,
        Self::Playlists,
        Self::Radio,
        Self::Pins,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Search => "search",
            Self::Songs => "songs",
            Self::Albums => "albums",
            Self::Artists => "artists",
            Self::Playlists => "playlists",
            Self::Radio => "radio",
            Self::History => "history",
            Self::Later => "later",
            Self::Discover => "discover",
            Self::Shorts => "shorts",
            Self::Settings => "settings",
            Self::Pins => "sidebar-pins",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Home => "nav-home",
            Self::Search => "nav-search",
            Self::Songs => "nav-songs",
            Self::Albums => "nav-albums",
            Self::Artists => "nav-artists",
            Self::Playlists => "nav-playlists",
            Self::Radio => "nav-radio",
            Self::History => "nav-history",
            Self::Later => "nav-later",
            Self::Discover => "nav-discover",
            Self::Shorts => "nav-shorts",
            Self::Settings => "nav-settings",
            Self::Pins => "nav-pinned",
        }
    }

    /// Whether the row stays out of the sidebar until the listener adds it, rather than showing
    /// until hidden.
    pub fn optional(self) -> bool {
        matches!(self, Self::Settings)
    }
}

/// A screen the app can open on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Search,
    Later,
    History,
    Discover,
}

impl Screen {
    pub const ALL: [Self; 5] = [
        Self::Home,
        Self::Search,
        Self::Discover,
        Self::Later,
        Self::History,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Search => "search",
            Self::Later => "later",
            Self::History => "history",
            Self::Discover => "discover",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Home => "nav-home",
            Self::Search => "nav-search",
            Self::Later => "nav-later",
            Self::History => "nav-history",
            Self::Discover => "nav-discover",
        }
    }

    /// The screen a stored id names, or `None` for one that no longer exists.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|screen| screen.id() == id)
    }

    pub fn destination(self) -> Destination {
        match self {
            Self::Home => Destination::Home,
            Self::Search => Destination::Search,
            Self::Later => Destination::Later,
            Self::History => Destination::History,
            Self::Discover => Destination::Discover,
        }
    }
}

/// One page of settings. The first fourteen are sections of the six tabs, each tab showing its sections
/// one after another on one scrolling page, and the last two are drill-ins that open from a row
/// on their parent page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPage {
    Providers,
    General,
    Appearance,
    PlayerBar,
    NowPlaying,
    Lyrics,
    Playback,
    Sound,
    Storage,
    Discord,
    Scrobbling,
    About,
    Keybinds,
    Profiles,
    LyricsSources,
    MusicFolders,
}

impl SettingsPage {
    /// Every page, the sections first and the drill-ins after them.
    pub const ALL: [Self; 16] = [
        Self::Providers,
        Self::General,
        Self::Appearance,
        Self::PlayerBar,
        Self::NowPlaying,
        Self::Lyrics,
        Self::Playback,
        Self::Sound,
        Self::Storage,
        Self::Discord,
        Self::Scrobbling,
        Self::About,
        Self::Keybinds,
        Self::Profiles,
        Self::LyricsSources,
        Self::MusicFolders,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Providers => "providers",
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::PlayerBar => "player-bar",
            Self::NowPlaying => "now-playing",
            Self::Lyrics => "lyrics",
            Self::Playback => "playback",
            Self::Sound => "sound",
            Self::Storage => "storage",
            Self::Discord => "discord",
            Self::Scrobbling => "scrobbling",
            Self::About => "about",
            Self::Keybinds => "keybinds",
            Self::Profiles => "profiles",
            Self::LyricsSources => "lyrics-sources",
            Self::MusicFolders => "music-folders",
        }
    }

    /// The Fluent key of the page's name.
    pub fn key(self) -> &'static str {
        match self {
            Self::Providers => "settings-page-providers",
            Self::General => "settings-page-general",
            Self::Appearance => "settings-page-appearance",
            Self::PlayerBar => "settings-page-player-bar",
            Self::NowPlaying => "settings-page-now-playing",
            Self::Lyrics => "settings-page-lyrics",
            Self::Playback => "settings-page-playback",
            Self::Sound => "settings-page-sound",
            Self::Storage => "settings-page-storage",
            Self::Discord => "settings-page-discord",
            Self::Scrobbling => "settings-page-scrobbling",
            Self::About => "settings-page-about",
            Self::Keybinds => "settings-page-keybinds",
            Self::Profiles => "settings-page-profiles",
            Self::LyricsSources => "settings-lyrics-sources",
            Self::MusicFolders => "settings-music-folders",
        }
    }

    /// The page a drill-in returns to, or `None` for a section of a tab.
    pub fn parent(self) -> Option<Self> {
        match self {
            Self::LyricsSources => Some(Self::Lyrics),
            Self::MusicFolders => Some(Self::Providers),
            _ => None,
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|page| page.id() == id)
    }

    /// The tab the page shows in. A drill-in belongs to its parent's tab.
    pub fn tab(self) -> SettingsTab {
        match self {
            Self::Providers | Self::MusicFolders | Self::General | Self::About => {
                SettingsTab::General
            }
            Self::Appearance | Self::PlayerBar | Self::NowPlaying => SettingsTab::Appearance,
            Self::Lyrics | Self::LyricsSources => SettingsTab::Lyrics,
            Self::Playback | Self::Sound | Self::Storage => SettingsTab::Playback,
            Self::Discord | Self::Scrobbling => SettingsTab::Integrations,
            Self::Keybinds => SettingsTab::Keybinds,
            Self::Profiles => SettingsTab::Profiles,
        }
    }
}

/// One tab of the settings header. A tab is one scrolling page of its sections, each a former
/// page under its own title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Appearance,
    Lyrics,
    Playback,
    Integrations,
    Keybinds,
    Profiles,
}

impl SettingsTab {
    /// Every tab, in the order the header shows them.
    pub const ALL: [Self; 5] = [
        Self::General,
        Self::Profiles,
        Self::Appearance,
        Self::Playback,
        Self::Keybinds,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Lyrics => "lyrics",
            Self::Playback => "playback",
            Self::Integrations => "integrations",
            Self::Keybinds => "keybinds",
            Self::Profiles => "profiles",
        }
    }

    /// The Fluent key of the tab's name.
    pub fn key(self) -> &'static str {
        match self {
            Self::General => "settings-page-general",
            Self::Appearance => "settings-page-appearance",
            Self::Lyrics => "settings-page-lyrics",
            Self::Playback => "settings-page-playback",
            Self::Integrations => "settings-tab-integrations",
            Self::Keybinds => "settings-page-keybinds",
            Self::Profiles => "settings-page-profiles",
        }
    }

    /// The icon before the tab's name in the header, and beside a search result found in it.
    pub fn icon(self) -> &'static str {
        match self {
            Self::General => "icons/settings.svg",
            Self::Appearance => "icons/palette.svg",
            Self::Lyrics => "icons/mic-vocal.svg",
            Self::Playback => "icons/play.svg",
            Self::Integrations => "icons/link.svg",
            Self::Keybinds => "icons/keyboard.svg",
            Self::Profiles => "icons/circle-user-round.svg",
        }
    }

    /// The pages the tab shows, top to bottom. The first is the one the tab opens on.
    pub fn sections(self) -> &'static [SettingsPage] {
        match self {
            Self::General => &[SettingsPage::General, SettingsPage::About],
            Self::Appearance => &[SettingsPage::Appearance],
            Self::Lyrics => &[SettingsPage::Lyrics],
            Self::Playback => &[SettingsPage::Playback],
            Self::Integrations => &[SettingsPage::Discord, SettingsPage::Scrobbling],
            Self::Keybinds => &[SettingsPage::Keybinds],
            Self::Profiles => &[SettingsPage::Profiles],
        }
    }

    /// The page the tab opens on, at its top.
    pub fn lead(self) -> SettingsPage {
        self.sections()[0]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Home,
    History,
    /// The videos saved to watch later.
    Later,
    /// Videos from channels not followed yet, picked from what was watched.
    Discover,
    /// The Shorts gathered from the followed channels and a search.
    Shorts,
    Library(LibraryTab),
    Album(SharedString),
    Song(SharedString),
    Playlist(SharedString),
    Artist(SharedString),
    User(SharedString),
    Genre(SharedString),
    /// The internet radio stations of a provider that lists them.
    Radio,
    /// A video, by its YouTube id.
    Watch(SharedString),
    /// A Short, by its YouTube id, in the upright player.
    Short(SharedString),
    /// A YouTube channel's uploads.
    Channel(SharedString),
    /// A YouTube playlist's videos.
    Videos(SharedString),
    Search,
    Settings(SettingsPage),
    /// The equalizer's own settings page, opened from Sound.
    Equalizer,
    Fullscreen,
}

impl From<&ui::Pin> for Destination {
    fn from(pin: &ui::Pin) -> Self {
        let id = SharedString::from(pin.id.clone());
        match pin.kind {
            ui::PinKind::Album => Destination::Album(id),
            ui::PinKind::Artist => Destination::Artist(id),
            ui::PinKind::Playlist => Destination::Playlist(id),
            ui::PinKind::Song => Destination::Song(id),
            ui::PinKind::Liked => Destination::Library(LibraryTab::Songs),
        }
    }
}

impl Destination {
    /// The YouTube video shown by a watch or Shorts page.
    pub fn video_id(&self) -> Option<&str> {
        match self {
            Self::Watch(id) | Self::Short(id) => Some(id.as_ref()),
            _ => None,
        }
    }

    /// Whether the sidebar row leading to `self` is lit while `other` is open. Every settings page
    /// lights the one Settings row, and every other row is lit only on its own page.
    pub fn same_section(&self, other: &Destination) -> bool {
        (self.in_settings() && other.in_settings()) || self == other
    }

    /// Whether this destination is a settings page, the equalizer included.
    pub fn in_settings(&self) -> bool {
        matches!(self, Destination::Settings(_) | Destination::Equalizer)
    }

    /// The settings page a drill-in's back button returns to. Only drill-ins have one.
    pub fn parent(&self) -> Option<Destination> {
        match self {
            Destination::Equalizer => Some(Destination::Settings(SettingsPage::Sound)),
            Destination::Settings(page) => page.parent().map(Destination::Settings),
            _ => None,
        }
    }

    /// The tabbed page this destination belongs to: the page itself, or a drill-in's parent.
    pub fn settings_page(&self) -> Option<SettingsPage> {
        match self {
            Destination::Equalizer => Some(SettingsPage::Sound),
            Destination::Settings(page) => Some(page.parent().unwrap_or(*page)),
            _ => None,
        }
    }

    /// Whether this page shows provider data that Refresh can load again. Settings, search and
    /// the fullscreen player have nothing to reload, so they get no refresh button.
    pub fn refreshes(&self) -> bool {
        !matches!(
            self,
            Destination::Search
                | Destination::Settings(_)
                | Destination::Equalizer
                | Destination::Fullscreen
        )
    }
}

#[derive(Clone)]
struct Router(Entity<Navigation>);

impl Global for Router {}

pub fn init(start: Destination, cx: &mut App) {
    let navigation = cx.new(|_| Navigation::new(start));
    cx.set_global(Router(navigation));
}

pub fn trail(cx: &App) -> Entity<Navigation> {
    cx.global::<Router>().0.clone()
}

pub fn navigate(destination: Destination, cx: &mut App) {
    trail(cx).update(cx, |navigation, cx| navigation.go(destination, cx));
}

/// Replaces the current destination so Back and Forward do not revisit a page that disappeared.
pub fn replace(destination: Destination, cx: &mut App) {
    trail(cx).update(cx, |navigation, cx| navigation.replace(destination, cx));
}

pub fn back(cx: &mut App) {
    trail(cx).update(cx, |navigation, cx| navigation.back(cx));
}

pub fn forward(cx: &mut App) {
    trail(cx).update(cx, |navigation, cx| navigation.forward(cx));
}

/// Steps up from a drill-in to `parent`, going back when it is the page before so the history
/// holds no loop, and replacing the drill-in otherwise.
pub fn up(parent: Destination, cx: &mut App) {
    trail(cx).update(cx, |navigation, cx| navigation.up(parent, cx));
}

/// Opens settings on the page they last showed, or on General the first time.
pub fn open_settings(cx: &mut App) {
    trail(cx).update(cx, |navigation, cx| navigation.open_settings(cx));
}
