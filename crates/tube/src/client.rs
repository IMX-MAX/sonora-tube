//! The requests: one POST per page, each answered with a JSON tree `parse` reads.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};
use sha1::{Digest as _, Sha1};

use crate::parse;
use crate::{
    Caption, Captions, Channel, ChannelPage, Comments, EndCard, Found, Identity, Page,
    PlaylistPage, Section, Streams, Tab, Video, Watch,
};

const ORIGIN: &str = "https://www.youtube.com";
/// The cookies that tell a signed-in session from a guest one, the first found signs requests.
pub const PROOF: &[&str] = &["SAPISID", "__Secure-3PAPISID"];
const HOST: &str = "https://www.youtube.com/youtubei/v1";
/// The web client YouTube's own site reports. The answers this crate parses are that client's.
const CLIENT_NAME: &str = "WEB";
/// How many Google accounts of one session are asked for their channels.
const ACCOUNTS: usize = 8;
/// The lists of channels YouTube's account switcher asks for, the usual one first.
const SWITCHERS: [&str; 2] = [
    "ACCOUNTS_LIST_REQUEST_TYPE_ACCOUNT_SWITCHER",
    "ACCOUNTS_LIST_REQUEST_TYPE_CHANNEL_SWITCHER",
];
const CLIENT_VERSION: &str = "2.20260707.00.00";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";
/// The iPhone app, a client YouTube still hands readable caption tracks to without a token.
/// Who a request is made as: a Google account of the session and a channel under it.
#[derive(Clone, Default)]
struct Acting {
    user: usize,
    page: Option<String>,
}

const APP: Guise = Guise {
    name: "IOS",
    version: "20.10.4",
    number: "5",
    agent: "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)",
    device: &[],
};
/// The headset app, the client YouTube hands a playable stream list to without a token, as
/// long as the request names a visitor.
const HEADSET: Guise = Guise {
    name: "VISIONOS",
    version: "1.02",
    number: "101",
    agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 \
         (KHTML, like Gecko) Version/26.0 Safari/605.1.15",
    device: &[
        ("deviceMake", "Apple"),
        ("deviceModel", "RealityDevice17,1"),
        ("osName", "visionOS"),
        ("osVersion", "26.5.23O471"),
    ],
};
const WEB: Guise = Guise {
    name: CLIENT_NAME,
    version: CLIENT_VERSION,
    number: "1",
    agent: USER_AGENT,
    device: &[],
};
/// How many times a caption file is asked for before giving up on it.
const TRIES: u32 = 5;
/// How long the first retry of a caption file waits, doubling with each one after.
const RETRY: Duration = Duration::from_millis(300);
/// The `params` that open a channel on its Videos tab.
/// How many pages of an account's subscriptions are read, each some hundred channels.
const SUBSCRIBED_PAGES: usize = 20;
const VIDEOS_TAB: &str = "EgZ2aWRlb3PyBgQKAjoA";
const SHORTS_TAB: &str = "EgZzaG9ydHPyBgUKA5oBAA%3D%3D";
const PLAYLISTS_TAB: &str = "EglwbGF5bGlzdHPyBgoKCEIGCgIQaCIA";
const POSTS_TAB: &str = "EgVwb3N0c_IGBAoCSgA%3D";
const RELEASES_TAB: &str = "EghyZWxlYXNlc_IGBQoDsgEA";

/// The client a request says it comes from.
struct Guise {
    name: &'static str,
    version: &'static str,
    /// The number YouTube knows the client by, sent as a header.
    number: &'static str,
    agent: &'static str,
    /// What the client says about the device it runs on.
    device: &'static [(&'static str, &'static str)],
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
    /// The Google account of the session and the channel under it that requests act as, kept
    /// together so that no request goes out with one of a former choice.
    acting: Arc<RwLock<Acting>>,
    /// The name YouTube gave this guest, which it wants back before it hands out streams.
    visitor: Arc<RwLock<Option<String>>>,
}

impl Client {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            language: "en".to_owned(),
            region: "US".to_owned(),
            no_shorts: Arc::default(),
            cookies: Arc::default(),
            acting: Arc::default(),
            visitor: Arc::default(),
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

    /// Makes every later request act as one channel of the signed-in account, a brand account
    /// by its page id or the account's own channel with none.
    pub fn act_as(&self, user: usize, page: Option<String>) {
        *self.acting.write().unwrap_or_else(|held| held.into_inner()) = Acting { user, page };
    }

    /// The Google account of the session and the channel of it requests go out as, nothing as
    /// a guest.
    pub fn acting(&self) -> Option<(usize, Option<String>)> {
        self.account()?;
        let acting = self.acting.read().unwrap_or_else(|held| held.into_inner());
        Some((acting.user, acting.page.clone()))
    }

