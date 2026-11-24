//! Reads InnerTube's JSON trees.
//!
//! YouTube moves its renderers between containers often, so lists are found by walking the whole
//! tree for the few renderers that are a video, a channel or a playlist, rather than by a fixed
//! path. The same walk is where the clutter is dropped.

use std::time::Duration;

use anyhow::{Context as _, Result};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;

use crate::{
    Caption, Channel, ChannelPage, ChannelRef, Comment, Comments, Found, Identity, Line, Page,
    Playlist, PlaylistPage, Post, Quality, Section, Storyboard, Tab, Video, Watch,
};

/// Containers whose contents are never listed: ads, the side card of a search and the chapter
/// and summary strips under a result.
const CLUTTER: [&str; 9] = [
    "adSlotRenderer",
    "promotedSparkleWebRenderer",
    "statementBannerRenderer",
    "secondarySearchContainerRenderer",
    "horizontalCardListRenderer",
    "expandableMetadataRenderer",
    "engagementPanels",
    "playerOverlays",
    "topbar",
];
/// The renderer of one Short.
/// Which of a video's storyboard sizes the seek bar previews with, counted from the smallest.
const BOARD_LEVEL: usize = 2;
const SHORT: &str = "shortsLockupViewModel";
const COMMENTS_SECTION: &str = "comment-item-section";

/// Every video, channel and playlist in an answer, in the order the page shows them.
pub(crate) fn listing(answer: &Value) -> Page<Found> {
    let mut page = Page::default();
    walk(answer, &mut page);
    page
}

