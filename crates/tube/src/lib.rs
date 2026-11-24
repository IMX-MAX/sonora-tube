//! YouTube's own data, read anonymously from the InnerTube endpoints the website calls.
//!
//! Only what a watching app needs is modelled: search, the watch page, comments, channels and
//! playlists. Ads and promoted shelves are dropped while parsing and never reach a caller, and
//! Shorts come back as ordinary videos unless the client is told to leave them out.

use std::time::Duration;

pub mod ai;
mod client;
mod parse;

pub use client::{Client, PROOF, ShortStreams};

/// A video as a list shows it. The counts and dates stay the text YouTube wrote, since that is
/// already abbreviated and localised.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: Option<ChannelRef>,
    /// The clock text of its length, such as `10:35`. A live stream has none.
    pub length: Option<String>,
    pub views: Option<String>,
    pub published: Option<String>,
    /// Whether YouTube lists it as a Short. It plays like any other video.
    #[serde(default)]
    pub short: bool,
    /// A line or two of its description, which only search results carry.
    #[serde(default)]
    pub snippet: Option<String>,
    /// The picture of the channel that made it, where the listing has one.
    #[serde(default)]
    pub avatar: Option<String>,
    /// The upright picture a Short is listed with.
    #[serde(default)]
    pub still: Option<String>,
}

impl Video {
    /// The 16:9 still YouTube keeps for every video.
    pub fn thumbnail(&self) -> String {
        format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", self.id)
    }

    /// The upright picture of a Short, or its first frame when it was listed without one.
    pub fn upright(&self) -> String {
        self.still
            .clone()
            .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{}/frame0.jpg", self.id))
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
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChannelRef {
    pub id: String,
    pub name: String,
}

/// A channel as a search result or a watch page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub subscribers: Option<String>,
}

/// One of the YouTube channels a Google account can act as: its own, or a brand account's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    /// Which Google account of the session the channel belongs to, counted from the first.
    pub user: usize,
    /// What a request names to act as this channel, none for the account's own channel.
    pub page: Option<String>,
    pub name: String,
    /// The handle or the address YouTube shows under the name.
    pub detail: Option<String>,
    pub avatar: Option<String>,
    /// Whether YouTube itself has this one as the channel in use.
    pub selected: bool,
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
    /// The other channels a video made together is credited to, after `channel`.
    pub partners: Vec<Channel>,
    /// The tokens that fetch the comments best first and newest first.
    pub top: Option<String>,
    pub newest: Option<String>,
    pub related: Vec<Video>,
    /// The token of the first page of comments, absent when comments are turned off.
    pub comments: Option<String>,
    /// Whether the signed-in account liked the video. `None` without an account.
    pub liked: Option<bool>,
}

/// One picture quality of a video: the address of its picture and of the sound that goes
/// with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Quality {
    /// The height of the picture in pixels, which is what a quality is named by.
    pub height: u32,
    /// Frames a second, rounded.
    pub rate: u32,
    pub picture: String,
    pub sound: String,
}

/// Playback metadata and the result of loading YouTube's direct streams.
#[derive(Debug)]
pub struct Streams {
    /// Direct stream qualities, best first, or why they could not be loaded.
    pub qualities: anyhow::Result<Vec<Quality>>,
    pub board: Option<Storyboard>,
    /// YouTube's published running time, absent for live broadcasts.
    pub duration: Option<Duration>,
}

/// Sheets of small pictures taken through a video at even steps, which a seek bar shows the
/// moment under the pointer from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Storyboard {
    /// The address of a sheet, split around where its number goes.
    pub(crate) address: (String, String),
    /// The size of one picture in pixels.
    pub width: u32,
    pub height: u32,
    /// How many pictures there are in all, and how many across and down a full sheet.
    pub count: u32,
    pub columns: u32,
    pub rows: u32,
    /// The seconds between two pictures, zero when they are spread evenly over the video.
    pub interval: f64,
}

/// Where the picture of one moment sits: the sheet's address, the column and row on it, and how
/// many columns and rows that sheet has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peek {
    pub sheet: String,
    pub column: u32,
    pub row: u32,
    pub columns: u32,
    pub rows: u32,
}

impl Storyboard {
    /// The picture nearest `seconds` into a video `length` seconds long.
    pub fn at(&self, seconds: f64, length: f64) -> Option<Peek> {
        let per = self.columns * self.rows;
        if per == 0 || self.count == 0 {
            return None;
        }
        let step = match self.interval > 0. {
            true => self.interval,
            false => length / f64::from(self.count),
        };
        if step <= 0. {
            return None;
        }
        let frame = ((seconds.max(0.) / step) as u32).min(self.count - 1);
        let (sheet, place) = (frame / per, frame % per);
        // The last sheet holds what is left over, so it may be shorter or narrower.
        let held = (self.count - sheet * per).min(per);
        Some(Peek {
            sheet: format!("{}{sheet}{}", self.address.0, self.address.1),
            column: place % self.columns,
            row: place / self.columns,
            columns: held.min(self.columns),
            rows: held.div_ceil(self.columns),
        })
    }
}

/// Something in a description that can be pressed.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    /// A web address.
    Link(String),
    /// A moment of the video, in seconds.
    Moment(f64),
}

