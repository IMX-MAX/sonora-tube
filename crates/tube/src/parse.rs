//! Reads InnerTube's JSON trees.
//!
//! YouTube moves its renderers between containers often, so lists are found by walking the whole
//! tree for the few renderers that are a video, a channel or a playlist, rather than by a fixed
//! path. The same walk is where the clutter is dropped.

use anyhow::{Context as _, Result};
use serde_json::Value;

use crate::{
    Channel, ChannelPage, ChannelRef, Comment, Comments, Found, Page, Playlist, PlaylistPage,
    Video, Watch,
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
const SHORT: &str = "shortsLockupViewModel";
const COMMENTS_SECTION: &str = "comment-item-section";

/// Every video, channel and playlist in an answer, in the order the page shows them.
pub(crate) fn listing(answer: &Value) -> Page<Found> {
    let mut page = Page::default();
    walk(answer, &mut page);
    page
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
    let channel = owner.and_then(|owner| {
        Some(Channel {
            id: first(owner, "browseId")?.as_str()?.to_owned(),
            name: text(&owner["title"])?,
            avatar: thumbnail(&owner["thumbnail"]),
            subscribers: text(&owner["subscriberCountText"]),
        })
    });
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
            .map(str::to_owned),
        description: secondary
            .and_then(|secondary| text(&secondary["attributedDescription"]))
            .unwrap_or_default(),
        channel,
        related: videos(&columns["secondaryResults"]).items,
        comments,
    })
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
            })
        })
        .collect();

    Comments {
        page: Page { items, more },
        total: first(answer, "commentsHeaderRenderer")
            .and_then(|header| text(&header["countText"])),
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
        description: string(&about["description"]).unwrap_or_default(),
        videos: videos(&answer["contents"]),
    })
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
        id: node["videoId"].as_str()?.to_owned(),
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
    })
}

/// A video or a playlist from the newer lockup, which only has rows of text.
fn lockup(node: &Value) -> Option<Found> {
    let id = node["contentId"].as_str()?.to_owned();
    let about = &node["metadata"]["lockupMetadataViewModel"];
    let title = string(&about["title"]["content"])?;
    let rows = parts(&about["metadata"]["contentMetadataViewModel"]);
    match node["contentType"].as_str()? {
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