    /// The channels the signed-in session can act as, over every Google account in it.
    pub async fn identities(&self) -> Result<Vec<Identity>> {
        ensure!(self.signed_in(), "no account is signed in");
        let mut found: Vec<Identity> = Vec::new();
        for user in 0..ACCOUNTS {
            let mut listed = Vec::new();
            // The switcher answers one of the two, depending on the account.
            for kind in SWITCHERS {
                let asked = self
                    .send(
                        &WEB,
                        "account/accounts_list",
                        json!({ "requestType": kind }),
                        Some(Acting { user, page: None }),
                    )
                    .await;
                match asked {
                    Ok(answer) => listed = parse::identities(&answer, user),
                    Err(error) => {
                        log::debug!("tube: account {user} names no channel: {error:#}");
                    }
                }
                if !listed.is_empty() {
                    break;
                }
            }
            let known = found.len();
            for identity in listed {
                let seen = found.iter().any(|held| {
                    held.page == identity.page
                        && held.name == identity.name
                        && held.detail == identity.detail
                });
                if !seen {
                    found.push(identity);
                }
            }
            // A number past the last account answers with nothing, or with the first again.
            if found.len() == known {
                break;
            }
        }
        Ok(found)
    }

    /// Every channel the account in use is subscribed to.
    pub async fn subscribed(&self) -> Result<Vec<Channel>> {
        ensure!(self.signed_in(), "no account is signed in");
        let mut answer = self
            .call("browse", json!({ "browseId": "FEchannels" }))
            .await?;
        let mut channels = Vec::new();
        for _ in 0..SUBSCRIBED_PAGES {
            let page = parse::listing(&answer);
            channels.extend(page.items.into_iter().filter_map(|found| match found {
                Found::Channel(channel) => Some(channel),
                Found::Video(_) | Found::Playlist(_) => None,
            }));
            let Some(token) = page.more else { break };
            answer = self
                .call("browse", json!({ "continuation": token }))
                .await?;
        }
        Ok(channels)
    }

