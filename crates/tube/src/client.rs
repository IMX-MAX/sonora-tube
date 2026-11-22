//! The requests: one POST per page, each answered with a JSON tree `parse` reads.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};
use sha1::{Digest as _, Sha1};

use crate::parse;
use crate::{ChannelPage, Comments, Found, Line, Page, PlaylistPage, Video, Watch};

const ORIGIN: &str = "https://www.youtube.com";
/// The cookies that tell a signed-in session from a guest one, the first found signs requests.
pub const PROOF: &[&str] = &["SAPISID", "__Secure-3PAPISID"];
const HOST: &str = "https://www.youtube.com/youtubei/v1";
/// The web client YouTube's own site reports. The answers this crate parses are that client's.
const CLIENT_NAME: &str = "WEB";
const CLIENT_VERSION: &str = "2.20260707.00.00";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";
/// The iPhone app, a client YouTube still hands readable caption tracks to without a token.
const APP: Guise = Guise {
    name: "IOS",
    version: "20.10.4",
    number: "5",
    agent: "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
};
const WEB: Guise = Guise {
    name: CLIENT_NAME,
    version: CLIENT_VERSION,
    number: "1",
    agent: USER_AGENT,
};
/// How many times a caption file is asked for before giving up on it.
const TRIES: u32 = 5;
/// How long the first retry of a caption file waits, doubling with each one after.
const RETRY: Duration = Duration::from_millis(300);
/// The `params` that open a channel on its Videos tab.
const VIDEOS_TAB: &str = "EgZ2aWRlb3PyBgQKAjoA";

/// The client a request says it comes from.
struct Guise {
    name: &'static str,
    version: &'static str,
    /// The number YouTube knows the client by, sent as a header.
    number: &'static str,
    agent: &'static str,
}

/// Reads YouTube, as a guest until [`Client::sign_in`] hands it an account's cookies. Cheap to clone, and every method is one round trip.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    language: String,
    region: String,
    /// Whether Shorts are left out of every list, shared by the clones of a client.
    no_shorts: Arc<AtomicBool>,
    /// The `Cookie` header of the signed-in account, shared by the clones of a client.
    cookies: Arc<RwLock<Option<String>>>,
}

impl Client {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            language: "en".to_owned(),
            region: "US".to_owned(),
            no_shorts: Arc::default(),
            cookies: Arc::default(),
        }
    }

    /// Signs every later request with an account's `Cookie` header, or with none to go back to
    /// a guest. It fails when the cookies carry no proof of a session.
    pub fn sign_in(&self, cookies: Option<String>) -> Result<()> {
        if let Some(cookies) = &cookies {
            secret(cookies).context("the cookies carry no signed-in session")?;
        }
        *self
            .cookies
            .write()
            .unwrap_or_else(|held| held.into_inner()) = cookies;
        Ok(())
    }

    /// Whether requests go out as an account rather than as a guest.
    pub fn signed_in(&self) -> bool {
        self.account().is_some()
    }

    fn account(&self) -> Option<String> {
        self.cookies
            .read()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    }

    /// Likes a video as the signed-in account, or takes the like back.
    pub async fn like(&self, id: &str, liked: bool) -> Result<()> {
        ensure!(self.signed_in(), "liking needs an account");
        let endpoint = match liked {
            true => "like/like",
            false => "like/removelike",
        };
        self.call(endpoint, json!({ "target": { "videoId": id } }))
            .await?;
        Ok(())
    }

    /// Posts a comment under a video, with the `write` its first page of [`Comments`] carried.
    pub async fn comment(&self, write: &str, words: &str) -> Result<()> {
        ensure!(self.signed_in(), "commenting needs an account");
        let answer = self
            .call(
                "comment/create_comment",
                json!({ "createCommentParams": write, "commentText": words }),
            )
            .await?;
        let status = answer["actionResult"]["status"].as_str();
        ensure!(
            status.is_none_or(|status| status == "STATUS_SUCCEEDED"),
            "youtube refused the comment"
        );
        Ok(())
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

    /// The lines of a video's captions, in the client's language where the video has them and
    /// in its first language otherwise. A video without captions has no lines.
    pub async fn transcript(&self, id: &str) -> Result<Vec<Line>> {
        let answer = self
            .call_as(&APP, "player", json!({ "videoId": id }))
            .await?;
        let Some(track) = parse::caption_track(&answer, &self.language) else {
            return Ok(Vec::new());
        };
        let address = format!("{track}&fmt=json3");
        let mut wait = RETRY;
        let mut tries = TRIES;
        let response = loop {
            let response = self
                .http
                .get(&address)
                .header("User-Agent", APP.agent)
                .send()
                .await
                .context("cannot reach youtube")?;
            tries -= 1;
            // YouTube turns away a share of caption requests at random, and the next one passes.
            if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS || tries == 0 {
                break response;
            }
            tokio::time::sleep(wait).await;
            wait *= 2;
        };
        let status = response.status();
        ensure!(status.is_success(), "youtube answered {status}");
        let timed = response
            .json()
            .await
            .context("cannot read the transcript")?;
        Ok(parse::transcript(&timed))
    }

    async fn call(&self, endpoint: &str, body: Value) -> Result<Value> {
        self.call_as(&WEB, endpoint, body).await
    }

    async fn call_as(&self, guise: &Guise, endpoint: &str, mut body: Value) -> Result<Value> {
        body["context"] = json!({
            "client": {
                "clientName": guise.name,
                "clientVersion": guise.version,
                "hl": self.language,
                "gl": self.region,
            }
        });
        let mut request = self
            .http
            .post(format!("{HOST}/{endpoint}?prettyPrint=false"))
            .header("User-Agent", guise.agent)
            .header("Origin", ORIGIN)
            .header("X-Youtube-Client-Name", guise.number)
            .header("X-Youtube-Client-Version", guise.version);
        // Only the web client is the one the account signed in to.
        let account = self.account().filter(|_| guise.name == CLIENT_NAME);
        if let Some((cookies, key)) = account
            .as_deref()
            .and_then(|held| Some((held, secret(held)?)))
        {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs());
            request = request
                .header("Cookie", cookies)
                .header("Authorization", proof(now, key))
                .header("X-Origin", ORIGIN)
                .header("X-Goog-AuthUser", "0");
        }
        let response = request
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

/// The value of the first proof cookie in a `Cookie` header.
fn secret(cookies: &str) -> Option<&str> {
    PROOF.iter().find_map(|name| {
        cookies
            .split(';')
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(held, value)| held == name && !value.is_empty())
            .map(|(_, value)| value)
    })
}

/// The `Authorization` header YouTube's own pages sign a request with at second `now`.
fn proof(now: u64, key: &str) -> String {
    let digest = Sha1::digest(format!("{now} {key} {ORIGIN}"));
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("SAPISIDHASH {now}_{hex}")
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}
