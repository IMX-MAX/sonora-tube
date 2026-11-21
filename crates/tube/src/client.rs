//! The requests: one POST per page, each answered with a JSON tree `parse` reads.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};

use crate::parse;
use crate::{ChannelPage, Comments, Found, Page, PlaylistPage, Video, Watch};

const HOST: &str = "https://www.youtube.com/youtubei/v1";
/// The web client YouTube's own site reports. The answers this crate parses are that client's.
const CLIENT_NAME: &str = "WEB";
const CLIENT_VERSION: &str = "2.20260707.00.00";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";
/// The `params` that open a channel on its Videos tab.
const VIDEOS_TAB: &str = "EgZ2aWRlb3PyBgQKAjoA";

/// Reads YouTube without an account. Cheap to clone, and every method is one round trip.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    language: String,
    region: String,
    /// Whether Shorts are left out of every list, shared by the clones of a client.
    no_shorts: Arc<AtomicBool>,
}

impl Client {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            language: "en".to_owned(),
            region: "US".to_owned(),
            no_shorts: Arc::default(),
        }
    }

    /// Leaves Shorts out of every list fetched from now on, or lists them as videos again.
    pub fn hide_shorts(&self, hidden: bool) {
        self.no_shorts.store(hidden, Ordering::Relaxed);
    }

    /// Searches videos, channels and playlists together.
    pub async fn search(&self, query: &str) -> Result<Page<Found>> {
        let answer = self.call("search", json!({ "query": query })).await?;
        Ok(parse::listing(&answer))
    }

    /// The next stretch of a search.
    pub async fn search_more(&self, token: &str) -> Result<Page<Found>> {
        let answer = self
            .call("search", json!({ "continuation": token }))
            .await?;
        Ok(parse::listing(&answer))
    }

    /// What surrounds the player: the title, the channel, the description and what to watch next.
    pub async fn watch(&self, id: &str) -> Result<Watch> {
        let answer = self.call("next", json!({ "videoId": id })).await?;
        parse::watch(id, &answer).context("cannot read the watch page")
    }

    /// A page of comments or of replies, from the token a [`Watch`] or a comment carries.
    pub async fn comments(&self, token: &str) -> Result<Comments> {
        let answer = self.call("next", json!({ "continuation": token })).await?;
        Ok(parse::comments(&answer))
    }

    /// A channel and its newest uploads.
    pub async fn channel(&self, id: &str) -> Result<ChannelPage> {
        let answer = self
            .call("browse", json!({ "browseId": id, "params": VIDEOS_TAB }))
            .await?;
        parse::channel(id, &answer).context("cannot read the channel")
    }

    /// A playlist and its first videos.
    pub async fn playlist(&self, id: &str) -> Result<PlaylistPage> {
        let browse = match id.starts_with("VL") {
            true => id.to_owned(),
            false => format!("VL{id}"),
        };
        let answer = self.call("browse", json!({ "browseId": browse })).await?;
        Ok(parse::playlist(id, &answer))
    }

    /// The next stretch of a channel's uploads or of a playlist.
    pub async fn videos_more(&self, token: &str) -> Result<Page<Video>> {
        let answer = self
            .call("browse", json!({ "continuation": token }))
            .await?;
        Ok(parse::videos(&answer))
    }

    async fn call(&self, endpoint: &str, mut body: Value) -> Result<Value> {
        body["context"] = json!({
            "client": {
                "clientName": CLIENT_NAME,
                "clientVersion": CLIENT_VERSION,
                "hl": self.language,
                "gl": self.region,
            }
        });
        let response = self
            .http
            .post(format!("{HOST}/{endpoint}?prettyPrint=false"))
            .header("User-Agent", USER_AGENT)
            .header("Origin", "https://www.youtube.com")
            .header("X-Youtube-Client-Name", "1")
            .header("X-Youtube-Client-Version", CLIENT_VERSION)
            .json(&body)
            .send()
            .await
            .context("cannot reach youtube")?;
        let status = response.status();
        ensure!(status.is_success(), "youtube answered {status}");
        let mut answer = response
            .json()
            .await
            .context("cannot read youtube's answer")?;
        if self.no_shorts.load(Ordering::Relaxed) {
            parse::drop_shorts(&mut answer);
        }
        Ok(answer)
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}