    /// What YouTube's own home page recommends to the account in use.
    pub async fn recommended(&self) -> Result<Page<Video>> {
        ensure!(self.signed_in(), "no account is signed in");
        let answer = self
            .call("browse", json!({ "browseId": "FEwhat_to_watch" }))
            .await?;
        Ok(parse::videos(&answer))
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

    /// Likes a comment or takes the like back, with the `like` or `unlike` it carried.
    pub async fn comment_action(&self, action: &str) -> Result<()> {
        ensure!(self.signed_in(), "liking a comment needs an account");
        self.call(
            "comment/perform_comment_action",
            json!({ "actions": [action] }),
        )
        .await?;
        Ok(())
    }

    /// Replies to a comment, with the `reply` it carried.
    pub async fn reply(&self, reply: &str, words: &str) -> Result<()> {
        ensure!(self.signed_in(), "replying needs an account");
        let answer = self
            .call(
                "comment/create_comment_reply",
                json!({ "createReplyParams": reply, "commentText": words }),
            )
            .await?;
        let status = answer["actionResult"]["status"].as_str();
        ensure!(
            status.is_none_or(|status| status == "STATUS_SUCCEEDED"),
            "youtube refused the reply"
        );
        Ok(())
    }

    /// Leaves Shorts out of every list fetched from now on, or lists them as videos again.
    pub fn hide_shorts(&self, hidden: bool) {
        self.no_shorts.store(hidden, Ordering::Relaxed);
    }

    /// The stretches of `video` that SponsorBlock's viewers marked as paid or self-promoting.
    pub async fn sponsors(&self, video: &str) -> Result<Vec<crate::Sponsor>> {
        crate::sponsor::marked(&self.http, video).await
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
        let mut channel = parse::channel(id, &answer).context("cannot read the channel")?;
        if self.no_shorts.load(Ordering::Relaxed) {
            channel.tabs.retain(|tab| *tab != Tab::Shorts);
        }
        Ok(channel)
    }

    /// What a part of a channel other than its videos lists.
    pub async fn channel_section(&self, id: &str, tab: Tab) -> Result<Section> {
        let params = match tab {
            Tab::Videos => VIDEOS_TAB,
            Tab::Shorts => SHORTS_TAB,
            Tab::Playlists => PLAYLISTS_TAB,
            Tab::Posts => POSTS_TAB,
            Tab::Releases => RELEASES_TAB,
        };
        let answer = self
            .call("browse", json!({ "browseId": id, "params": params }))
            .await?;
        Ok(parse::section(&answer["contents"]))
    }

    /// The next stretch of a part of a channel.
    pub async fn section_more(&self, token: &str) -> Result<Section> {
        let answer = self
            .call("browse", json!({ "continuation": token }))
            .await?;
        Ok(parse::section(&answer))
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

    /// The lines of a video's captions and the languages they could be in. They come in the
    /// `wanted` language when one is given, and otherwise in the client's language where the
    /// video has them and in its first language where not. A video without captions has no lines.
    pub async fn captions(&self, id: &str, wanted: Option<&Caption>) -> Result<Captions> {
        let answer = self
            .call_as(&APP, "player", json!({ "videoId": id }))
            .await?;
        let choices = parse::caption_choices(&answer);
        let Some((track, shown)) = parse::caption_track(&answer, &self.language, wanted) else {
            return Ok(Captions::default());
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
        Ok(Captions {
            lines: parse::transcript(&timed),
            shown: Some(shown),
            choices,
        })
    }

    /// The qualities a video can be played at, best first, each a pair of addresses a player
    /// opens directly. That skips the page scraping a player would otherwise do before the
    /// first frame.
    pub async fn streams(&self, id: &str) -> Result<Streams> {
        if self.visitor().is_none() {
            self.call("visitor_id", json!({})).await?;
        }
        let answer = self
            .call_as(&HEADSET, "player", json!({ "videoId": id }))
            .await?;
        let list = answer["streamingData"]["hlsManifestUrl"]
            .as_str()
            .with_context(|| {
                let reason = answer["playabilityStatus"]["reason"]
                    .as_str()
                    .unwrap_or("no stream list");
                format!("youtube holds the streams back: {reason}")
            })?;
        let response = self
            .http
            .get(list)
            .header("User-Agent", HEADSET.agent)
            .send()
            .await
            .context("cannot reach youtube")?;
        let status = response.status();
        ensure!(status.is_success(), "youtube answered {status}");
        let listed = response
            .text()
            .await
            .context("cannot read the stream list")?;
        let qualities = parse::qualities(&listed);
        ensure!(!qualities.is_empty(), "the stream list names no stream");
        Ok(Streams {
            qualities,
            board: parse::storyboard(&answer),
        })
    }

    /// The cards a video ends on. The headset's player answer leaves them out, so they are read
    /// from the watch page, asked for without the account.
    pub async fn end_cards(&self, id: &str) -> Result<Vec<EndCard>> {
        const OPENED: &str = "\"endscreen\":";
        let response = self
            .http
            .get(crate::watch_url(id))
            .header("User-Agent", USER_AGENT)
            .header("Accept-Language", &self.language)
            .send()
            .await
            .context("cannot reach youtube")?;
        let status = response.status();
        ensure!(status.is_success(), "youtube answered {status}");
        let page = response
            .text()
            .await
            .context("cannot read the watch page")?;
        let Some(at) = page.find(OPENED) else {
            return Ok(Vec::new());
        };
        // The page goes on after the end cards, so only the one value that opens here is read.
        let endscreen = serde_json::Deserializer::from_str(&page[at + OPENED.len()..])
            .into_iter::<Value>()
            .next()
            .context("the watch page ends inside its end cards")?
            .context("cannot read the end cards")?;
        Ok(parse::end_cards(&endscreen))
    }

    fn visitor(&self) -> Option<String> {
        self.visitor
            .read()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    }

    async fn call(&self, endpoint: &str, body: Value) -> Result<Value> {
        self.call_as(&WEB, endpoint, body).await
    }

    async fn call_as(&self, guise: &Guise, endpoint: &str, body: Value) -> Result<Value> {
        self.send(guise, endpoint, body, None).await
    }

    /// Asks an endpoint as `acting`, or as the channel in use with none.
    async fn send(
        &self,
        guise: &Guise,
        endpoint: &str,
        mut body: Value,
        acting: Option<Acting>,
    ) -> Result<Value> {
        let acting = acting.unwrap_or_else(|| {
            self.acting
                .read()
                .unwrap_or_else(|held| held.into_inner())
                .clone()
        });
        // Only the request for streams needs the guest's name, so nothing else carries it and
        // searching and browsing are not tied to what is played.
        let visitor = self.visitor().filter(|_| guise.name == HEADSET.name);
        let mut client = json!({
            "clientName": guise.name,
            "clientVersion": guise.version,
            "hl": self.language,
            "gl": self.region,
        });
        for (name, value) in guise.device {
            client[*name] = json!(value);
        }
        if let Some(visitor) = &visitor {
            client["visitorData"] = json!(visitor);
        }
        body["context"] = json!({ "client": client });
        // Only the web client is the one the account signed in to.
        let account = self.account().filter(|_| guise.name == CLIENT_NAME);
        let page = account.as_ref().and_then(|_| acting.page.clone());
        if let Some(page) = &page {
            body["context"]["user"] = json!({ "onBehalfOfUser": page });
        }
        let mut request = self
            .http
            .post(format!("{HOST}/{endpoint}?prettyPrint=false"))
            .header("User-Agent", guise.agent)
            .header("Origin", ORIGIN)
            .header("X-Youtube-Client-Name", guise.number)
            .header("X-Youtube-Client-Version", guise.version);
        if let Some(visitor) = &visitor {
            request = request.header("X-Goog-Visitor-Id", visitor);
        }
        if let Some(page) = &page {
            request = request.header("X-Goog-PageId", page);
        }
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
                .header("X-Goog-AuthUser", acting.user.to_string());
        }
        let response = request
            .json(&body)
            .send()
            .await
            .context("cannot reach youtube")?;
        let status = response.status();
        ensure!(status.is_success(), "youtube answered {status}");
        let mut answer: Value = response
            .json()
            .await
            .context("cannot read youtube's answer")?;
        if self.visitor().is_none()
            && let Some(named) = answer["responseContext"]["visitorData"].as_str()
        {
            *self
                .visitor
                .write()
                .unwrap_or_else(|held| held.into_inner()) = Some(named.to_owned());
        }
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
