//! YouTube as the video screens see it: one entity each for the search, the video being watched
//! and the channel or playlist that is open.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyView, App, AppContext as _, ClipboardItem, Context, Entity, Global, Image, Task};
use tube::ai::{Said, Speaker};
use tube::{
    Caption, Channel, Chapter, Client, Comment, Found, Identity, Line, Page, Peek, Playlist, Post,
    Quality, Storyboard, Tab, Video, Watch,
};
use video::{Player, VideoView};

use crate::{Assistant, Io, Outcome, Playback, Toasts, VideoDownloads, join};

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

/// How fast a video plays while its picture is held down.
pub const HURRY: f32 = 2.;
/// The tallest picture played when the user has not chosen one.
const AUTO_HEIGHT: u32 = 1080;
/// How long a caption without a length of its own stays up, in seconds.
const CAPTION_STAYS: f64 = 5.;
/// How often and how many times a freshly opened stream is checked for having failed.
const OPENING_CHECK: Duration = Duration::from_millis(250);
const OPENING_CHECKS: usize = 80;
/// How often the place a video is watched up to is written down.
const PLACE_EVERY: Duration = Duration::from_secs(5);
/// The share of a video before which it is not worth resuming and after which it is finished.
const RESUME_FROM: f32 = 0.02;
const RESUME_UNTIL: f32 = 0.95;

/// The comments under the video being watched, and the replies opened so far.
#[derive(Default)]
struct Thread {
    comments: Vec<Comment>,
    total: Option<String>,
    more: Option<String>,
    loading: bool,
    replies: HashMap<String, Vec<Comment>>,
    /// The comments whose fetched replies are folded away again.
    folded: std::collections::HashSet<String>,
    /// The comments whose like was turned here, and which way.
    liked: HashMap<String, bool>,
    actions: Vec<Task<()>>,
    /// What posting a comment sends back, held once the first page came for an account.
    write: Option<String>,
    posting: bool,
    post: Option<Task<()>>,
    task: Option<Task<()>>,
    reply_tasks: Vec<Task<()>>,
}

/// A chat with the AI about the video being watched, which reads its transcript.
#[derive(Default)]
struct Chat {
    said: Vec<Said>,
    /// Whether an answer is on its way.
    asking: bool,
    /// Whether the question waits for the transcript to arrive before it is sent.
    waiting: bool,
    /// Why the last answer did not come.
    failed: Option<String>,
    task: Option<Task<()>>,
}

