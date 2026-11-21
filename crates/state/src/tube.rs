//! YouTube as the video screens see it: one entity each for the search, the video being watched
//! and the channel or playlist that is open.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AppContext as _, Context, Entity, Task};
use tube::{Channel, Client, Comment, Found, Page, Video, Watch};
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
    task: Option<Task<()>>,
    reply_tasks: Vec<Task<()>>,
}

/// The video being watched: its player, its page and its comments.
pub struct Watching {
    client: Client,
    io: Io,
    playback: Entity<Playback>,
    id: Option<String>,
    watch: Option<Arc<Watch>>,
    loading: bool,
    error: Option<String>,
    thread: Thread,
    /// Made on the first video and kept, so one mpv serves every video after it.
    screen: Option<(Arc<Player>, Entity<VideoView>)>,
    task: Option<Task<()>>,
}

impl Watching {
    pub fn new(client: Client, io: Io, playback: Entity<Playback>) -> Self {
        Self {
            client,
            io,
            playback,
            id: None,
            watch: None,
            loading: false,
            error: None,
            thread: Thread::default(),
            screen: None,
            task: None,
        }
    }

    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub fn watch(&self) -> Option<&Watch> {
        self.watch.as_deref()
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
                        this.watch = Some(Arc::new(watch));
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
        self.set_paused(true, cx);
    }

    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        if let Some(player) = self.player() {
            player.set_paused(paused);
        }
        cx.notify();
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        let paused = self.player().is_some_and(|player| player.paused());
        self.set_paused(!paused, cx);
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