/// The channels an account can act as, out of the answer that fills YouTube's account switcher.
pub(crate) fn identities(answer: &Value, user: usize) -> Vec<Identity> {
    let mut items = Vec::new();
    gather(answer, "accountItem", &mut items);
    items
        .into_iter()
        .filter(|item| !item["isDisabled"].as_bool().unwrap_or(false))
        .filter_map(|item| {
            // The token sits at a different depth from one kind of switcher to the next.
            let mut tokens = Vec::new();
            gather(item, "pageIdToken", &mut tokens);
            let page = tokens
                .into_iter()
                .find_map(|token| string(&token["pageId"]));
            let detail = text(&item["channelHandle"])
                .or_else(|| text(&item["email"]))
                .or_else(|| text(&item["accountByline"]));
            Some(Identity {
                user,
                page,
                name: text(&item["accountName"]).or_else(|| detail.clone())?,
                detail,
                avatar: thumbnail(&item["accountPhoto"]),
                selected: item["isSelected"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// Takes every Short out of an answer, wherever it sits.
pub(crate) fn drop_shorts(node: &mut Value) {
    match node {
        Value::Array(entries) => entries.iter_mut().for_each(drop_shorts),
        Value::Object(fields) => {
            fields.remove(SHORT);
            fields.values_mut().for_each(drop_shorts);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Only the videos of an answer.
pub(crate) fn videos(answer: &Value) -> Page<Video> {
    let Page { items, more } = listing(answer);
    Page {
        items: items
            .into_iter()
            .filter_map(|found| match found {
                Found::Video(video) => Some(video),
                Found::Channel(_) | Found::Playlist(_) => None,
            })
            .collect(),
        more,
    }
}

pub(crate) fn watch(id: &str, answer: &Value) -> Result<Watch> {
    let columns = &answer["contents"]["twoColumnWatchNextResults"];
    let contents = columns["results"]["results"]["contents"]
        .as_array()
        .context("the watch page has no contents")?;
    let section = |name: &str| contents.iter().find_map(|entry| entry.get(name));
    let primary = section("videoPrimaryInfoRenderer").context("the video has no title")?;
    let secondary = section("videoSecondaryInfoRenderer");

    let counter = &primary["viewCount"]["videoViewCountRenderer"];
    let owner = secondary.map(|secondary| &secondary["owner"]["videoOwnerRenderer"]);
    // A video made together lists everyone in a sheet behind the owner's line instead.
    let mut makers = owner.map(makers).unwrap_or_default();
    let channel = match makers.is_empty() {
        false => Some(makers.remove(0)),
        true => owner.and_then(|owner| {
            Some(Channel {
                id: first(owner, "browseId")?.as_str()?.to_owned(),
                name: text(&owner["title"])?,
                avatar: thumbnail(&owner["thumbnail"]),
                subscribers: text(&owner["subscriberCountText"]),
            })
        }),
    };
    let order = |place: usize| {
        first(&answer["engagementPanels"], "sortFilterSubMenuRenderer")
            .and_then(|menu| menu["subMenuItems"].get(place))
            .and_then(token)
    };
    let comments = contents
        .iter()
        .filter_map(|entry| entry.get("itemSectionRenderer"))
        .find(|section| section["sectionIdentifier"] == COMMENTS_SECTION)
        .and_then(|section| first(section, "continuationCommand"))
        .and_then(|command| command["token"].as_str())
        .map(str::to_owned);

    Ok(Watch {
        id: id.to_owned(),
        title: text(&primary["title"]).unwrap_or_default(),
        views: text(&counter["viewCount"]),
        published: text(&primary["dateText"]),
        likes: first(primary, "likeButtonViewModel")
            .and_then(|like| first(like, "title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| title.chars().next().is_some_and(char::is_numeric))
            .map(str::to_owned),
        description: secondary
            .and_then(|secondary| text(&secondary["attributedDescription"]))
            .unwrap_or_default(),
        channel,
        partners: makers,
        top: order(0),
        newest: order(1),
        related: videos(&columns["secondaryResults"]).items,
        liked: first(primary, "likeStatus")
            .or_else(|| first(answer, "likeStatus"))
            .and_then(Value::as_str)
            .map(|status| status == "LIKE"),
        comments,
    })
}

/// The whole-second duration YouTube publishes, rather than the length of a media rendition.
pub(crate) fn duration(player: &Value) -> Option<Duration> {
    let details = &player["videoDetails"];
    if details["isLive"].as_bool() == Some(true) {
        return None;
    }
    let seconds = details["lengthSeconds"].as_str()?.parse::<u64>().ok()?;
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

pub(crate) fn comments(answer: &Value) -> Comments {
    let empty = Vec::new();
    let listed: Vec<&Value> = answer["onResponseReceivedEndpoints"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .flat_map(|endpoint| {
            [
                "reloadContinuationItemsCommand",
                "appendContinuationItemsAction",
            ]
            .into_iter()
            .filter_map(|name| endpoint[name]["continuationItems"].as_array())
            .flatten()
        })
        .collect();

    // A thread names its comment by id and holds the token of its replies. The words and the
    // author are kept apart, in the entity mutations.
    let mut order = Vec::new();
    let mut more = None;
    for item in &listed {
        if let Some(next) = item.get("continuationItemRenderer") {
            more = token(next);
            continue;
        }
        let thread = &item["commentThreadRenderer"];
        let comment = match thread.is_object() {
            true => &thread["commentViewModel"]["commentViewModel"],
            false => &item["commentViewModel"],
        };
        let Some(id) = comment["commentId"].as_str() else {
            continue;
        };
        let replies = thread
            .get("replies")
            .and_then(|replies| first(replies, "continuationItemRenderer"))
            .and_then(token);
        order.push((id.to_owned(), replies));
    }

    let mutations = answer["frameworkUpdates"]["entityBatchUpdate"]["mutations"]
        .as_array()
        .unwrap_or(&empty);
    let items = order
        .into_iter()
        .filter_map(|(id, replies)| {
            let entity = mutations
                .iter()
                .map(|mutation| &mutation["payload"]["commentEntityPayload"])
                .find(|entity| entity["properties"]["commentId"] == id.as_str())?;
            let (properties, author, toolbar) =
                (&entity["properties"], &entity["author"], &entity["toolbar"]);
            // What the buttons under a comment do, and whether it is liked, are kept in
            // entities of their own that the comment names by key.
            let keyed = |payload: &str, key: &Value| {
                mutations
                    .iter()
                    .map(|mutation| &mutation["payload"][payload])
                    .find(|held| key.is_string() && held["key"] == *key)
            };
            let surface = keyed(
                "engagementToolbarSurfaceEntityPayload",
                &properties["toolbarSurfaceKey"],
            );
            let state = keyed(
                "engagementToolbarStateEntityPayload",
                &properties["toolbarStateKey"],
            );
            let command = |name: &str, field: &str| {
                surface
                    .and_then(|surface| first(&surface[name], field))
                    .and_then(string)
            };
            Some(Comment {
                id,
                author: ChannelRef {
                    id: string(&author["channelId"]).unwrap_or_default(),
                    name: string(&author["displayName"]).unwrap_or_default(),
                },
                avatar: string(&author["avatarThumbnailUrl"]),
                text: string(&properties["content"]["content"]).unwrap_or_default(),
                published: string(&properties["publishedTime"]),
                likes: string(&toolbar["likeCountNotliked"]).filter(|likes| !likes.is_empty()),
                creator: author["isCreator"].as_bool().unwrap_or(false),
                reply_count: string(&toolbar["replyCount"]).filter(|count| !count.is_empty()),
                replies,
                liked: state.is_some_and(|state| state["likeState"] == "TOOLBAR_LIKE_STATE_LIKED"),
                like: command("likeCommand", "action"),
                unlike: command("unlikeCommand", "action"),
                reply: command("replyCommand", "createReplyParams"),
            })
        })
        .collect();

    Comments {
        page: Page { items, more },
        total: first(answer, "commentsHeaderRenderer")
            .and_then(|header| text(&header["countText"])),
        write: first(answer, "createCommentParams").and_then(string),
    }
}

pub(crate) fn channel(id: &str, answer: &Value) -> Result<ChannelPage> {
    let about = &answer["metadata"]["channelMetadataRenderer"];
    let name = string(&about["title"]).context("the channel has no name")?;
    let subscribers = first(&answer["header"], "contentMetadataViewModel").and_then(|rows| {
        parts(rows)
            .into_iter()
            .find(|part| part.contains("subscriber"))
    });
    Ok(ChannelPage {
        channel: Channel {
            id: string(&about["externalId"]).unwrap_or_else(|| id.to_owned()),
            name,
            avatar: thumbnail(&about["avatar"]),
            subscribers,
        },
        banner: first(&answer["header"], "imageBannerViewModel")
            .and_then(|banner| banner["image"]["sources"].as_array()?.last())
            .and_then(|source| string(&source["url"]))
            .map(|address| whole_banner(&address)),
        description: string(&about["description"]).unwrap_or_default(),
        videos: videos(&answer["contents"]),
        tabs: tabs(answer),
    })
}

/// The parts a channel's page offers, of those the app shows.
fn tabs(answer: &Value) -> Vec<Tab> {
    let mut found = Vec::new();
    gather(&answer["contents"], "tabRenderer", &mut found);
    found
        .into_iter()
        .filter_map(|tab| {
            let address = first(&tab["endpoint"], "url")?.as_str()?;
            match address.rsplit('/').next()? {
                "videos" => Some(Tab::Videos),
                "shorts" => Some(Tab::Shorts),
                "playlists" => Some(Tab::Playlists),
                "posts" | "community" => Some(Tab::Posts),
                "releases" => Some(Tab::Releases),
                _ => None,
            }
        })
        .collect()
}

/// The playlists and the posts of a part of a channel, whichever it holds.
pub(crate) fn section(answer: &Value) -> Section {
    let Page { items, more } = listing(answer);
    let mut written = Vec::new();
    gather(answer, "backstagePostRenderer", &mut written);
    Section {
        videos: Page {
            items: items
                .iter()
                .filter_map(|found| match found {
                    Found::Video(video) if video.short => Some(video.clone()),
                    _ => None,
                })
                .collect(),
            more: more.clone(),
        },
        playlists: Page {
            items: items
                .into_iter()
                .filter_map(|found| match found {
                    Found::Playlist(playlist) => Some(playlist),
                    Found::Video(_) | Found::Channel(_) => None,
                })
                .collect(),
            more: more.clone(),
        },
        posts: Page {
            items: written.into_iter().filter_map(post).collect(),
            more,
        },
    }
}

fn post(node: &Value) -> Option<Post> {
    let attached = &node["backstageAttachment"];
    let mut shown = Vec::new();
    gather(attached, "backstageImageRenderer", &mut shown);
    Some(Post {
        id: string(&node["postId"])?,
        text: text(&node["contentText"]).unwrap_or_default(),
        published: text(&node["publishedTimeText"]),
        likes: text(&node["voteCount"]).filter(|likes| !likes.trim().is_empty()),
        pictures: shown
            .into_iter()
            .filter_map(|picture| thumbnail(&picture["image"]))
            .collect(),
        video: attached.get("videoRenderer").and_then(video),
    })
}

/// A playlist from the older renderer, which is how a channel lists its releases.
fn playlist_card(node: &Value) -> Option<Playlist> {
    Some(Playlist {
        id: string(&node["playlistId"])?,
        title: text(&node["title"])?,
        thumbnail: node["thumbnails"].get(0).and_then(thumbnail),
        detail: text(&node["videoCountText"]),
    })
}

/// Everyone a video made together is credited to, in the order YouTube names them.
fn makers(owner: &Value) -> Vec<Channel> {
    let mut listed = Vec::new();
    gather(
        &owner["navigationEndpoint"],
        "listItemViewModel",
        &mut listed,
    );
    let mut makers: Vec<Channel> = Vec::new();
    for entry in listed {
        let Some(id) = first(&entry["title"], "browseId").and_then(string) else {
            continue;
        };
        let Some(name) = string(&entry["title"]["content"]) else {
            continue;
        };
        if makers.iter().any(|maker| maker.id == id) {
            continue;
        }
        makers.push(Channel {
            id,
            name,
            avatar: first(&entry["leadingAccessory"], "sources")
                .and_then(|sources| sources.as_array()?.last())
                .and_then(|source| string(&source["url"])),
            subscribers: string(&entry["subtitle"]["content"]).and_then(|line| {
                line.split('•')
                    .map(plain)
                    .find(|part| part.contains("subscriber"))
            }),
        });
    }
    makers
}

/// Every value stored under `name` anywhere below `node`, in the order the tree holds them.
fn gather<'a>(node: &'a Value, name: &str, found: &mut Vec<&'a Value>) {
    match node {
        Value::Object(fields) => {
            for (field, value) in fields {
                match field == name {
                    true => found.push(value),
                    false => gather(value, name, found),
                }
            }
        }
        Value::Array(entries) => entries.iter().for_each(|entry| gather(entry, name, found)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

pub(crate) fn playlist(id: &str, answer: &Value) -> PlaylistPage {
    PlaylistPage {
        id: id.to_owned(),
        title: string(&answer["metadata"]["playlistMetadataRenderer"]["title"]).unwrap_or_default(),
        videos: videos(&answer["contents"]),
    }
}

fn walk(node: &Value, page: &mut Page<Found>) {
    match node {
        Value::Array(entries) => entries.iter().for_each(|entry| walk(entry, page)),
        Value::Object(fields) => {
            for (name, value) in fields {
                let found = match name.as_str() {
                    "videoRenderer"
                    | "compactVideoRenderer"
                    | "gridVideoRenderer"
                    | "playlistVideoRenderer" => video(value).map(Found::Video),
                    "channelRenderer" => channel_card(value).map(Found::Channel),
                    "playlistRenderer" | "gridPlaylistRenderer" => {
                        playlist_card(value).map(Found::Playlist)
                    }
                    "lockupViewModel" => lockup(value),
                    SHORT => short(value).map(Found::Video),
                    "continuationItemRenderer" | "continuationItemViewModel" => {
                        if let Some(token) = token(value) {
                            page.more = Some(token);
                        }
                        continue;
                    }
                    name if CLUTTER.contains(&name) => continue,
                    _ => {
                        walk(value, page);
                        continue;
                    }
                };
                let Some(found) = found else { continue };
                if !page.items.contains(&found) {
                    page.items.push(found);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// The address of the captions a transcript is read from and the language they are in. The
/// `wanted` language is taken when the video has it, translated by YouTube when it does not.
/// Otherwise written captions in `language` come first, then generated ones in it, then whatever
/// the video lists first.
pub(crate) fn caption_track(
    player: &Value,
    language: &str,
    wanted: Option<&Caption>,
) -> Option<(String, Caption)> {
    let tracks = first(player, "captionTracks")?.as_array()?;
    let spoken = |track: &&Value| {
        track["languageCode"]
            .as_str()
            .is_some_and(|code| code.starts_with(language))
    };
    let written = |track: &&Value| track["kind"].as_str() != Some("asr");
    let usual = || {
        tracks
            .iter()
            .find(|track| spoken(track) && written(track))
            .or_else(|| tracks.iter().find(spoken))
            .or_else(|| tracks.first())
    };
    let own = wanted
        .filter(|wanted| !wanted.translated)
        .and_then(|wanted| {
            tracks
                .iter()
                .find(|track| track["languageCode"].as_str() == Some(&wanted.code))
        });
    let track = own.or_else(usual)?;
    let address = string(&track["baseUrl"])?;
    match wanted.filter(|wanted| wanted.translated) {
        Some(wanted) => Some((format!("{address}&tlang={}", wanted.code), wanted.clone())),
        None => Some((address, caption(track, false)?)),
    }
}

/// The languages a video's captions come in: the ones it carries, then the ones YouTube
/// translates them into.
pub(crate) fn caption_choices(player: &Value) -> Vec<Caption> {
    let listed = |key: &str, translated: bool| {
        first(player, key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(move |language| caption(language, translated))
            .collect::<Vec<_>>()
    };
    let mut choices = listed("captionTracks", false);
    choices.dedup_by(|next, kept| next.code == kept.code);
    choices.extend(listed("translationLanguages", true));
    choices
}

/// One entry of a list of caption languages.
fn caption(language: &Value, translated: bool) -> Option<Caption> {
    let code = string(&language["languageCode"])?;
    let name = text(&language["name"])
        .or_else(|| text(&language["languageName"]))
        .unwrap_or_else(|| code.clone());
    Some(Caption {
        code,
        name,
        translated,
    })
}

/// The lines of a `json3` caption file, without the empty ones that only break a line.
/// The value of `name` in the attribute list of a stream list's line, without its quotes.
fn attribute<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = line.split_once(':')?.1;
    while !rest.is_empty() {
        let (held, after) = rest.split_once('=')?;
        let (value, next) = match after.strip_prefix('"') {
            Some(quoted) => {
                let (value, next) = quoted.split_once('"')?;
                (value, next.trim_start_matches(','))
            }
            None => after.split_once(',').unwrap_or((after, "")),
        };
        if held == name {
            return Some(value);
        }
        rest = next;
    }
    None
}

/// What asks YouTube's image host for a banner uncut and wide enough for a large window.
const WHOLE_BANNER: &str = "w2120-fcrop64=1,00000000ffffffff-k-c0xffffffff-no-nd-rj";
/// How far into a caption an HTML entity may end, counted from its `&`.
const ENTITY_REACH: usize = 9;

/// The address of a channel's whole banner, as a television shows it, from the address of the
/// thin strip a desktop page cuts out of it. The size and the cut follow the first `=`.
fn whole_banner(address: &str) -> String {
    match address.split_once('=') {
        Some((picture, _)) => format!("{picture}={WHOLE_BANNER}"),
        None => address.to_owned(),
    }
}

/// How well a sound track of an HLS stream list suits a listener, higher being better: the one
/// YouTube marks as the default, then the language the video was recorded in, then anything
/// that is not a dub. YouTube lists dubs first and often marks no default.
fn fitness(line: &str) -> u8 {
    if attribute(line, "DEFAULT") == Some("YES") {
        return 3;
    }
    let tags = attribute(line, "YT-EXT-XTAGS")
        .and_then(|tags| URL_SAFE_NO_PAD.decode(tags.trim_end_matches('=')).ok())
        .map(|tags| String::from_utf8_lossy(&tags).into_owned())
        .unwrap_or_default();
    match (tags.contains("original"), tags.contains("dubbed")) {
        (true, _) => 2,
        (false, false) => 1,
        (false, true) => 0,
    }
}

/// The qualities an HLS stream list offers, best first and one per height: the H.264 picture
/// where there is one, since every machine decodes it in hardware, and no HDR ones, which
/// would need tone mapping.
pub(crate) fn qualities(listed: &str) -> Vec<Quality> {
    let mut sounds: Vec<(&str, &str, u8)> = Vec::new();
    let mut found: Vec<(Quality, bool)> = Vec::new();
    let mut lines = listed.lines();
    while let Some(line) = lines.next() {
        if line.starts_with("#EXT-X-MEDIA:") {
            if attribute(line, "TYPE") == Some("AUDIO")
                && let (Some(group), Some(address)) =
                    (attribute(line, "GROUP-ID"), attribute(line, "URI"))
            {
                sounds.push((group, address, fitness(line)));
            }
            continue;
        }
        if !line.starts_with("#EXT-X-STREAM-INF:") {
            continue;
        }
        let Some(picture) = lines.next() else { break };
        if attribute(line, "VIDEO-RANGE").is_some_and(|range| range != "SDR") {
            continue;
        }
        let Some(height) = attribute(line, "RESOLUTION")
            .and_then(|size| size.split_once('x'))
            .and_then(|(_, height)| height.parse().ok())
        else {
            continue;
        };
        let group = attribute(line, "AUDIO");
        // The first of the best fitting ones, since `max_by_key` would take the last.
        let Some(sound) = sounds
            .iter()
            .filter(|sound| Some(sound.0) == group)
            .rev()
            .max_by_key(|sound| sound.2)
        else {
            continue;
        };
        let rate = attribute(line, "FRAME-RATE")
            .and_then(|rate| rate.parse::<f32>().ok())
            .map_or(0, |rate| rate.round() as u32);
        let plain = attribute(line, "CODECS").is_some_and(|codecs| codecs.starts_with("avc1"));
        found.push((
            Quality {
                height,
                rate,
                picture: picture.trim().to_owned(),
                sound: sound.1.to_owned(),
            },
            plain,
        ));
    }
    // Within a height the smoother picture wins, then the plainer codec, then the last listed,
    // which carries the better sound.
    found.sort_by_key(|(quality, plain)| (quality.height, quality.rate, *plain));
    let mut qualities: Vec<Quality> = Vec::new();
    for (quality, _) in found.into_iter().rev() {
        if qualities
            .last()
            .is_none_or(|last| last.height != quality.height)
        {
            qualities.push(quality);
        }
    }
    qualities
}

pub(crate) fn transcript(timed: &Value) -> Vec<Line> {
    let Some(events) = timed["events"].as_array() else {
        return Vec::new();
    };
    events
        .iter()
        .filter_map(|event| {
            let said: String = event["segs"]
                .as_array()?
                .iter()
                .filter_map(|piece| piece["utf8"].as_str())
                .collect();
            let text = plain(&said);
            (!text.is_empty()).then(|| Line {
                start: event["tStartMs"].as_f64().unwrap_or(0.) / 1000.,
                length: event["dDurationMs"].as_f64().unwrap_or(0.) / 1000.,
                text,
            })
        })
        .collect()
}

/// A Short, which is listed with little more than its title and views.
fn short(node: &Value) -> Option<Video> {
    let id = first(node, "reelWatchEndpoint")?["videoId"].as_str()?;
    let words = &node["overlayMetadata"];
    Some(Video {
        id: id.to_owned(),
        title: words["primaryText"]["content"].as_str()?.to_owned(),
        views: words["secondaryText"]["content"]
            .as_str()
            .map(str::to_owned),
        short: true,
        still: first(&node["thumbnail"], "url")
            .or_else(|| first(&node["thumbnailViewModel"], "url"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        ..Video::default()
    })
}

/// A video from the older renderers, which name each field.
fn video(node: &Value) -> Option<Video> {
    let byline = ["longBylineText", "shortBylineText", "ownerText"]
        .into_iter()
        .map(|name| &node[name])
        .find(|byline| byline.is_object());
    Some(Video {
        id: node["videoId"]
            .as_str()
            .filter(|id| is_video(id))?
            .to_owned(),
        title: text(&node["title"])?,
        channel: byline.and_then(channel_ref),
        length: text(&node["lengthText"]),
        views: text(&node["shortViewCountText"]).or_else(|| text(&node["viewCountText"])),
        published: text(&node["publishedTimeText"]),
        short: false,
        snippet: node["detailedMetadataSnippets"][0]
            .get("snippetText")
            .and_then(text)
            .or_else(|| text(&node["descriptionSnippet"])),
        avatar: first(&node["channelThumbnailSupportedRenderers"], "thumbnail").and_then(thumbnail),
        still: None,
    })
}

/// A video or a playlist from the newer lockup, which only has rows of text.
fn lockup(node: &Value) -> Option<Found> {
    let id = node["contentId"].as_str()?.to_owned();
    let about = &node["metadata"]["lockupMetadataViewModel"];
    let title = string(&about["title"]["content"])?;
    let rows = parts(&about["metadata"]["contentMetadataViewModel"]);
    match node["contentType"].as_str()? {
        // An advert dressed as a video carries something else where the id goes.
        "LOCKUP_CONTENT_TYPE_VIDEO" if !is_video(&id) => None,
        "LOCKUP_CONTENT_TYPE_VIDEO" => {
            // A channel's own page leaves its name out, so the views lead. Everywhere else the
            // name comes first.
            let owner = first(&about["image"], "browseId").and_then(Value::as_str);
            let (name, rest) = match owner {
                Some(_) => (rows.first().cloned(), rows.get(1..).unwrap_or_default()),
                None => (None, rows.as_slice()),
            };
            Some(Found::Video(Video {
                id,
                title,
                channel: owner.zip(name).map(|(id, name)| ChannelRef {
                    id: id.to_owned(),
                    name,
                }),
                length: first(&node["contentImage"], "thumbnailBadgeViewModel")
                    .and_then(|badge| string(&badge["text"]))
                    .filter(|length| length.contains(':')),
                views: rest.first().cloned(),
                published: rest.get(1).cloned(),
                short: false,
                snippet: None,
                avatar: None,
                still: None,
            }))
        }
        "LOCKUP_CONTENT_TYPE_PLAYLIST" | "LOCKUP_CONTENT_TYPE_PODCAST" => {
            Some(Found::Playlist(Playlist {
                id,
                title,
                thumbnail: first(&node["contentImage"], "sources")
                    .and_then(|sources| sources.as_array()?.last())
                    .and_then(|source| string(&source["url"])),
                detail: Some(rows.join(" · ")).filter(|detail| !detail.is_empty()),
            }))
        }
        _ => None,
    }
}

/// Whether `id` has the shape of a video's id: eleven letters, digits, dashes or underscores.
fn is_video(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// The sheets of preview pictures a player answer names, at the size a seek bar shows.
pub(crate) fn storyboard(player: &Value) -> Option<Storyboard> {
    let spec = player["storyboards"]["playerStoryboardSpecRenderer"]["spec"].as_str()?;
    let mut levels = spec.split('|');
    let base = levels.next()?;
    // The levels grow in size. The third is large enough to read and small enough to be quick.
    let (level, fields) = levels
        .map(|level| level.split('#').collect::<Vec<_>>())
        .enumerate()
        .filter(|(_, fields)| fields.len() >= 8)
        .take(BOARD_LEVEL + 1)
        .last()?;
    let number = |place: usize| fields[place].parse::<u32>().ok();
    let named = base
        .replace("$L", &level.to_string())
        .replace("$N", fields[6]);
    let (before, after) = named.split_once("$M").unwrap_or((&named, ""));
    Some(Storyboard {
        address: (before.to_owned(), format!("{after}&sigh={}", fields[7])),
        width: number(0)?,
        height: number(1)?,
        count: number(2)?,
        columns: number(3)?,
        rows: number(4)?,
        interval: f64::from(number(5)?) / 1000.,
    })
}

fn channel_card(node: &Value) -> Option<Channel> {
    // Since handles arrived the subscriber count sits in the field named for the video count.
    let subscribers = [&node["videoCountText"], &node["subscriberCountText"]]
        .into_iter()
        .filter_map(text)
        .find(|count| count.contains("subscriber"));
    Some(Channel {
        id: node["channelId"].as_str()?.to_owned(),
        name: text(&node["title"])?,
        avatar: thumbnail(&node["thumbnail"]),
        subscribers,
    })
}

fn channel_ref(byline: &Value) -> Option<ChannelRef> {
    Some(ChannelRef {
        id: first(byline, "browseId")?.as_str()?.to_owned(),
        name: text(byline)?,
    })
}

/// The token a continuation entry carries, wherever in it the command sits.
fn token(entry: &Value) -> Option<String> {
    string(&first(entry, "continuationCommand")?["token"])
}

/// The words of any of the three shapes YouTube writes text in.
fn text(node: &Value) -> Option<String> {
    if let Some(plain) = node["simpleText"]
        .as_str()
        .or_else(|| node["content"].as_str())
    {
        return Some(plain.to_owned());
    }
    let runs = node["runs"].as_array()?;
    Some(runs.iter().filter_map(|run| run["text"].as_str()).collect())
}

/// Every piece of text in the rows of a `contentMetadataViewModel`, row after row.
fn parts(rows: &Value) -> Vec<String> {
    let empty = Vec::new();
    rows["metadataRows"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|row| row["metadataParts"].as_array())
        .flatten()
        .filter_map(|part| string(&part["text"]["content"]))
        .collect()
}

/// The largest of a renderer's thumbnails, with the scheme YouTube sometimes leaves off.
fn thumbnail(node: &Value) -> Option<String> {
    let url = node["thumbnails"].as_array()?.last()?["url"].as_str()?;
    Some(match url.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_owned(),
    })
}

fn string(node: &Value) -> Option<String> {
    node.as_str().map(str::to_owned)
}

/// The first value stored under `name` anywhere below `node`, depth first.
fn first<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    match node {
        Value::Object(fields) => fields
            .get(name)
            .or_else(|| fields.values().find_map(|value| first(value, name))),
        Value::Array(entries) => entries.iter().find_map(|entry| first(entry, name)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

/// A caption as plain words: without the markup uploaders leave in (`<b>`, `<font …>`), with
/// HTML entities read, and without invisible characters or runs of white space.
fn plain(said: &str) -> String {
    let mut bare = String::with_capacity(said.len());
    let mut rest = said;
    while let Some(at) = rest.find('<') {
        let tag = rest[at..].find('>').filter(|end| {
            rest[at + 1..at + end]
                .starts_with(|first: char| first.is_ascii_alphabetic() || first == '/')
        });
        match tag {
            Some(end) => {
                bare.push_str(&rest[..at]);
                rest = &rest[at + end + 1..];
            }
            None => {
                bare.push_str(&rest[..=at]);
                rest = &rest[at + 1..];
            }
        }
    }
    bare.push_str(rest);
    let mut read = String::with_capacity(bare.len());
    let mut rest = bare.as_str();
    while let Some(at) = rest.find('&') {
        read.push_str(&rest[..at]);
        rest = &rest[at..];
        let named = rest
            .find(';')
            .filter(|end| *end <= ENTITY_REACH)
            .and_then(|end| Some((entity(&rest[1..end])?, end)));
        match named {
            Some((letter, end)) => {
                read.push(letter);
                rest = &rest[end + 1..];
            }
            None => {
                read.push('&');
                rest = &rest[1..];
            }
        }
    }
    read.push_str(rest);
    read.retain(|letter| {
        !matches!(letter, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}')
            && (!letter.is_control() || letter.is_whitespace())
    });
    read.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The character an HTML entity stands for, given without its `&` and `;`.
fn entity(name: &str) -> Option<char> {
    let number = |digits: &str, radix| {
        u32::from_str_radix(digits, radix)
            .ok()
            .and_then(char::from_u32)
    };
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => match name.strip_prefix('#') {
            Some(code) => match code.strip_prefix(['x', 'X']) {
                Some(hex) => number(hex, 16),
                None => number(code, 10),
            },
            None => None,
        },
    }
}
