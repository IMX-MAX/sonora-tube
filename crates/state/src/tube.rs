//! YouTube as the video screens see it: one entity each for the search, the video being watched
//! and the channel or playlist that is open.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyView, App, AppContext as _, Context, Entity, Global, Task};
use tube::{Channel, Client, Comment, Found, Line, Page, Video, Watch};
use video::{Player, VideoView};

use crate::{Io, Playback, join};

/// A search of YouTube and the results fetched so far.
pub struct VideoSearch {
    client: Client,
    io: Io,
    query: String,
    found: Vec<Found>,
    more: Option<String>,
    loading: bool,
    error: Option<String>,
    task: Option<Task<()>>,
}

impl VideoSearch {
    pub fn new(client: Client, io: Io) -> Self {
        Self {
            client,
            io,
            query: String::new(),
            found: Vec::new(),
            more: None,
            loading: false,
            error: None,
            task: None,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn found(&self) -> &[Found] {
        &self.found
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Whether another stretch of results can be asked for.
    pub fn has_more(&self) -> bool {
        self.more.is_some()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Replaces the results with those of `query`. A blank query empties the page.
    pub fn search(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.trim();
        self.query = query.to_owned();
        self.found.clear();
        self.more = None;
        self.error = None;
        self.task = None;
        self.loading = !query.is_empty();
        cx.notify();
        if query.is_empty() {
            return;
        }
        let (client, query) = (self.client.clone(), query.to_owned());
        let request = self.io.spawn(async move { client.search(&query).await });
        self.land(request, cx);
    }

    /// Runs the current query again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.query.clone();
        self.search(&query, cx);
    }

    /// Fetches the stretch after the last one, unless one is already on its way.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(token) = self.more.take() else {
            return;
        };
        self.loading = true;
        cx.notify();
        let client = self.client.clone();
        let request = self
            .io
            .spawn(async move { client.search_more(&token).await });
        self.land(request, cx);
    }

    fn land(
        &mut self,
        request: tokio::task::JoinHandle<anyhow::Result<Page<Found>>>,
        cx: &mut Context<Self>,
    ) {
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match crate::settled(loaded, cx) {
                    Ok(page) => {
                        for found in page.items {
                            if !this.found.contains(&found) {
                                this.found.push(found);
                            }
                        }
                        this.more = page.more;
                    }
                    Err(reason) => this.error = Some(reason),
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

/// The comments under the video being watched, and the replies opened so far.
#[derive(Default)]
struct Thread {
    comments: Vec<Comment>,
    total: Option<String>,
    more: Option<String>,
    loading: bool,
    replies: HashMap<String, Vec<Comment>>,
    /// What posting a comment sends back, held once the first page came for an account.
    write: Option<String>,
    posting: bool,
    post: Option<Task<()>>,
    task: Option<Task<()>>,
    reply_tasks: Vec<Task<()>>,
}

/// The transcript of the video being watched, fetched the first time its panel is opened.
#[derive(Default)]
struct Script {
    open: bool,
    lines: Option<Arc<Vec<Line>>>,
    loading: bool,
    failed: bool,
    task: Option<Task<()>>,
}

/// The video being watched: its player, its page and its comments.
pub struct Watching {
    client: Client,
    io: Io,
    playback: Entity<Playback>,
    subscriptions: Entity<Subscriptions>,
    id: Option<String>,
    watch: Option<Arc<Watch>>,
    loading: bool,
    error: Option<String>,
    thread: Thread,
    script: Script,
    /// Made on the first video and kept, so one mpv serves every video after it.
    screen: Option<(Arc<Player>, Entity<VideoView>)>,
    /// The volume from 0 to 1, as the slider shows it.
    volume: f32,
    muted: bool,
    captions: bool,
    /// Whether the picture is in its own small window rather than on the watch page.
    floating: bool,
    /// Whether the picture fills the whole screen.
    full: bool,
    /// Whether the account liked the video, `None` as a guest.
    liked: Option<bool>,
    like: Option<Task<()>>,
    task: Option<Task<()>>,
}

impl Watching {
    pub fn new(
        client: Client,
        io: Io,
        playback: Entity<Playback>,
        subscriptions: Entity<Subscriptions>,
    ) -> Self {
        Self {
            client,
            io,
            playback,
            subscriptions,
            id: None,
            watch: None,
            loading: false,
            error: None,
            thread: Thread::default(),
            script: Script::default(),
            screen: None,
            volume: 1.,
            muted: false,
            captions: false,
            floating: false,
            full: false,
            liked: None,
            like: None,
            task: None,
        }
    }

    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub fn watch(&self) -> Option<&Watch> {
        self.watch.as_deref()
    }

    /// The video on the page as a tile lists it, for the history and for watching later.
    pub fn video(&self) -> Option<Video> {
        let watch = self.watch.as_ref()?;
        Some(Video {
            id: watch.id.clone(),
            title: watch.title.clone(),
            channel: watch.channel.as_ref().map(|channel| tube::ChannelRef {
                id: channel.id.clone(),
                name: channel.name.clone(),
            }),
            ..Video::default()
        })
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The picture, once a video has been opened.
    pub fn view(&self) -> Option<Entity<VideoView>> {
        self.screen.as_ref().map(|(_, view)| view.clone())
    }

    pub fn player(&self) -> Option<&Arc<Player>> {
        self.screen.as_ref().map(|(player, _)| player)
    }

    pub fn comments(&self) -> &[Comment] {
        &self.thread.comments
    }

    /// The count YouTube words itself, such as `3,534 Comments`.
    pub fn comment_total(&self) -> Option<&str> {
        self.thread.total.as_deref()
    }

    pub fn comments_loading(&self) -> bool {
        self.thread.loading
    }

    pub fn has_more_comments(&self) -> bool {
        self.thread.more.is_some()
    }

    /// The replies under `comment`, once [`Self::load_replies`] has fetched them.
    pub fn replies(&self, comment: &str) -> Option<&[Comment]> {
        self.thread.replies.get(comment).map(Vec::as_slice)
    }

    /// Starts `id` playing and fetches its page. The music stops, since both would sound at once.
    pub fn open(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id.as_deref() == Some(id) {
            self.set_paused(false, cx);
            return;
        }
        self.playback.update(cx, |playback, cx| playback.pause(cx));
        self.id = Some(id.to_owned());
        self.watch = None;
        self.error = None;
        self.thread = Thread::default();
        self.script = Script {
            open: self.script.open,
            ..Script::default()
        };
        if self.script.open {
            self.load_transcript(cx);
        }
        self.loading = true;
        cx.notify();

        if let Err(error) = self.play(id, cx) {
            log::error!("video: cannot play {id}: {error:#}");
            self.error = Some(format!("{error:#}"));
        }

        let (client, asked) = (self.client.clone(), id.to_owned());
        let request = self.io.spawn({
            let asked = asked.clone();
            async move { client.watch(&asked).await }
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                if this.id.as_deref() != Some(asked.as_str()) {
                    return;
                }
                this.loading = false;
                match crate::settled(loaded, cx) {
                    Ok(watch) => {
                        this.thread.more = watch.comments.clone();
                        this.liked = watch.liked;
                        this.watch = Some(Arc::new(watch));
                        if let Some(seen) = this.video() {
                            this.subscriptions
                                .update(cx, |kept, cx| kept.watched_now(seen, cx));
                        }
                        this.load_comments(cx);
                    }
                    Err(reason) => this.error = Some(reason),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Fetches the video's page again, leaving the player alone.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.id.take() else {
            return;
        };
        let paused = self.player().is_some_and(|player| player.paused());
        self.open(&id, cx);
        self.set_paused(paused, cx);
    }

    /// Stops the video when its page is left, so nothing plays unseen.
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        self.full = false;
        self.set_paused(true, cx);
    }

    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        if let Some(player) = self.player() {
            player.set_paused(paused);
        }
        cx.notify();
    }

    /// Whether the picture is in its own small window.
    pub fn floating(&self) -> bool {
        self.floating
    }

    /// Records that the picture moved to its own window or came back from it.
    pub fn set_floating(&mut self, floating: bool, cx: &mut Context<Self>) {
        self.floating = floating;
        cx.notify();
    }

    /// Whether the picture fills the whole screen.
    pub fn full(&self) -> bool {
        self.full
    }

    pub fn set_full(&mut self, full: bool, cx: &mut Context<Self>) {
        self.full = full;
        cx.notify();
    }

    /// A second picture of the video for another window, once there is a player.
    pub fn mirror(&self, cx: &mut App) -> Option<AnyView> {
        let (_, view) = self.screen.as_ref()?;
        Some(cx.new(|cx| VideoView::mirror(view, cx)).into())
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        let paused = self.player().is_some_and(|player| player.paused());
        self.set_paused(!paused, cx);
    }

    /// The volume from 0 to 1, which is 0 while muted.
    pub fn volume(&self) -> f32 {
        match self.muted {
            true => 0.,
            false => self.volume,
        }
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Sets the volume from 0 to 1, which also ends a mute.
    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.volume = volume.clamp(0., 1.);
        self.muted = false;
        self.sound();
        cx.notify();
    }

    pub fn toggle_muted(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        self.sound();
        cx.notify();
    }

    pub fn captions(&self) -> bool {
        self.captions
    }

    pub fn toggle_captions(&mut self, cx: &mut Context<Self>) {
        self.show_captions(!self.captions, cx);
    }

    /// Shows or hides the captions of this video and the ones after it.
    pub fn show_captions(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.captions == shown {
            return;
        }
        self.captions = shown;
        if let Some(player) = self.player() {
            player.set_captions(self.captions);
        }
        cx.notify();
    }

    /// Hands the volume to the player.
    fn sound(&self) {
        if let Some(player) = self.player() {
            player.set_volume(f64::from(self.volume()) * 100.);
        }
    }

    pub fn seek_to(&mut self, seconds: f64, cx: &mut Context<Self>) {
        if let Some(player) = self.player() {
            player.seek_to(seconds);
        }
        cx.notify();
    }

    pub fn seek_by(&mut self, seconds: f64, cx: &mut Context<Self>) {
        if let Some(player) = self.player() {
            player.seek_by(seconds);
        }
        cx.notify();
    }

    /// Whether the transcript panel is open.
    pub fn transcript_open(&self) -> bool {
        self.script.open
    }

    /// The lines of the transcript once they have arrived. A video without captions has none.
    pub fn transcript(&self) -> Option<Arc<Vec<Line>>> {
        self.script.lines.clone()
    }

    pub fn transcript_loading(&self) -> bool {
        self.script.loading
    }

    /// Whether the transcript could not be fetched, as opposed to the video having none.
    pub fn transcript_failed(&self) -> bool {
        self.script.failed
    }

    /// Opens or closes the transcript panel, fetching the lines the first time it opens.
    pub fn toggle_transcript(&mut self, cx: &mut Context<Self>) {
        self.script.open = !self.script.open;
        if self.script.open && self.script.lines.is_none() {
            self.load_transcript(cx);
        }
        cx.notify();
    }

    /// Fetches the transcript of the current video, unless it is already on its way.
    pub fn load_transcript(&mut self, cx: &mut Context<Self>) {
        if self.script.loading {
            return;
        }
        let Some(id) = self.id.clone() else {
            return;
        };
        self.script.loading = true;
        self.script.failed = false;
        cx.notify();
        let client = self.client.clone();
        let asked = Some(id.clone());
        let request = self.io.spawn(async move { client.transcript(&id).await });
        self.script.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                if this.id != asked {
                    return;
                }
                this.script.loading = false;
                match loaded {
                    Ok(lines) => this.script.lines = Some(Arc::new(lines)),
                    Err(error) => {
                        log::warn!("video: cannot read the transcript: {error:#}");
                        this.script.failed = true;
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Whether the account liked the video, `None` as a guest or before the page came.
    pub fn liked(&self) -> Option<bool> {
        self.liked
    }

    /// Likes the video as the account, or takes the like back. The button turns at once and
    /// turns back when YouTube refuses.
    pub fn toggle_like(&mut self, cx: &mut Context<Self>) {
        let (Some(id), Some(before)) = (self.id.clone(), self.liked) else {
            return;
        };
        self.liked = Some(!before);
        cx.notify();
        let client = self.client.clone();
        let asked = id.clone();
        let request = self
            .io
            .spawn(async move { client.like(&id, !before).await });
        self.like = Some(cx.spawn(async move |this, cx| {
            let Err(error) = join(request).await else {
                return;
            };
            log::warn!("video: cannot like: {error:#}");
            this.update(cx, |this, cx| {
                if this.id.as_deref() == Some(asked.as_str()) {
                    this.liked = Some(before);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Whether the account may write a comment under the video.
    pub fn can_comment(&self) -> bool {
        self.thread.write.is_some()
    }

    /// Whether a comment is on its way to YouTube.
    pub fn posting(&self) -> bool {
        self.thread.posting
    }

    /// Posts a comment under the video, then reads the comments again so it shows among them.
    pub fn post(&mut self, words: &str, cx: &mut Context<Self>) {
        let words = words.trim().to_owned();
        let Some(write) = self.thread.write.clone() else {
            return;
        };
        if words.is_empty() || self.thread.posting {
            return;
        }
        self.thread.posting = true;
        cx.notify();
        let client = self.client.clone();
        let asked = self.id.clone();
        let request = self
            .io
            .spawn(async move { client.comment(&write, &words).await });
        self.thread.post = Some(cx.spawn(async move |this, cx| {
            let posted = join(request).await;
            this.update(cx, |this, cx| {
                if this.id != asked {
                    return;
                }
                this.thread.posting = false;
                match posted {
                    Ok(()) => {
                        let first = this.watch.as_ref().and_then(|watch| watch.comments.clone());
                        this.thread.comments.clear();
                        this.thread.replies.clear();
                        this.thread.loading = false;
                        this.thread.more = first;
                        this.load_comments(cx);
                    }
                    Err(error) => log::warn!("video: cannot post the comment: {error:#}"),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Fetches the next page of comments, unless one is already on its way.
    pub fn load_comments(&mut self, cx: &mut Context<Self>) {
        if self.thread.loading {
            return;
        }
        let Some(token) = self.thread.more.take() else {
            return;
        };
        self.thread.loading = true;
        cx.notify();
        let client = self.client.clone();
        let request = self.io.spawn(async move { client.comments(&token).await });
        let asked = self.id.clone();
        self.thread.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                if this.id != asked {
                    return;
                }
                this.thread.loading = false;
                match loaded {
                    Ok(comments) => {
                        this.thread.comments.extend(comments.page.items);
                        this.thread.more = comments.page.more;
                        if comments.total.is_some() {
                            this.thread.total = comments.total;
                        }
                        if comments.write.is_some() {
                            this.thread.write = comments.write;
                        }
                    }
                    Err(error) => log::warn!("video: cannot read comments: {error:#}"),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Fetches the replies under one comment.
    pub fn load_replies(&mut self, comment: &str, cx: &mut Context<Self>) {
        let Some(token) = self
            .thread
            .comments
            .iter()
            .find(|listed| listed.id == comment)
            .and_then(|listed| listed.replies.clone())
        else {
            return;
        };
        if self.thread.replies.contains_key(comment) {
            return;
        }
        self.thread.replies.insert(comment.to_owned(), Vec::new());
        let client = self.client.clone();
        let request = self.io.spawn(async move { client.comments(&token).await });
        let (asked, comment) = (self.id.clone(), comment.to_owned());
        let task = cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                if this.id != asked {
                    return;
                }
                match loaded {
                    Ok(replies) => {
                        this.thread.replies.insert(comment, replies.page.items);
                    }
                    Err(error) => {
                        this.thread.replies.remove(&comment);
                        log::warn!("video: cannot read replies: {error:#}");
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.thread.reply_tasks.push(task);
    }

    fn play(&mut self, id: &str, cx: &mut Context<Self>) -> anyhow::Result<()> {
        if self.screen.is_none() {
            let (player, frames) = Player::new()?;
            let player = Arc::new(player);
            let view = cx.new(|cx| VideoView::new(player.clone(), frames, cx));
            self.screen = Some((player, view));
        }
        let Some((player, _)) = &self.screen else {
            return Ok(());
        };
        player.load(&tube::watch_url(id))?;
        player.set_volume(f64::from(self.volume()) * 100.);
        player.set_captions(self.captions);
        player.set_paused(false);
        Ok(())
    }
}

/// What a page of videos belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shelf {
    Channel(String),
    Playlist(String),
}

/// A channel's uploads or a playlist's videos.
pub struct VideoList {
    client: Client,
    io: Io,
    shelf: Option<Shelf>,
    title: String,
    channel: Option<Channel>,
    description: String,
    videos: Vec<Video>,
    more: Option<String>,
    loading: bool,
    error: Option<String>,
    task: Option<Task<()>>,
}

/// A loaded channel or playlist, reduced to what the page shows.
struct Listed {
    title: String,
    channel: Option<Channel>,
    description: String,
    videos: Page<Video>,
}

impl VideoList {
    pub fn new(client: Client, io: Io) -> Self {
        Self {
            client,
            io,
            shelf: None,
            title: String::new(),
            channel: None,
            description: String::new(),
            videos: Vec::new(),
            more: None,
            loading: false,
            error: None,
            task: None,
        }
    }

    pub fn shelf(&self) -> Option<&Shelf> {
        self.shelf.as_ref()
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// The channel, when the page is one.
    pub fn channel(&self) -> Option<&Channel> {
        self.channel.as_ref()
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn videos(&self) -> &[Video] {
        &self.videos
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn has_more(&self) -> bool {
        self.more.is_some()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn open(&mut self, shelf: Shelf, cx: &mut Context<Self>) {
        if self.shelf.as_ref() == Some(&shelf) && (self.loading || self.error.is_none()) {
            return;
        }
        self.shelf = Some(shelf);
        self.refresh(cx);
    }

    /// Drops the page and fetches it again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(shelf) = self.shelf.clone() else {
            return;
        };
        self.title.clear();
        self.channel = None;
        self.description.clear();
        self.videos.clear();
        self.more = None;
        self.error = None;
        self.loading = true;
        cx.notify();

        let client = self.client.clone();
        let asked = shelf.clone();
        let request = self.io.spawn(async move {
            Ok(match asked {
                Shelf::Channel(id) => {
                    let page = client.channel(&id).await?;
                    Listed {
                        title: page.channel.name.clone(),
                        channel: Some(page.channel),
                        description: page.description,
                        videos: page.videos,
                    }
                }
                Shelf::Playlist(id) => {
                    let page = client.playlist(&id).await?;
                    Listed {
                        title: page.title,
                        channel: None,
                        description: String::new(),
                        videos: page.videos,
                    }
                }
            })
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                if this.shelf.as_ref() != Some(&shelf) {
                    return;
                }
                this.loading = false;
                match crate::settled(loaded, cx) {
                    Ok(listed) => {
                        this.title = listed.title;
                        this.channel = listed.channel;
                        this.description = listed.description;
                        this.videos = listed.videos.items;
                        this.more = listed.videos.more;
                    }
                    Err(reason) => this.error = Some(reason),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Fetches the stretch after the last one, unless one is already on its way.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(token) = self.more.take() else {
            return;
        };
        self.loading = true;
        cx.notify();
        let client = self.client.clone();
        let request = self
            .io
            .spawn(async move { client.videos_more(&token).await });
        let shelf = self.shelf.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                if this.shelf != shelf {
                    return;
                }
                this.loading = false;
                match crate::settled(loaded, cx) {
                    Ok(page) => {
                        for video in page.items {
                            if !this.videos.contains(&video) {
                                this.videos.push(video);
                            }
                        }
                        this.more = page.more;
                    }
                    Err(reason) => log::warn!("video: cannot read more videos: {reason}"),
                }
                cx.notify();
            })
            .ok();
        }));
    }
}

/// How many watched videos the history keeps.
/// How often the sign-in window is asked whether the account is in.
const WINDOW_POLL: Duration = Duration::from_millis(300);
const SIGN_IN_URL: &str = "https://accounts.google.com/ServiceLogin?service=youtube&passive=true&continue=https%3A%2F%2Fwww.youtube.com%2Fsignin%3Faction_handle_signin%3Dtrue%26next%3Dhttps%253A%252F%252Fwww.youtube.com%252F";
const LANDING: &str = "www.youtube.com";
const COOKIE_DOMAIN: &str = "youtube.com";

/// The one [`Account`] of the app, where the chrome finds it.
struct Held(Entity<Account>);

impl Global for Held {}

/// The YouTube account the client acts as: signed in through a browser window, kept as its
/// cookies in a file only the owner reads, and handed to the [`Client`] so likes and comments
/// go out under it.
pub struct Account {
    client: Client,
    path: Option<PathBuf>,
    window: Option<webview::Page>,
    failed: bool,
    task: Option<Task<()>>,
}

impl Account {
    /// Makes the account, signs it in from the kept cookies and registers it for
    /// [`Account::entity`].
    pub fn install(client: Client, cx: &mut App) -> Entity<Self> {
        let path = dirs::data_local_dir().map(|root| {
            root.join("youtube-plus")
                .join("accounts")
                .join("tube")
                .join("cookies")
        });
        let kept = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok());
        if let Some(cookies) = kept
            && let Err(error) = client.sign_in(Some(cookies))
        {
            log::warn!("video: cannot use the kept account: {error:#}");
        }
        let account = cx.new(|_| Self {
            client,
            path,
            window: None,
            failed: false,
            task: None,
        });
        cx.set_global(Held(account.clone()));
        account
    }

    /// The app's account, once [`Account::install`] ran.
    pub fn entity(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<Held>().map(|held| held.0.clone())
    }

    pub fn signed_in(&self) -> bool {
        self.client.signed_in()
    }

    /// Whether the sign-in window is open.
    pub fn signing_in(&self) -> bool {
        self.window.is_some()
    }

    /// Whether the last sign-in could not open its window or keep its cookies.
    pub fn failed(&self) -> bool {
        self.failed
    }

    /// Whether this platform has a browser window to sign in through.
    pub fn supported(&self) -> bool {
        webview::supported()
    }

    /// Opens YouTube's own sign-in in a browser window named `title` and takes the session's
    /// cookies once the account is in.
    pub fn sign_in(&mut self, title: String, cx: &mut Context<Self>) {
        if self.window.is_some() {
            return;
        }
        self.failed = false;
        let target = webview::Target {
            url: SIGN_IN_URL.to_owned(),
            landing: LANDING.to_owned(),
            domain: COOKIE_DOMAIN.to_owned(),
            proof: tube::PROOF.iter().map(ToString::to_string).collect(),
            title,
            agent: None,
            script: None,
        };
        match webview::Page::open(target) {
            Ok(window) => self.window = Some(window),
            Err(error) => {
                log::warn!("video: cannot open the sign-in window: {error:#}");
                self.failed = true;
                return cx.notify();
            }
        }
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WINDOW_POLL).await;
                let open = this.update(cx, |this, cx| {
                    let Some(window) = this.window.as_mut() else {
                        return false;
                    };
                    let cookies = match window.poll() {
                        webview::Poll::Pending => return true,
                        webview::Poll::Closed => None,
                        webview::Poll::Cookies(cookies) => Some(cookies),
                    };
                    this.window = None;
                    if let Some(cookies) = cookies {
                        this.keep(cookies);
                    }
                    cx.notify();
                    false
                });
                if !open.unwrap_or(false) {
                    break;
                }
            }
        }));
    }

    /// Goes back to a guest and forgets the kept cookies.
    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        self.window = None;
        self.client.sign_in(None).ok();
        if let Some(path) = &self.path {
            std::fs::remove_file(path).ok();
        }
        cx.notify();
    }

    /// Hands the cookies to the client and writes them where the next launch finds them.
    fn keep(&mut self, cookies: String) {
        if let Err(error) = self.client.sign_in(Some(cookies.clone())) {
            log::warn!("video: cannot sign in: {error:#}");
            self.failed = true;
            return;
        }
        let Some(path) = &self.path else {
            return;
        };
        if let Err(error) = write_private(path, cookies.as_bytes()) {
            log::warn!("video: cannot keep the account: {error:#}");
        }
    }
}

/// Writes a file only its owner can read, making its folder first.
fn write_private(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)?.write_all(contents)
}

const HISTORY_CAP: usize = 500;
/// How many of a channel's newest uploads go into the feed.
const FEED_PER_CHANNEL: usize = 12;

/// What is kept on disk between runs: the channels followed and the videos watched, newest first.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Kept {
    #[serde(default)]
    channels: Vec<Channel>,
    #[serde(default)]
    watched: Vec<Video>,
    #[serde(default)]
    later: Vec<Video>,
}

/// The channels the user follows, the feed of their newest uploads and the videos watched. All
/// of it lives on this machine, so nothing here needs a YouTube account.
pub struct Subscriptions {
    client: Client,
    io: Io,
    path: PathBuf,
    kept: Kept,
    feed: Vec<Video>,
    loaded: bool,
    loading: bool,
    task: Option<Task<()>>,
}

impl Subscriptions {
    pub fn new(client: Client, io: Io) -> Self {
        let path = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("youtube-plus")
            .join("videos.json");
        let kept = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            client,
            io,
            path,
            kept,
            feed: Vec::new(),
            loaded: false,
            loading: false,
            task: None,
        }
    }

    pub fn channels(&self) -> &[Channel] {
        &self.kept.channels
    }

    /// The videos watched, the latest first.
    pub fn watched(&self) -> &[Video] {
        &self.kept.watched
    }

    /// The newest uploads of the followed channels, the latest first.
    pub fn feed(&self) -> &[Video] {
        &self.feed
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn follows(&self, channel: &str) -> bool {
        self.kept.channels.iter().any(|kept| kept.id == channel)
    }

    /// Follows the channel, or stops following it.
    pub fn toggle(&mut self, channel: &Channel, cx: &mut Context<Self>) {
        match self.follows(&channel.id) {
            true => {
                self.kept.channels.retain(|kept| kept.id != channel.id);
                self.feed
                    .retain(|video| video.channel.as_ref().is_none_or(|by| by.id != channel.id));
            }
            false => {
                self.kept.channels.push(channel.clone());
                self.loaded = false;
            }
        }
        self.save();
        cx.notify();
    }

    /// Puts the video at the top of the history.
    pub fn watched_now(&mut self, video: Video, cx: &mut Context<Self>) {
        self.kept.watched.retain(|kept| kept.id != video.id);
        self.kept.watched.insert(0, video);
        self.kept.watched.truncate(HISTORY_CAP);
        self.save();
        cx.notify();
    }

    /// The videos saved to watch later, the latest saved first.
    pub fn later(&self) -> &[Video] {
        &self.kept.later
    }

    pub fn saved(&self, video: &str) -> bool {
        self.kept.later.iter().any(|kept| kept.id == video)
    }

    /// Saves the video to watch later, or takes it back out.
    pub fn toggle_later(&mut self, video: &Video, cx: &mut Context<Self>) {
        match self.saved(&video.id) {
            true => self.kept.later.retain(|kept| kept.id != video.id),
            false => self.kept.later.insert(0, video.clone()),
        }
        self.save();
        cx.notify();
    }

    pub fn forget_watched(&mut self, cx: &mut Context<Self>) {
        self.kept.watched.clear();
        self.save();
        cx.notify();
    }

    /// Fetches the feed unless it is already here or on its way.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        if !self.loaded && !self.loading {
            self.refresh(cx);
        }
    }

    /// Fetches the newest uploads of every followed channel again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.kept.channels.is_empty() {
            self.feed.clear();
            self.loaded = true;
            cx.notify();
            return;
        }
        self.loading = true;
        cx.notify();
        let client = self.client.clone();
        let channels = self.kept.channels.clone();
        let request = self.io.spawn(async move {
            let pages = channels.iter().map(|channel| client.channel(&channel.id));
            let mut feed = Vec::new();
            for (channel, page) in channels.iter().zip(futures::future::join_all(pages).await) {
                match page {
                    Ok(page) => {
                        let by = tube::ChannelRef {
                            id: channel.id.clone(),
                            name: channel.name.clone(),
                        };
                        feed.extend(page.videos.items.into_iter().take(FEED_PER_CHANNEL).map(
                            |mut video| {
                                video.channel.get_or_insert_with(|| by.clone());
                                video
                            },
                        ));
                    }
                    Err(error) => log::warn!("video: cannot read {}: {error:#}", channel.name),
                }
            }
            feed.sort_by_key(|video| video.age().unwrap_or(u64::MAX));
            Ok(feed)
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                this.loaded = true;
                match crate::settled(loaded, cx) {
                    Ok(feed) => this.feed = feed,
                    Err(reason) => log::warn!("video: cannot read the feed: {reason}"),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn save(&self) {
        let written = serde_json::to_vec(&self.kept)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                if let Some(folder) = self.path.parent() {
                    std::fs::create_dir_all(folder)?;
                }
                Ok(std::fs::write(&self.path, bytes)?)
            });
        if let Err(error) = written {
            log::warn!("video: cannot save the followed channels: {error:#}");
        }
    }
}
