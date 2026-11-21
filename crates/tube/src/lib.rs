//! YouTube's own data, read anonymously from the InnerTube endpoints the website calls.
//!
//! Only what a watching app needs is modelled: search, the watch page, comments, channels and
//! playlists. Shorts, ads and promoted shelves are dropped while parsing and never reach a caller.

mod client;
mod parse;

pub use client::Client;

/// A video as a list shows it. The counts and dates stay the text YouTube wrote, since that is
/// already abbreviated and localised.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: Option<ChannelRef>,
    /// The clock text of its length, such as `10:35`. A live stream has none.
    pub length: Option<String>,
    pub views: Option<String>,
    pub published: Option<String>,
}

impl Video {
    /// The 16:9 still YouTube keeps for every video.
    pub fn thumbnail(&self) -> String {
        format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", self.id)
    }

    /// The page mpv is pointed at to play it.
    pub fn url(&self) -> String {
        watch_url(&self.id)
    }
}

/// The address of a video's watch page.
pub fn watch_url(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// The channel a video or a comment belongs to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelRef {
    pub id: String,
    pub name: String,
}

/// A channel as a search result or a watch page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub subscribers: Option<String>,
}

/// A playlist as a search result shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub thumbnail: Option<String>,
    /// The line under the title, usually the owner and the number of videos.
    pub detail: Option<String>,
}

/// One thing a search or a channel page lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Video(Video),
    Channel(Channel),
    Playlist(Playlist),
}

/// A stretch of a longer list, and the token that fetches the stretch after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub more: Option<String>,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            more: None,
        }
    }
}

/// Everything the watch page shows around the player.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    pub id: String,
    pub title: String,
    pub views: Option<String>,
    pub published: Option<String>,
    pub likes: Option<String>,
    pub description: String,
    pub channel: Option<Channel>,
    pub related: Vec<Video>,
    /// The token of the first page of comments, absent when comments are turned off.
    pub comments: Option<String>,
}

/// A comment or a reply under a video.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub author: ChannelRef,
    pub avatar: Option<String>,
    pub text: String,
    pub published: Option<String>,
    pub likes: Option<String>,
    /// Whether the video's own channel wrote it.
    pub creator: bool,
    pub reply_count: Option<String>,
    /// The token of its replies, when it has any.
    pub replies: Option<String>,
}

/// A page of comments, with the total the first page announces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comments {
    pub page: Page<Comment>,
    pub total: Option<String>,
}

/// A channel's own page: who it is and what it uploaded, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelPage {
    pub channel: Channel,
    pub description: String,
    pub videos: Page<Video>,
}

/// A playlist's own page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlaylistPage {
    pub id: String,
    pub title: String,
    pub videos: Page<Video>,
}