/// The web addresses and the moments such as `1:23` written in `text`, each with the bytes it
/// covers, in order.
pub fn marks(text: &str) -> Vec<(std::ops::Range<usize>, Mark)> {
    let mut found = Vec::new();
    let mut start = 0;
    for word in text.split_inclusive(char::is_whitespace) {
        let at = start;
        start += word.len();
        let word = word.trim_end();
        if word.starts_with("https://") || word.starts_with("http://") {
            // A full stop or a bracket closing the sentence is not part of the address.
            let address = word.trim_end_matches(['.', ',', ')', ']', '!', '?', ';', ':']);
            found.push((at..at + address.len(), Mark::Link(address.to_owned())));
            continue;
        }
        // A stretch such as `6:21–7:07` holds a moment on each side of its dash.
        let mut from = at;
        for part in word.split_inclusive(['-', '–', '—']) {
            let here = from;
            from += part.len();
            let opened = part.trim_start_matches(['(', '[']);
            let moment = opened.trim_end_matches(['.', ',', ')', ']', ':', ';', '-', '–', '—']);
            let digits =
                |piece: &str| !piece.is_empty() && piece.bytes().all(|b| b.is_ascii_digit());
            let pieces: Vec<&str> = moment.split(':').collect();
            if !(2..=3).contains(&pieces.len())
                || !pieces.iter().all(|piece| digits(piece) && piece.len() <= 2)
                || pieces[1..].iter().any(|piece| piece.len() != 2)
            {
                continue;
            }
            let seconds = pieces.iter().fold(0., |total, piece| {
                total * 60. + piece.parse::<f64>().unwrap_or(0.)
            });
            let lead = part.len() - opened.len();
            found.push((
                here + lead..here + lead + moment.len(),
                Mark::Moment(seconds),
            ));
        }
    }
    found
}

/// A named stretch of a video, from the second it starts to where the next one does.
#[derive(Clone, Debug, PartialEq)]
pub struct Chapter {
    pub start: f64,
    pub title: String,
}

/// The chapters a description lists as lines that open with a moment, when there are at least
/// three and the first is at the start, as YouTube itself requires.
pub fn chapters(description: &str) -> Vec<Chapter> {
    let mut found = Vec::new();
    for line in description.lines() {
        let line = line.trim();
        let Some((range, Mark::Moment(start))) = marks(line).into_iter().next() else {
            continue;
        };
        if range.start != 0 {
            continue;
        }
        let title = line[range.end..]
            .trim_start_matches(|letter: char| {
                letter.is_whitespace() || matches!(letter, '-' | '–' | '—' | ':' | '|' | ')')
            })
            .trim();
        if title.is_empty() {
            continue;
        }
        found.push(Chapter {
            start,
            title: title.to_owned(),
        });
    }
    let ordered = found.windows(2).all(|pair| pair[0].start < pair[1].start);
    match found.first() {
        Some(first) if first.start == 0. && found.len() >= 3 && ordered => found,
        _ => Vec::new(),
    }
}

/// One line of a video's transcript and the second it is spoken at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub start: f64,
    /// How many seconds it stays on screen as a caption.
    pub length: f64,
    pub text: String,
}

/// A language a video's captions can be read in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caption {
    pub code: String,
    pub name: String,
    /// Whether YouTube translates into it by machine, rather than the video carrying it.
    pub translated: bool,
}

/// The lines of a video's captions and the languages they could be in instead.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Captions {
    pub lines: Vec<Line>,
    /// The language the lines are in, when the video has captions.
    pub shown: Option<Caption>,
    pub choices: Vec<Caption>,
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
    /// Whether the signed-in account liked it.
    pub liked: bool,
    /// What liking it, taking the like back and replying to it each send to YouTube. Present
    /// only for a signed-in account.
    pub like: Option<String>,
    pub unlike: Option<String>,
    pub reply: Option<String>,
}

/// A page of comments, with the total the first page announces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Comments {
    pub page: Page<Comment>,
    pub total: Option<String>,
    /// What posting a comment under the video has to send back, present only when the
    /// signed-in account may write one.
    pub write: Option<String>,
}

/// A channel's own page: who it is and what it uploaded, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelPage {
    pub channel: Channel,
    /// The wide picture across the top of the channel.
    pub banner: Option<String>,
    pub description: String,
    pub videos: Page<Video>,
    /// The parts of the channel that can be opened, in the order YouTube shows them.
    pub tabs: Vec<Tab>,
}

/// A part of a channel's page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Videos,
    Shorts,
    Playlists,
    Posts,
    Releases,
}

/// Something a channel wrote to its followers: words, pictures, or a video it points at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Post {
    pub id: String,
    pub text: String,
    pub published: Option<String>,
    pub likes: Option<String>,
    pub pictures: Vec<String>,
    pub video: Option<Video>,
}

/// What one part of a channel other than its videos lists. Releases come as playlists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Section {
    /// The Shorts of the part that lists them.
    pub videos: Page<Video>,
    pub playlists: Page<Playlist>,
    pub posts: Page<Post>,
}

/// A playlist's own page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlaylistPage {
    pub id: String,
    pub title: String,
    pub videos: Page<Video>,
}

impl Video {
    /// Roughly how many seconds ago the video was published, read from wording such as
    /// "3 days ago". `None` when the wording is missing or not understood.
    pub fn age(&self) -> Option<u64> {
        let mut words = self.published.as_deref()?.split_whitespace();
        let count = words.find_map(|word| word.parse::<u64>().ok())?;
        let unit = match words.next()?.trim_end_matches('s') {
            "second" => 1,
            "minute" => 60,
            "hour" => 3_600,
            "day" => 86_400,
            "week" => 604_800,
            "month" => 2_592_000,
            "year" => 31_536_000,
            _ => return None,
        };
        Some(count * unit)
    }
}