/// The transcript of the video being watched, fetched the first time its panel is opened.
#[derive(Default)]
struct Script {
    open: bool,
    lines: Option<Arc<Vec<Line>>>,
    /// The languages the captions could be in, and the one they are in.
    choices: Arc<Vec<Caption>>,
    shown: Option<Caption>,
    /// The language asked for by hand, which the next fetch reads the captions in.
    wanted: Option<Caption>,
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
    liked: bool,
    /// Whether the page on its way is one read again, which the history leaves out.
    again: bool,
    like: Option<Task<()>>,
    /// The qualities the video can be played at, best first, once YouTube has named them.
    qualities: Vec<Quality>,
    /// The small pictures the seek bar previews the video with.
    board: Option<Storyboard>,
    /// The tallest picture the user wants, `None` to let the app choose.
    ceiling: Option<u32>,
    /// Whether the picture's colours glow around it.
    ambient: bool,
    /// How fast videos play, 1 being as recorded.
    speed: f32,
    /// Whether the video is held at [`HURRY`] for as long as the picture is pressed.
    hurried: bool,
    stream: Option<Task<()>>,
    task: Option<Task<()>>,
    /// Whether the comments come newest first rather than best first.
    newest: bool,
    /// Writes down how far the video is watched while it plays.
    placing: Option<Task<()>>,
    shot: Option<Task<()>>,
    /// The video's chapters in order: the description's, or the AI's when it lists none.
    chapters: Arc<Vec<Chapter>>,
    /// Whether the chapters were cut by the AI rather than written by the uploader.
    chapters_ai: bool,
    chaptering: Option<Task<()>>,
    /// Whether cutting chapters for this video was tried and failed, so it is not retried.
    chapters_failed: bool,
    /// The key and model the chapters last failed with, so only a change of either tries again.
    chapters_tried: Option<String>,
    chat: Chat,
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
            liked: false,
            again: false,
            like: None,
            qualities: Vec::new(),
            board: None,
            ceiling: None,
            ambient: true,
            speed: 1.,
            hurried: false,
            stream: None,
            task: None,
            newest: false,
            placing: None,
            shot: None,
            chapters: Arc::default(),
            chapters_ai: false,
            chaptering: None,
            chapters_failed: false,
            chapters_tried: None,
            chat: Chat::default(),
        }
    }

    /// The video's chapters in order, empty when it has none.
    pub fn chapters(&self) -> Arc<Vec<Chapter>> {
        self.chapters.clone()
    }

    /// Whether the chapters were made by the AI.
    pub fn chapters_ai(&self) -> bool {
        self.chapters_ai
    }

    /// Whether the AI is cutting chapters for the video right now.
    pub fn chaptering(&self) -> bool {
        self.chaptering.is_some()
    }

    /// The place of the chapter playing at `seconds`.
    pub fn chapter_at(&self, seconds: f64) -> Option<usize> {
        self.chapters
            .iter()
            .rposition(|chapter| chapter.start <= seconds)
    }

    /// The messages of the chat about the video, oldest first.
    pub fn said(&self) -> &[Said] {
        &self.chat.said
    }

    /// Whether the AI's answer is on its way.
    pub fn asking(&self) -> bool {
        self.chat.asking
    }

    /// Why the last answer did not come, until another question is asked.
    pub fn chat_failed(&self) -> Option<&str> {
        self.chat.failed.as_deref()
    }

    /// Whether the chat failed because the video has no transcript to go on.
    pub fn chat_blind(&self) -> bool {
        self.chat.failed.is_some() && self.script.lines.is_none() && self.script.failed
    }

    /// Sends `question` to the AI, with the transcript as what it knows about the video.
    pub fn ask(&mut self, question: &str, cx: &mut Context<Self>) {
        let question = question.trim();
        if question.is_empty() || self.chat.asking {
            return;
        }
        self.chat.said.push(Said {
            speaker: Speaker::User,
            text: question.to_owned(),
        });
        self.answer(cx);
    }

    /// Asks for the answer to the last question again, after it failed.
    pub fn ask_again(&mut self, cx: &mut Context<Self>) {
        let unanswered = self
            .chat
            .said
            .last()
            .is_some_and(|said| said.speaker == Speaker::User);
        if !unanswered || self.chat.asking {
            return;
        }
        if self.script.lines.is_none() && self.script.failed {
            self.chat.failed = None;
            self.chat.asking = true;
            self.chat.waiting = true;
            self.load_transcript(cx);
            return;
        }
        self.answer(cx);
    }

    /// Forgets the chat, to start over.
    pub fn clear_chat(&mut self, cx: &mut Context<Self>) {
        self.chat = Chat::default();
        cx.notify();
    }

    /// Sends the chat to the AI once the transcript is here, fetching it first when needed.
    fn answer(&mut self, cx: &mut Context<Self>) {
        self.chat.failed = None;
        self.chat.asking = true;
        cx.notify();
        let lines = match &self.script.lines {
            Some(lines) => lines.clone(),
            None if !self.script.failed => {
                self.chat.waiting = true;
                self.load_transcript(cx);
                return;
            }
            None => {
                self.chat.asking = false;
                self.chat.failed = Some("the transcript could not be loaded".to_owned());
                return;
            }
        };
        let assistant = Assistant::entity(cx);
        let (client, model) = match &assistant {
            Some(assistant) => {
                let assistant = assistant.read(cx);
                (assistant.client(), assistant.chat_model().to_owned())
            }
            None => (None, String::new()),
        };
        let Some(client) = client else {
            self.chat.asking = false;
            self.chat.failed = Some("no key".to_owned());
            return;
        };
        let title = self
            .watch
            .as_ref()
            .map(|watch| watch.title.clone())
            .unwrap_or_default();
        let (said, asked) = (self.chat.said.clone(), self.id.clone());
        let request = self
            .io
            .spawn(async move { client.ask(&model, &title, &lines, &said).await });
        self.chat.task = Some(cx.spawn(async move |this, cx| {
            let answered = join(request).await;
            this.update(cx, |this, cx| {
                if this.id != asked {
                    return;
                }
                this.chat.asking = false;
                match answered {
                    Ok(text) => this.chat.said.push(Said {
                        speaker: Speaker::Assistant,
                        text,
                    }),
                    Err(error) => {
                        let reason = format!("{error:#}");
                        log::warn!("video: the ai cannot answer: {reason}");
                        refused(&reason, cx);
                        this.chat.failed = Some(reason);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Has the AI cut chapters from the transcript, when the video lists none, a key is saved
    /// and auto chapters are on.
    fn chapter(&mut self, cx: &mut Context<Self>) {
        if !self.chapters.is_empty()
            || self.chaptering.is_some()
            || self.chapters_failed
            || self.watch.is_none()
        {
            return;
        }
        let Some(assistant) = Assistant::entity(cx) else {
            return;
        };
        let (client, model) = {
            let assistant = assistant.read(cx);
            if !assistant.auto_chapters() {
                return;
            }
            (assistant.client(), assistant.chapter_model().to_owned())
        };
        let Some(client) = client else { return };
        let lines = match &self.script.lines {
            Some(lines) if !lines.is_empty() => lines.clone(),
            Some(_) => return,
            None => {
                if !self.script.failed {
                    self.load_transcript(cx);
                }
                return;
            }
        };
        let title = self
            .watch
            .as_ref()
            .map(|watch| watch.title.clone())
            .unwrap_or_default();
        let length = self
            .player()
            .and_then(|player| player.duration())
            .or_else(|| lines.last().map(|line| line.start + line.length))
            .unwrap_or(0.);
        let asked = self.id.clone();
        let request = self
            .io
            .spawn(async move { client.chapters(&model, &title, &lines, length).await });
        self.chaptering = Some(cx.spawn(async move |this, cx| {
            let made = join(request).await;
            this.update(cx, |this, cx| {
                if this.id != asked {
                    return;
                }
                this.chaptering = None;
                match made {
                    Ok(chapters) if !chapters.is_empty() => {
                        this.chapters = Arc::new(chapters);
                        this.chapters_ai = true;
                    }
                    Ok(_) => this.chapters_failed = true,
                    Err(error) => {
                        let reason = format!("{error:#}");
                        log::warn!("video: the ai cannot cut chapters: {reason}");
                        refused(&reason, cx);
                        this.chapters_failed = true;
                    }
                }
                if this.chapters_failed {
                    this.chapters_tried = tried(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Tries the AI chapters again, after a key was saved or auto chapters were turned on.
    pub fn chapter_again(&mut self, cx: &mut Context<Self>) {
        if self.chapters_failed && self.chapters_tried != tried(cx) {
            self.chapters_failed = false;
        }
        self.chapter(cx);
    }

    /// Whether the comments come newest first.
    pub fn comments_newest(&self) -> bool {
        self.newest
    }

    /// Whether the video's comments can be put in another order.
    pub fn comments_sortable(&self) -> bool {
        self.watch
            .as_ref()
            .is_some_and(|watch| watch.top.is_some() && watch.newest.is_some())
    }

    /// Fetches the comments again, newest first or best first.
    pub fn sort_comments(&mut self, newest: bool, cx: &mut Context<Self>) {
        let Some(watch) = &self.watch else { return };
        let token = match newest {
            true => watch.newest.clone(),
            false => watch.top.clone(),
        };
        if token.is_none() || newest == self.newest {
            return;
        }
        self.newest = newest;
        self.thread = Thread {
            total: self.thread.total.take(),
            write: self.thread.write.take(),
            more: token,
            ..Thread::default()
        };
        self.load_comments(cx);
    }

    /// Saves the picture on screen as an image beside the downloads, or puts it on the
    /// clipboard, and tells how it went in a toast.
    pub fn take_frame(&mut self, copied: bool, cx: &mut Context<Self>) {
        let Some(player) = self.player().cloned() else {
            return;
        };
        let title = self
            .watch
            .as_ref()
            .map(|watch| watch.title.clone())
            .or_else(|| self.id.clone())
            .unwrap_or_default();
        let at = player.position().unwrap_or(0.).max(0.) as u64;
        let name: String = format!("{title} {:02}m{:02}s.png", at / 60, at % 60)
            .chars()
            .map(|letter| match letter {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                letter if letter.is_control() => '_',
                letter => letter,
            })
            .collect();
        let folder = match copied {
            true => Some(std::env::temp_dir()),
            false => VideoDownloads::folder(),
        };
        let Some(path) = folder.map(|folder| folder.join(name)) else {
            return;
        };
        let taken = cx.background_executor().spawn(async move {
            if let Some(folder) = path.parent() {
                std::fs::create_dir_all(folder)?;
            }
            player.snapshot(&path)?;
            anyhow::ensure!(path.is_file(), "the player wrote no picture");
            anyhow::Ok(match copied {
                true => {
                    let bytes = std::fs::read(&path)?;
                    std::fs::remove_file(&path).ok();
                    Some(bytes)
                }
                false => None,
            })
        });
        self.shot = Some(cx.spawn(async move |_, cx| {
            let taken = taken.await;
            cx.update(|cx| match taken {
                Ok(bytes) => {
                    if let Some(bytes) = bytes {
                        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                            gpui::ImageFormat::Png,
                            bytes,
                        )));
                    }
                    let key = match copied {
                        true => "toast-video-frame-copied",
                        false => "toast-video-frame-saved",
                    };
                    Toasts::about(Outcome::Done, key, title, cx);
                }
                Err(error) => {
                    log::warn!("video: cannot take the frame: {error:#}");
                    Toasts::about(Outcome::Failed, "toast-video-frame-failed", title, cx);
                }
            });
        }));
    }

    /// Writes down how far into the video the player is, for the bar on its thumbnail and for
    /// taking it up again.
    fn place(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.id.clone() else { return };
        let Some(player) = self.player() else { return };
        let (Some(at), Some(length)) = (player.position(), player.duration()) else {
            return;
        };
        if length <= 0. || player.idle() {
            return;
        }
        self.subscriptions
            .update(cx, |kept, cx| kept.set_place(&id, at, length, cx));
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

    /// Whether the replies of `comment` were fetched and then folded away.
    pub fn replies_folded(&self, comment: &str) -> bool {
        self.thread.folded.contains(comment)
    }

    /// Shows the replies of a comment, fetching them the first time, or folds them away.
    pub fn toggle_replies(&mut self, comment: &str, cx: &mut Context<Self>) {
        match self.thread.replies.contains_key(comment) {
            true => {
                if !self.thread.folded.remove(comment) {
                    self.thread.folded.insert(comment.to_owned());
                }
                cx.notify();
            }
            false => self.load_replies(comment, cx),
        }
    }

    /// Whether `comment` is liked, by what YouTube said unless it was turned here since.
    pub fn comment_liked(&self, comment: &Comment) -> bool {
        self.thread
            .liked
            .get(&comment.id)
            .copied()
            .unwrap_or(comment.liked)
    }

    /// Likes a comment or takes the like back. The heart turns at once. With an account YouTube
    /// is told, and the heart turns back when it refuses.
    pub fn toggle_comment_like(&mut self, comment: &Comment, cx: &mut Context<Self>) {
        let before = self.comment_liked(comment);
        self.thread.liked.insert(comment.id.clone(), !before);
        cx.notify();
        let action = match before {
            true => comment.unlike.clone(),
            false => comment.like.clone(),
        };
        let Some(action) = action else { return };
        let client = self.client.clone();
        let request = self
            .io
            .spawn(async move { client.comment_action(&action).await });
        let (asked, id) = (self.id.clone(), comment.id.clone());
        let task = cx.spawn(async move |this, cx| {
            let Err(error) = join(request).await else {
                return;
            };
            log::warn!("video: cannot like the comment: {error:#}");
            this.update(cx, |this, cx| {
                if this.id == asked {
                    this.thread.liked.insert(id, before);
                    cx.notify();
                }
            })
            .ok();
        });
        self.thread.actions.push(task);
    }

    /// Posts `words` as a reply to a top-level comment and fetches its replies again.
    pub fn reply(&mut self, comment: &Comment, words: &str, cx: &mut Context<Self>) {
        let words = words.trim().to_owned();
        let Some(reply) = comment.reply.clone() else {
            return;
        };
        if words.is_empty() {
            return;
        }
        let client = self.client.clone();
        let request = self
            .io
            .spawn(async move { client.reply(&reply, &words).await });
        let (asked, id) = (self.id.clone(), comment.id.clone());
        let task = cx.spawn(async move |this, cx| {
            let posted = join(request).await;
            this.update(cx, |this, cx| {
                if this.id != asked {
                    return;
                }
                match posted {
                    Ok(()) => {
                        this.thread.replies.remove(&id);
                        this.thread.folded.remove(&id);
                        this.load_replies(&id, cx);
                    }
                    Err(error) => log::warn!("video: cannot post the reply: {error:#}"),
                }
                cx.notify();
            })
            .ok();
        });
        self.thread.actions.push(task);
    }

    /// Starts `id` playing and fetches its page. The music stops, since both would sound at once.
    pub fn open(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.id.as_deref() == Some(id) {
            // Opened again, perhaps under another profile, whose history has yet to see it.
            if let Some(seen) = self.video() {
                self.subscriptions
                    .update(cx, |kept, cx| kept.watched_now(seen, cx));
            }
            self.set_paused(false, cx);
            return;
        }
        self.place(cx);
        self.newest = false;
        self.playback.update(cx, |playback, cx| playback.pause(cx));
        self.id = Some(id.to_owned());
        self.again = false;
        self.watch = None;
        self.error = None;
        self.thread = Thread::default();
        self.script = Script {
            open: self.script.open,
            ..Script::default()
        };
        self.chat = Chat::default();
        self.chapters = Arc::default();
        self.chapters_ai = false;
        self.chaptering = None;
        self.chapters_failed = false;
        if self.script.open {
            self.load_transcript(cx);
        }
        self.loading = true;
        self.liked = self.subscriptions.read(cx).liked(id);
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
                        if let Some(liked) = watch.liked {
                            this.liked = liked;
                        }
                        this.chapters = Arc::new(tube::chapters(&watch.description));
                        this.watch = Some(Arc::new(watch));
                        // A page read again is no new watch, and the profile may have changed.
                        let again = std::mem::take(&mut this.again);
                        if let Some(seen) = this.video().filter(|_| !again) {
                            this.subscriptions
                                .update(cx, |kept, cx| kept.watched_now(seen, cx));
                        }
                        this.load_comments(cx);
                        this.chapter(cx);
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
        // A page that never arrived was never put into the history, so its retry is.
        let again = self.watch.is_some();
        self.open(&id, cx);
        self.again = again;
        self.set_paused(paused, cx);
    }

    /// Stops the video when its page is left, so nothing plays unseen.
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        self.place(cx);
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
        if shown && self.script.lines.is_none() {
            self.load_transcript(cx);
        }
        cx.notify();
    }

    /// The words being spoken now, when captions are on and the video has any.
    pub fn caption(&self) -> Option<String> {
        if !self.captions {
            return None;
        }
        let lines = self.script.lines.as_ref()?;
        let now = self.player()?.position()?;
        let next = lines.partition_point(|line| line.start <= now);
        let line = lines.get(next.checked_sub(1)?)?;
        // A line without a length stays until the next one, though never for long.
        let length = match line.length > 0. {
            true => line.length,
            false => CAPTION_STAYS,
        };
        (now < line.start + length).then(|| line.text.clone())
    }

    /// The preview picture of the moment `seconds` in, once the video has named its previews.
    pub fn peek(&self, seconds: f64) -> Option<(Peek, (u32, u32))> {
        let board = self.board.as_ref()?;
        let length = self.player()?.duration()?;
        Some((board.at(seconds, length)?, (board.width, board.height)))
    }

    /// The qualities the video can be played at, best first. Empty until YouTube names them.
    pub fn qualities(&self) -> &[Quality] {
        &self.qualities
    }

    /// The tallest picture asked for, `None` when the app chooses.
    pub fn ceiling(&self) -> Option<u32> {
        self.ceiling
    }

    /// The quality being played, the best one under the ceiling.
    pub fn quality(&self) -> Option<&Quality> {
        let ceiling = self.ceiling.unwrap_or(AUTO_HEIGHT);
        self.qualities
            .iter()
            .find(|quality| quality.height <= ceiling)
            .or_else(|| self.qualities.last())
    }

    /// Plays this video and the ones after it no taller than `ceiling`, carrying on from where
    /// the picture is.
    pub fn set_ceiling(&mut self, ceiling: Option<u32>, cx: &mut Context<Self>) {
        if self.ceiling == ceiling {
            return;
        }
        let before = self.quality().map(|quality| quality.height);
        self.ceiling = ceiling;
        cx.notify();
        if before == self.quality().map(|quality| quality.height) {
            return;
        }
        let (Some(player), Some(quality)) = (self.player(), self.quality()) else {
            return;
        };
        let (at, paused) = (player.position().unwrap_or(0.), player.paused());
        if let Err(error) = player.play(&quality.picture, &quality.sound, at) {
            log::warn!("video: cannot switch the quality: {error:#}");
        }
        player.set_paused(paused);
    }

    /// How fast videos play, 1 being as recorded.
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Plays this video and the ones after it at `speed`.
    pub fn set_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        self.speed = speed;
        self.pace();
        cx.notify();
    }

    /// Whether the video is being hurried along by a held press.
    pub fn hurried(&self) -> bool {
        self.hurried
    }

    /// Plays at [`HURRY`] while `hurried`, and at the chosen speed again after.
    pub fn hurry(&mut self, hurried: bool, cx: &mut Context<Self>) {
        if self.hurried == hurried {
            return;
        }
        self.hurried = hurried;
        self.pace();
        cx.notify();
    }

    /// Hands the speed to the player.
    fn pace(&self) {
        let speed = match self.hurried {
            true => HURRY,
            false => self.speed,
        };
        if let Some(player) = self.player() {
            player.set_speed(f64::from(speed));
        }
    }

    pub fn ambient(&self) -> bool {
        self.ambient
    }

    /// Lets the picture's colours glow around it, or not.
    pub fn set_ambient(&mut self, ambient: bool, cx: &mut Context<Self>) {
        self.ambient = ambient;
        cx.notify();
    }

    /// The colours of the left and right halves of the picture as `0xRRGGBB`, when they are to
    /// glow around it.
    pub fn glow(&self) -> Option<(u32, u32)> {
        self.ambient
            .then(|| self.player().map(|player| player.glow()))
            .flatten()
    }

    /// Whether the picture is still on its way, so there is nothing to show yet.
    pub fn buffering(&self) -> bool {
        self.id.is_some()
            && self.error.is_none()
            && self
                .player()
                // a live stream has no length, so its place stands in for one
                .is_none_or(|player| player.duration().is_none() && player.position().is_none())
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

    /// The languages the captions of this video come in, once its captions were fetched.
    pub fn caption_choices(&self) -> Arc<Vec<Caption>> {
        self.script.choices.clone()
    }

    /// The language the captions are in.
    pub fn caption_shown(&self) -> Option<&Caption> {
        self.script.shown.as_ref()
    }

    /// Fetches the languages of the captions when nothing was fetched yet, for a menu of them.
    pub fn want_caption_choices(&mut self, cx: &mut Context<Self>) {
        if self.script.lines.is_none() {
            self.load_transcript(cx);
        }
    }

    /// Reads the captions and the transcript in another language, and turns the captions on.
    pub fn set_caption_language(&mut self, wanted: Caption, cx: &mut Context<Self>) {
        self.captions = true;
        self.script.wanted = Some(wanted);
        self.script.task = None;
        self.script.loading = false;
        self.load_transcript(cx);
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
        let wanted = self.script.wanted.clone();
        let request = self
            .io
            .spawn(async move { client.captions(&id, wanted.as_ref()).await });
        self.script.task = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                if this.id != asked {
                    return;
                }
                this.script.loading = false;
                match loaded {
                    Ok(captions) => {
                        this.script.lines = Some(Arc::new(captions.lines));
                        this.script.shown = captions.shown;
                        this.script.choices = Arc::new(captions.choices);
                    }
                    Err(error) => {
                        log::warn!("video: cannot read the transcript: {error:#}");
                        this.script.failed = true;
                    }
                }
                if std::mem::take(&mut this.chat.waiting) {
                    this.answer(cx);
                }
                this.chapter(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Whether the video is liked: by the account when there is one, on this machine otherwise.
    pub fn liked(&self) -> bool {
        self.liked
    }

    /// Likes the video or takes the like back. The button turns at once and the like is kept
    /// on this machine. With an account YouTube is told too, and the button turns back when it
    /// refuses.
    pub fn toggle_like(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.id.clone() else {
            return;
        };
        let before = self.liked;
        self.liked = !before;
        self.subscriptions
            .update(cx, |kept, cx| kept.set_liked(&id, !before, cx));
        cx.notify();
        if !self.client.signed_in() {
            return;
        }
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
                this.subscriptions
                    .update(cx, |kept, cx| kept.set_liked(&asked, before, cx));
                if this.id.as_deref() == Some(asked.as_str()) {
                    this.liked = before;
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
        player.stop();
        let start = self.subscriptions.read(cx).resume(id).unwrap_or(0.);
        if self.placing.is_none() {
            self.placing = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(PLACE_EVERY).await;
                    let alive = this.update(cx, |this, cx| {
                        if this.player().is_some_and(|player| !player.paused()) {
                            this.place(cx);
                        }
                    });
                    if alive.is_err() {
                        return;
                    }
                }
            }));
        }
        let Some((player, _)) = &self.screen else {
            return Ok(());
        };
        player.set_volume(f64::from(self.volume()) * 100.);
        player.set_paused(false);
        self.hurried = false;
        self.pace();
        self.qualities.clear();
        self.board = None;
        if self.captions {
            self.load_transcript(cx);
        }

        // Asking YouTube for the streams directly is seconds faster than letting the player
        // scrape the page, which stays as the way out when YouTube holds them back.
        let (client, asked) = (self.client.clone(), id.to_owned());
        let request = self.io.spawn({
            let asked = asked.clone();
            async move { client.streams(&asked).await }
        });
        self.stream = Some(cx.spawn(async move |this, cx| {
            let named = join(request).await;
            let started = this.update(cx, |this, cx| {
                if this.id.as_deref() != Some(asked.as_str()) {
                    return None;
                }
                let player = this.player()?.clone();
                let direct = match named {
                    Ok(streams) => {
                        this.qualities = streams.qualities;
                        this.board = streams.board;
                        this.quality()
                            .map(|quality| player.play(&quality.picture, &quality.sound, start))
                    }
                    Err(error) => {
                        log::warn!("video: no direct streams for {asked}: {error:#}");
                        None
                    }
                };
                cx.notify();
                match direct {
                    Some(Ok(())) => Some(player),
                    _ => {
                        this.scrape(&asked, start, cx);
                        None
                    }
                }
            });
            let Ok(Some(player)) = started else { return };
            for _ in 0..OPENING_CHECKS {
                cx.background_executor().timer(OPENING_CHECK).await;
                if player.duration().is_some() {
                    return;
                }
                if player.idle() {
                    this.update(cx, |this, cx| {
                        if this.id.as_deref() == Some(asked.as_str()) {
                            this.qualities.clear();
                            this.scrape(&asked, start, cx);
                        }
                    })
                    .ok();
                    return;
                }
            }
        }));
        Ok(())
    }

    /// Plays the video through the player's own page scraper, the slow way that always works.
    fn scrape(&mut self, id: &str, start: f64, cx: &mut Context<Self>) {
        let Some(player) = self.player() else { return };
        if let Err(error) = player.load_from(&tube::watch_url(id), start) {
            log::error!("video: cannot play {id}: {error:#}");
            self.error = Some(format!("{error:#}"));
        }
        cx.notify();
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
    banner: Option<String>,
    description: String,
    videos: Vec<Video>,
    more: Option<String>,
    loading: bool,
    error: Option<String>,
    task: Option<Task<()>>,
    /// The parts a channel offers and the one that is open.
    tabs: Vec<Tab>,
    tab: Tab,
    /// What the open part lists when it is not the videos, each part kept once fetched.
    playlists: HashMap<Tab, Page<Playlist>>,
    posts: Page<Post>,
    posts_read: bool,
    part: Option<Task<()>>,
}

/// A loaded channel or playlist, reduced to what the page shows.
struct Listed {
    title: String,
    channel: Option<Channel>,
    banner: Option<String>,
    description: String,
    videos: Page<Video>,
    tabs: Vec<Tab>,
}

impl VideoList {
    pub fn new(client: Client, io: Io) -> Self {
        Self {
            client,
            io,
            shelf: None,
            title: String::new(),
            channel: None,
            banner: None,
            description: String::new(),
            videos: Vec::new(),
            more: None,
            loading: false,
            error: None,
            task: None,
            tabs: Vec::new(),
            tab: Tab::Videos,
            playlists: HashMap::new(),
            posts: Page::default(),
            posts_read: false,
            part: None,
        }
    }

    /// The parts of the channel that can be opened, the videos among them.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// The playlists or the releases of the open part.
    pub fn playlists(&self) -> &[Playlist] {
        self.playlists
            .get(&self.tab)
            .map(|page| page.items.as_slice())
            .unwrap_or_default()
    }

    pub fn posts(&self) -> &[Post] {
        &self.posts.items
    }

    /// Opens a part of the channel, fetching it the first time.
    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.part = None;
        self.loading = false;
        let read = match tab {
            Tab::Videos => true,
            Tab::Posts => self.posts_read,
            Tab::Playlists | Tab::Releases => self.playlists.contains_key(&tab),
        };
        cx.notify();
        if !read {
            self.load_part(None, cx);
        }
    }

    /// Fetches the open part, or with a token the stretch after what it already lists.
    fn load_part(&mut self, token: Option<String>, cx: &mut Context<Self>) {
        let Some(Shelf::Channel(id)) = self.shelf.clone() else {
            return;
        };
        let (client, tab) = (self.client.clone(), self.tab);
        self.loading = true;
        cx.notify();
        let request = self.io.spawn(async move {
            match token {
                Some(token) => client.section_more(&token).await,
                None => client.channel_section(&id, tab).await,
            }
        });
        let shelf = self.shelf.clone();
        self.part = Some(cx.spawn(async move |this, cx| {
            let loaded = join(request).await;
            this.update(cx, |this, cx| {
                if this.shelf != shelf || this.tab != tab {
                    return;
                }
                this.loading = false;
                match crate::settled(loaded, cx) {
                    Ok(section) => match tab {
                        Tab::Posts => {
                            this.posts_read = true;
                            for post in section.posts.items {
                                if !this.posts.items.contains(&post) {
                                    this.posts.items.push(post);
                                }
                            }
                            this.posts.more = section.posts.more;
                        }
                        Tab::Playlists | Tab::Releases | Tab::Videos => {
                            let kept = this.playlists.entry(tab).or_default();
                            for playlist in section.playlists.items {
                                if !kept.items.contains(&playlist) {
                                    kept.items.push(playlist);
                                }
                            }
                            kept.more = section.playlists.more;
                        }
                    },
                    Err(reason) => {
                        log::warn!("video: cannot read that part of the channel: {reason}");
                        // Kept as read, so an empty part is shown instead of asked for forever.
                        match tab {
                            Tab::Posts => this.posts_read = true,
                            Tab::Playlists | Tab::Releases | Tab::Videos => {
                                this.playlists.entry(tab).or_default();
                            }
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
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

    /// The wide picture across the top of a channel.
    pub fn banner(&self) -> Option<&str> {
        self.banner.as_deref()
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
        match self.tab {
            Tab::Videos => self.more.is_some(),
            Tab::Posts => self.posts.more.is_some(),
            Tab::Playlists | Tab::Releases => self
                .playlists
                .get(&self.tab)
                .is_some_and(|page| page.more.is_some()),
        }
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
        self.banner = None;
        self.description.clear();
        self.videos.clear();
        self.more = None;
        self.error = None;
        self.loading = true;
        self.tabs.clear();
        self.tab = Tab::Videos;
        self.playlists.clear();
        self.posts = Page::default();
        self.posts_read = false;
        self.part = None;
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
                        banner: page.banner,
                        description: page.description,
                        videos: page.videos,
                        tabs: page.tabs,
                    }
                }
                Shelf::Playlist(id) => {
                    let page = client.playlist(&id).await?;
                    Listed {
                        title: page.title,
                        channel: None,
                        banner: None,
                        description: String::new(),
                        videos: page.videos,
                        tabs: Vec::new(),
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
                        this.banner = listed.banner;
                        this.description = listed.description;
                        this.tabs = listed.tabs;
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
        let further = match self.tab {
            Tab::Videos => None,
            Tab::Posts => Some(self.posts.more.take()),
            Tab::Playlists | Tab::Releases => Some(
                self.playlists
                    .get_mut(&self.tab)
                    .and_then(|page| page.more.take()),
            ),
        };
        if let Some(token) = further {
            if token.is_some() {
                self.load_part(token, cx);
            }
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
const SIGN_IN_URL: &str = "https://accounts.google.com/AccountChooser?service=youtube&continue=https%3A%2F%2Fwww.youtube.com%2Fsignin%3Faction_handle_signin%3Dtrue%26next%3Dhttps%253A%252F%252Fwww.youtube.com%252F";
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
    io: Io,
    /// The cookies of the Google session kept on this machine. While a guest profile is in
    /// use they stay here and no request carries them.
    cookies: Option<String>,
    /// The channels the Google account can act as, once YouTube listed them.
    identities: Vec<Identity>,
    /// The channel picked among them, none being the account's own. Unset until one is picked
    /// or YouTube's own choice is taken.
    page: Option<(usize, Option<String>)>,
    /// Counts every change of who the client acts as, so what is synced can follow.
    acting: u64,
    listing: Option<Task<()>>,
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
        // The marker beside the cookies says a guest profile was in use, so Google stays out.
        let away = path
            .as_ref()
            .is_some_and(|path| path.with_file_name(AWAY).exists());
        if let Some(cookies) = kept.clone().filter(|_| !away)
            && let Err(error) = client.sign_in(Some(cookies))
        {
            log::warn!("video: cannot use the kept account: {error:#}");
        }
        // An empty file stands for the account's own channel, a missing one for no choice yet.
        let page = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path.with_file_name("channel")).ok())
            .map(|kept| chosen(&kept));
        if let Some((user, page)) = &page {
            client.act_as(*user, page.clone());
        }
        let io = Io::global(cx);
        let account = cx.new(|cx| {
            let mut account = Self {
                client,
                path,
                window: None,
                failed: false,
                task: None,
                io,
                cookies: kept,
                identities: Vec::new(),
                page,
                acting: 0,
                listing: None,
            };
            if account.signed_in() {
                account.acting = 1;
                account.list(cx);
            }
            account
        });
        cx.set_global(Held(account.clone()));
        account
    }

    /// The app's account, once [`Account::install`] ran.
    pub fn entity(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<Held>().map(|held| held.0.clone())
    }

    /// Whether a Google profile is in use rather than a guest one.
    pub fn signed_in(&self) -> bool {
        self.client.signed_in()
    }

    /// Whether a Google session is kept on this machine, in use or not.
    pub fn kept(&self) -> bool {
        self.cookies.is_some()
    }

    /// Goes to the guest profiles and keeps the Google session for later. No request carries
    /// it from here on.
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        if !self.signed_in() {
            return;
        }
        self.client.sign_in(None).ok();
        self.client.act_as(0, None);
        if let Some(path) = &self.path
            && let Err(error) = write_private(&path.with_file_name(AWAY), b"")
        {
            log::warn!("video: cannot keep the guest profile in use: {error:#}");
        }
        self.acting += 1;
        cx.notify();
    }

    /// Comes back to the kept Google session as the channel last used.
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        if self.signed_in() {
            return;
        }
        let Some(cookies) = self.cookies.clone() else {
            return;
        };
        if let Err(error) = self.client.sign_in(Some(cookies)) {
            log::warn!("video: cannot use the kept account: {error:#}");
            return;
        }
        if let Some((user, page)) = &self.page {
            self.client.act_as(*user, page.clone());
        }
        if let Some(path) = &self.path {
            std::fs::remove_file(path.with_file_name(AWAY)).ok();
        }
        self.acting += 1;
        self.list(cx);
        cx.notify();
    }

    /// The channels the signed-in Google account can act as.
    pub fn identities(&self) -> &[Identity] {
        &self.identities
    }

    /// Whether `identity` is the channel the client acts as, which none is on a guest profile.
    pub fn acts_as(&self, identity: &Identity) -> bool {
        if !self.signed_in() {
            return false;
        }
        match &self.page {
            Some((user, page)) => *user == identity.user && *page == identity.page,
            None => identity.selected,
        }
    }

    /// A number that changes whenever the client starts acting as someone else.
    pub fn acting(&self) -> u64 {
        self.acting
    }

    /// Acts as another channel of the account from now on, and remembers it.
    pub fn choose(&mut self, identity: &Identity, cx: &mut Context<Self>) {
        if self.acts_as(identity) && self.page.is_some() {
            return;
        }
        self.resume(cx);
        if !self.signed_in() {
            return;
        }
        self.settle(identity.user, identity.page.clone());
        cx.notify();
    }

    /// Takes the channel `page` of the session's account `user` as the one in use and writes
    /// the choice down.
    fn settle(&mut self, user: usize, page: Option<String>) {
        self.client.act_as(user, page.clone());
        if let Some(path) = &self.path
            && let Err(error) = write_private(
                &path.with_file_name("channel"),
                format!("{user}:{}", page.as_deref().unwrap_or_default()).as_bytes(),
            )
        {
            log::warn!("video: cannot keep the chosen channel: {error:#}");
        }
        self.page = Some((user, page));
        self.acting += 1;
    }

    /// Asks YouTube which channels the account can act as. With no choice made yet the one
    /// YouTube has in use is taken, and a toast says there are others to pick from.
    fn list(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let request = self.io.spawn(async move { client.identities().await });
        self.listing = Some(cx.spawn(async move |this, cx| {
            let listed = join(request).await;
            this.update(cx, |this, cx| {
                match crate::settled(listed, cx) {
                    Ok(identities) => {
                        this.identities = identities;
                        // A guest profile taken while the list was on its way leaves the
                        // remembered channel as it is.
                        if !this.signed_in() {
                            return cx.notify();
                        }
                        let known = this.page.is_some()
                            && this
                                .identities
                                .iter()
                                .any(|identity| this.acts_as(identity));
                        // A kept choice whose account did not answer this time is left alone,
                        // since the list then says nothing about its channels.
                        let unheard = this.page.as_ref().is_some_and(|(user, _)| {
                            !this.identities.iter().any(|held| held.user == *user)
                        });
                        if !known && !unheard && !this.identities.is_empty() {
                            let taken = this
                                .identities
                                .iter()
                                .find(|identity| identity.selected)
                                .or_else(|| this.identities.first())
                                .map(|identity| (identity.user, identity.page.clone()))
                                .unwrap_or_default();
                            this.settle(taken.0, taken.1);
                            if this.identities.len() > 1 {
                                Toasts::show(Outcome::Info, "toast-video-channels", cx);
                            }
                        }
                    }
                    Err(reason) => {
                        log::warn!("video: cannot list the account's channels: {reason}");
                    }
                }
                cx.notify();
            })
            .ok();
        }));
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
                        this.keep(cookies, cx);
                        if this.signed_in() {
                            this.acting += 1;
                            this.list(cx);
                        }
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
        self.client.act_as(0, None);
        if let Some(path) = &self.path {
            std::fs::remove_file(path).ok();
            std::fs::remove_file(path.with_file_name("channel")).ok();
            std::fs::remove_file(path.with_file_name(AWAY)).ok();
        }
        self.cookies = None;
        self.identities.clear();
        self.page = None;
        self.listing = None;
        self.acting += 1;
        forget_channels(cx);
        cx.notify();
    }

    /// Hands the cookies to the client and writes them where the next launch finds them. They
    /// take the place of a session kept before, whose channels and what they kept are
    /// forgotten, since the new session may be another person's.
    fn keep(&mut self, cookies: String, cx: &mut Context<Self>) {
        if let Err(error) = self.client.sign_in(Some(cookies.clone())) {
            log::warn!("video: cannot sign in: {error:#}");
            self.failed = true;
            return;
        }
        if self.cookies.is_some() {
            self.client.act_as(0, None);
            self.identities.clear();
            self.page = None;
            if let Some(path) = &self.path {
                std::fs::remove_file(path.with_file_name("channel")).ok();
            }
            forget_channels(cx);
        }
        self.cookies = Some(cookies.clone());
        let Some(path) = &self.path else {
            return;
        };
        std::fs::remove_file(path.with_file_name(AWAY)).ok();
        if let Err(error) = write_private(path, cookies.as_bytes()) {
            log::warn!("video: cannot keep the account: {error:#}");
        }
    }
}

/// Deletes what every YouTube channel of the Google session kept on this machine, when the
/// session is signed out of or replaced.
fn forget_channels(cx: &mut App) {
    if let Some(kept) = Subscriptions::entity(cx) {
        kept.update(cx, |kept, cx| kept.forget_channels(cx));
    }
}

/// Writes a file only its owner can read, making its folder first.
/// Tells the assistant Mistral turned its key down, when `reason` says so.
/// The key and chapter model in use, told apart without holding the key.
fn tried(cx: &App) -> Option<String> {
    let assistant = Assistant::entity(cx)?.read(cx);
    Some(format!(
        "{:?} {} {}",
        assistant.hint(),
        assistant.refused(),
        assistant.chapter_model()
    ))
}

fn refused(reason: &str, cx: &mut App) {
    if reason.contains("API key")
        && let Some(assistant) = Assistant::entity(cx)
    {
        assistant.update(cx, |assistant, cx| assistant.mark_refused(cx));
    }
}

pub(crate) fn write_private(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
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

/// How many of the latest videos watched new ones are looked for next to.
const FIND_SEEDS: usize = 8;
/// How many videos are found, and how many of them one channel may have.
const FIND_CAP: usize = 60;
const FIND_PER_CHANNEL: usize = 2;
/// How much less a video counts for each step its seed lies back in the history.
const FIND_FADE: f32 = 0.85;
/// How much less a video counts when its channel was watched before, since it is not new.
const FIND_KNOWN: f32 = 0.4;

/// The file the first profile is kept in, the folder of the others and the file naming the
/// one in use.
const FIRST_PROFILE: usize = 1;
const VIDEOS: &str = "videos.json";
const PROFILES: &str = "profiles";
const PROFILE: &str = "profile";
/// The folder what each YouTube channel of a Google account keeps is in, apart from the guests.
const CHANNELS: &str = "google";
/// The file beside the cookies that says a guest profile is in use while they are kept.
const AWAY: &str = "guest";

/// The one [`Subscriptions`] of the app, where the chrome finds it.
struct Following(Entity<Subscriptions>);

impl Global for Following {}

/// Where a profile is kept: the first in the file the app always had, the others beside it.
fn profile_path(folder: &std::path::Path, profile: usize) -> PathBuf {
    match profile == FIRST_PROFILE {
        true => folder.join(VIDEOS),
        false => folder.join(PROFILES).join(format!("{profile}.json")),
    }
}

/// Where a YouTube channel of a Google account keeps what it follows and watched.
fn channel_path(folder: &std::path::Path, user: usize, page: Option<&str>) -> PathBuf {
    let page: String = page
        .unwrap_or("own")
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    folder.join(CHANNELS).join(format!("{user}-{page}.json"))
}

/// Reads what a profile kept, nothing when it has no file yet. A guest profile leaves out the
/// channels an account once brought into it.
fn read_kept(path: &std::path::Path, guest: bool) -> Kept {
    let mut kept: Kept = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if guest {
        let synced = std::mem::take(&mut kept.synced);
        kept.channels
            .retain(|channel| !synced.contains(&channel.id));
    }
    kept
}

/// What is kept on disk between runs: the channels followed and the videos watched, newest first.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Kept {
    #[serde(default)]
    channels: Vec<Channel>,
    #[serde(default)]
    watched: Vec<Video>,
    #[serde(default)]
    later: Vec<Video>,
    #[serde(default)]
    liked: Vec<String>,
    /// How far each video was watched and how long it is, both in seconds.
    #[serde(default)]
    places: HashMap<String, (f64, f64)>,
    /// The channels that came from the YouTube account rather than from following them here.
    #[serde(default)]
    synced: Vec<String>,
}

/// The channels the user follows, the feed of their newest uploads and the videos watched, for
/// the profile in use. A guest profile and a YouTube channel of a Google account each keep
/// their own, and neither ever shows the other's.
pub struct Subscriptions {
    client: Client,
    io: Io,
    /// The folder the profiles are kept in.
    folder: PathBuf,
    /// The profile in use, counted from one, and every profile there is, in order.
    profile: usize,
    profiles: Vec<usize>,
    path: PathBuf,
    kept: Kept,
    feed: Vec<Video>,
    loaded: bool,
    loading: bool,
    task: Option<Task<()>>,
    /// Videos from channels not followed, picked from what was watched.
    found: Vec<Video>,
    /// How many videos had been watched when `found` was picked, so more watching picks again.
    found_for: Option<usize>,
    /// How many of `found`, from its start, further videos were already looked for beside.
    dug: usize,
    finding: bool,
    find: Option<Task<()>>,
    /// What YouTube's home page recommends to the signed-in account.
    recommended: Vec<Video>,
    syncing: Option<Task<()>>,
}

impl Subscriptions {
    /// The app's one set of followed channels, made on the first call and shared by every
    /// window after it.
    pub fn shared(client: Client, io: Io, cx: &mut App) -> Entity<Self> {
        if let Some(following) = cx.try_global::<Following>() {
            return following.0.clone();
        }
        let made = cx.new(|_| Self::new(client, io));
        cx.set_global(Following(made.clone()));
        made
    }

    /// The app's followed channels, once [`Subscriptions::shared`] ran.
    pub fn entity(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<Following>().map(|held| held.0.clone())
    }

    pub fn new(client: Client, io: Io) -> Self {
        let folder = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("youtube-plus");
        let mut profiles: Vec<usize> = std::fs::read_dir(folder.join(PROFILES))
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok()?.path().file_stem()?.to_str()?.parse().ok())
            .filter(|profile| *profile > FIRST_PROFILE)
            .collect();
        profiles.push(FIRST_PROFILE);
        profiles.sort_unstable();
        profiles.dedup();
        let profile = std::fs::read_to_string(folder.join(PROFILE))
            .ok()
            .and_then(|kept| kept.trim().parse().ok())
            .filter(|profile| profiles.contains(profile))
            .unwrap_or(FIRST_PROFILE);
        let path = match client.acting() {
            Some((user, page)) => channel_path(&folder, user, page.as_deref()),
            None => profile_path(&folder, profile),
        };
        let kept = read_kept(&path, !client.signed_in());
        Self {
            client,
            io,
            folder,
            profile,
            profiles,
            path,
            kept,
            feed: Vec::new(),
            loaded: false,
            loading: false,
            task: None,
            found: Vec::new(),
            found_for: None,
            dug: 0,
            finding: false,
            find: None,
            recommended: Vec::new(),
            syncing: None,
        }
    }

    /// The guest profile in use, counted from one, or the one last used while a Google
    /// profile is.
    pub fn profile(&self) -> usize {
        self.profile
    }

    /// Deletes what every YouTube channel kept here and reads the profile in use afresh, so
    /// nothing of a Google session outlasts it.
    pub(crate) fn forget_channels(&mut self, cx: &mut Context<Self>) {
        match std::fs::remove_dir_all(self.folder.join(CHANNELS)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                log::warn!("video: cannot delete what the account kept: {error:#}");
            }
            _ => {}
        }
        self.load(cx);
    }

    /// Where the profile in use is kept: the YouTube channel acted as, or the guest profile.
    fn place(&self) -> PathBuf {
        match self.client.acting() {
            Some((user, page)) => channel_path(&self.folder, user, page.as_deref()),
            None => profile_path(&self.folder, self.profile),
        }
    }

    /// Every profile kept on this machine, in order.
    pub fn profiles(&self) -> &[usize] {
        &self.profiles
    }

    /// Uses another profile from now on: its followed channels, history, saved videos and
    /// places. The one left stays on disk as it was.
    pub fn use_profile(&mut self, profile: usize, cx: &mut Context<Self>) {
        if !self.profiles.contains(&profile) {
            return;
        }
        if profile != self.profile || self.place() != self.path {
            self.enter(profile, cx);
        }
    }

    /// Starts an empty profile and uses it.
    pub fn add_profile(&mut self, cx: &mut Context<Self>) {
        let profile = self.profiles.last().map_or(FIRST_PROFILE, |last| last + 1);
        self.profiles.push(profile);
        self.enter(profile, cx);
        self.save();
    }

    /// Deletes a guest profile with all it kept. The first one cannot be deleted, and it is
    /// the one used next when the profile in use goes.
    pub fn remove_profile(&mut self, gone: usize, cx: &mut Context<Self>) {
        if gone == FIRST_PROFILE || !self.profiles.contains(&gone) {
            return;
        }
        self.profiles.retain(|profile| *profile != gone);
        if self.profile == gone {
            self.enter(FIRST_PROFILE, cx);
        }
        cx.notify();
        if let Err(error) = std::fs::remove_file(profile_path(&self.folder, gone)) {
            log::warn!("video: cannot delete the profile: {error:#}");
        }
    }

    /// Takes `profile` as the guest profile in use and remembers it for next launch.
    fn enter(&mut self, profile: usize, cx: &mut Context<Self>) {
        self.profile = profile;
        if let Err(error) = std::fs::create_dir_all(&self.folder)
            .and_then(|()| std::fs::write(self.folder.join(PROFILE), profile.to_string()))
        {
            log::warn!("video: cannot keep the chosen profile: {error:#}");
        }
        self.load(cx);
    }

    /// Reads what the profile in use kept in place of what is held, and loads its feed.
    fn load(&mut self, cx: &mut Context<Self>) {
        self.path = self.place();
        self.kept = read_kept(&self.path, !self.client.signed_in());
        self.feed.clear();
        self.loaded = false;
        self.loading = false;
        self.task = None;
        self.found.clear();
        self.found_for = None;
        self.dug = 0;
        self.finding = false;
        self.find = None;
        self.sync(cx);
        self.refresh(cx);
        // A screen showing the videos to find stays open, so they are picked for this history.
        self.find_again(cx);
        cx.notify();
    }

    /// What YouTube recommends to the signed-in account, empty for a guest.
    pub fn recommended(&self) -> &[Video] {
        &self.recommended
    }

    /// Follows what the YouTube channel in use is subscribed to and reads its recommendations,
    /// after changing to what that channel keeps when another profile was held. Channels
    /// followed here stay, channels a former sync brought and the account no longer has go. A
    /// guest profile asks YouTube nothing.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        if self.place() != self.path {
            return self.load(cx);
        }
        self.recommended.clear();
        self.syncing = None;
        cx.notify();
        if !self.client.signed_in() {
            return;
        }
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            let (channels, recommended) =
                futures::future::join(client.subscribed(), client.recommended()).await;
            Ok((channels, recommended))
        });
        self.syncing = Some(cx.spawn(async move |this, cx| {
            let synced = join(request).await;
            this.update(cx, |this, cx| {
                let Ok((channels, recommended)) = crate::settled(synced, cx) else {
                    return;
                };
                match recommended {
                    Ok(page) => this.recommended = page.items,
                    Err(error) => log::warn!("video: cannot read the recommendations: {error:#}"),
                }
                match channels {
                    Ok(channels) => {
                        let gone = std::mem::take(&mut this.kept.synced);
                        this.kept.channels.retain(|kept| {
                            !gone.contains(&kept.id)
                                || channels.iter().any(|channel| channel.id == kept.id)
                        });
                        // A channel followed here before the account brought it stays a local
                        // follow, so a later sync without it leaves it alone.
                        for channel in channels {
                            let local = this.follows(&channel.id) && !gone.contains(&channel.id);
                            if local {
                                continue;
                            }
                            this.kept.synced.push(channel.id.clone());
                            if !this.follows(&channel.id) {
                                this.kept.channels.push(channel);
                            }
                        }
                        this.save();
                        this.refresh(cx);
                    }
                    Err(error) => log::warn!("video: cannot read the subscriptions: {error:#}"),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Videos to find new channels by, the likeliest first.
    pub fn found(&self) -> &[Video] {
        &self.found
    }

    pub fn is_finding(&self) -> bool {
        self.finding
    }

    /// Picks the videos to find unless they are picked for the history as it stands.
    pub fn find(&mut self, cx: &mut Context<Self>) {
        if !self.finding && self.found_for != Some(self.kept.watched.len()) {
            self.find_again(cx);
        }
    }

    /// Picks the videos to find afresh: what YouTube puts next to the latest videos watched,
    /// ranked by how many of them it sits next to and how lately they were watched.
    pub fn find_again(&mut self, cx: &mut Context<Self>) {
        let seeds: Vec<String> = self
            .kept
            .watched
            .iter()
            .take(FIND_SEEDS)
            .map(|video| video.id.clone())
            .collect();
        self.found_for = Some(self.kept.watched.len());
        self.dug = 0;
        if seeds.is_empty() {
            self.found.clear();
            cx.notify();
            return;
        }
        self.finding = true;
        cx.notify();
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            let pages = seeds.iter().map(|id| client.watch(id));
            let beside: Vec<Vec<Video>> = futures::future::join_all(pages)
                .await
                .into_iter()
                .map(|page| page.map(|watch| watch.related).unwrap_or_default())
                .collect();
            anyhow::Ok(beside)
        });
        self.find = Some(cx.spawn(async move |this, cx| {
            let beside = join(request).await;
            this.update(cx, |this, cx| {
                this.finding = false;
                match beside {
                    Ok(beside) => this.found = rank(beside, &this.kept),
                    Err(error) => log::warn!("video: cannot find new videos: {error:#}"),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Finds further videos, next to the ones found that were not yet looked beside, and puts
    /// them after those. Does nothing while a search runs or once every one was looked beside.
    pub fn find_more(&mut self, cx: &mut Context<Self>) {
        if self.finding {
            return;
        }
        let seeds: Vec<String> = self
            .found
            .iter()
            .skip(self.dug)
            .take(FIND_SEEDS)
            .map(|video| video.id.clone())
            .collect();
        if seeds.is_empty() {
            return;
        }
        self.dug += seeds.len();
        self.finding = true;
        cx.notify();
        let client = self.client.clone();
        let request = self.io.spawn(async move {
            let pages = seeds.iter().map(|id| client.watch(id));
            let beside: Vec<Vec<Video>> = futures::future::join_all(pages)
                .await
                .into_iter()
                .map(|page| page.map(|watch| watch.related).unwrap_or_default())
                .collect();
            anyhow::Ok(beside)
        });
        self.find = Some(cx.spawn(async move |this, cx| {
            let beside = join(request).await;
            this.update(cx, |this, cx| {
                this.finding = false;
                match beside {
                    Ok(beside) => {
                        let further: Vec<Video> = rank(beside, &this.kept)
                            .into_iter()
                            .filter(|video| this.found.iter().all(|held| held.id != video.id))
                            .collect();
                        this.found.extend(further);
                    }
                    Err(error) => log::warn!("video: cannot find more videos: {error:#}"),
                }
                cx.notify();
            })
            .ok();
        }));
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

    /// The share of the video watched so far, when it was started and not finished.
    pub fn progress(&self, video: &str) -> Option<f32> {
        let (at, length) = *self.kept.places.get(video)?;
        let share = (at / length) as f32;
        (share >= RESUME_FROM).then_some(share.min(1.))
    }

    /// The second a video left unfinished is taken up again at.
    pub fn resume(&self, video: &str) -> Option<f64> {
        let (at, _) = *self.kept.places.get(video)?;
        self.progress(video)
            .filter(|share| *share < RESUME_UNTIL)
            .map(|_| at)
    }

    /// Records how far the video was watched.
    pub fn set_place(&mut self, video: &str, at: f64, length: f64, cx: &mut Context<Self>) {
        if self.kept.places.len() > HISTORY_CAP * 2 {
            let watched = &self.kept.watched;
            self.kept
                .places
                .retain(|id, _| watched.iter().any(|kept| kept.id == *id));
        }
        self.kept.places.insert(video.to_owned(), (at, length));
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

    /// Whether the video was liked on this machine.
    pub fn liked(&self, video: &str) -> bool {
        self.kept.liked.iter().any(|kept| kept == video)
    }

    /// Remembers that the video is liked, or forgets it.
    pub fn set_liked(&mut self, video: &str, liked: bool, cx: &mut Context<Self>) {
        self.kept.liked.retain(|kept| kept != video);
        if liked {
            self.kept.liked.insert(0, video.to_owned());
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

/// The videos to find new channels by. `beside` holds, for each of the latest videos watched
/// and the latest first, what YouTube puts next to it. A video counts for more the more of
/// them it sits next to and the later they were watched, and for less when its channel was
/// watched before. Videos watched and channels followed are left out, and one channel gets
/// few places so that many are met.
fn rank(beside: Vec<Vec<Video>>, kept: &Kept) -> Vec<Video> {
    let channel = |video: &Video| video.channel.as_ref().map(|channel| channel.id.clone());
    let known: std::collections::HashSet<String> =
        kept.watched.iter().filter_map(channel).collect();
    let mut scores: HashMap<String, (f32, Video)> = HashMap::new();
    let mut weight = 1.;
    for videos in beside {
        for (place, video) in videos.into_iter().enumerate() {
            let followed =
                channel(&video).is_some_and(|id| kept.channels.iter().any(|kept| kept.id == id));
            if followed || kept.watched.iter().any(|kept| kept.id == video.id) {
                continue;
            }
            // What YouTube lists first it thinks closest.
            let near = 1. / (1. + place as f32 * 0.1);
            let familiar = match channel(&video).is_some_and(|id| known.contains(&id)) {
                true => FIND_KNOWN,
                false => 1.,
            };
            let score = weight * near * familiar;
            scores
                .entry(video.id.clone())
                .and_modify(|held| held.0 += score)
                .or_insert((score, video));
        }
        weight *= FIND_FADE;
    }
    let mut ranked: Vec<(f32, Video)> = scores.into_values().collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    let mut places: HashMap<String, usize> = HashMap::new();
    ranked
        .into_iter()
        .map(|(_, video)| video)
        .filter(|video| {
            let Some(id) = channel(video) else {
                return true;
            };
            let taken = places.entry(id).or_default();
            *taken += 1;
            *taken <= FIND_PER_CHANNEL
        })
        .take(FIND_CAP)
        .collect()
}

/// Reads a kept choice of channel: the session's account, a colon and the channel, where a
/// file from before accounts were numbered holds the channel alone.
fn chosen(kept: &str) -> (usize, Option<String>) {
    let kept = kept.trim();
    let (user, page) = match kept.split_once(':') {
        Some((user, page)) => (user.parse().unwrap_or_default(), page),
        None => (0, kept),
    };
    (user, Some(page.to_owned()).filter(|page| !page.is_empty()))
}
