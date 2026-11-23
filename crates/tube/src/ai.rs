//! Mistral's chat API: answers about a video drawn from its transcript, and chapters cut from it.

use std::fmt::Write as _;

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::{Chapter, Line};

/// Where Mistral's API lives. `YOUTUBE_PLUS_MISTRAL_URL` points the app at another server
/// speaking the same API, such as a local stand-in.
const HOST: &str = "https://api.mistral.ai/v1";
/// The models offered before the key has listed the ones it can use.
pub const MODELS: [&str; 9] = [
    "mistral-small-latest",
    "mistral-medium-latest",
    "mistral-large-latest",
    "magistral-small-latest",
    "magistral-medium-latest",
    "ministral-3b-latest",
    "ministral-8b-latest",
    "codestral-latest",
    "open-mistral-nemo",
];
/// The model chat and chapters start on.
pub const MODEL: &str = "mistral-small-latest";
/// How much of the transcript goes to the model, in characters, so a long video still fits.
const READ: usize = 90_000;
/// How much of the description goes to the model, in characters.
const DESCRIPTION: usize = 4_000;
/// How much of the top comments goes to the model, in characters.
const COMMENTS: usize = 6_000;
/// How long one transcript entry grows before a new timestamp starts, in characters.
const ENTRY: usize = 160;

/// Who said a message in a chat about a video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speaker {
    User,
    Assistant,
}

/// One message of a chat about a video.
#[derive(Clone, Debug)]
pub struct Said {
    pub speaker: Speaker,
    pub text: String,
}

/// Mistral reached with one API key.
#[derive(Clone)]
pub struct Mistral {
    http: reqwest::Client,
    key: String,
    host: String,
}

impl Mistral {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            key: key.into(),
            host: std::env::var("YOUTUBE_PLUS_MISTRAL_URL").unwrap_or_else(|_| HOST.to_owned()),
        }
    }

    /// Every model the key can chat with, by name, as Mistral lists them.
    pub async fn models(&self) -> Result<Vec<String>> {
        let answer = self
            .http
            .get(format!("{}/models", self.host))
            .bearer_auth(&self.key)
            .send()
            .await
            .context("cannot reach mistral")?;
        let answer = checked(answer).await?;
        let mut names: Vec<String> = answer["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| model["capabilities"]["completion_chat"].as_bool() != Some(false))
            .filter_map(|model| model["id"].as_str())
            .filter(|name| !name.contains("embed") && !name.contains("ocr"))
            .map(str::to_owned)
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// The answer to the last message of `said`, from a model that read the video's transcript
    /// with its `description` and its top `comments`, and writes the moments it points at as `[m:ss]`.
    pub async fn ask(
        &self,
        model: &str,
        title: &str,
        description: &str,
        lines: &[Line],
        comments: &[String],
        said: &[Said],
    ) -> Result<String> {
        let brief = format!(
            "You answer questions about the YouTube video \"{title}\" from its transcript below. \
             Each transcript entry starts with the moment it is spoken, as [m:ss] or [h:mm:ss]. \
             Whenever you refer to a part of the video, cite the moment it happens in that same \
             form, for example [4:05], so the viewer can jump to it, and a stretch of it as its start \
             and end joined by a dash, for example [4:05]-[5:10]. Answer in Markdown: lists, bold, tables \
             and code blocks are drawn, and math goes in LaTeX between $ signs, or $$ signs for \
             an equation on its own line. Never put a moment inside a code span or an equation. \
             Keep answers short and direct and answer in the language of the question. \
             If the transcript does not cover something, say so rather than guessing.{}{}\n\n\
             Transcript:\n{}",
            described(description),
            viewers(comments),
            transcript(lines)
        );
        let mut messages = vec![json!({ "role": "system", "content": brief })];
        messages.extend(said.iter().map(|said| {
            json!({
                "role": match said.speaker {
                    Speaker::User => "user",
                    Speaker::Assistant => "assistant",
                },
                "content": said.text,
            })
        }));
        let answer = self
            .complete(json!({ "model": model, "messages": messages, "temperature": 0.3 }))
            .await?;
        Ok(answer.trim().to_owned())
    }

    /// Chapters for the video, cut from its transcript, the first at its start.
    pub async fn chapters(
        &self,
        model: &str,
        title: &str,
        lines: &[Line],
        length: f64,
    ) -> Result<Vec<Chapter>> {
        let brief = format!(
            "Split the YouTube video \"{title}\" into chapters from its transcript below, where \
             each entry starts with the moment it is spoken as [m:ss] or [h:mm:ss]. Make between \
             3 and 12 chapters depending on the length, the first one starting at 0. Each title \
             is 2 to 6 words in the language of the transcript, in sentence case, with no \
             numbering or trailing period. Reply with JSON only, in the shape \
             {{\"chapters\":[{{\"start\":\"0:00\",\"title\":\"Introduction\"}}]}}.\n\n\
             Transcript:\n{}",
            transcript(lines)
        );
        let answer = self
            .complete(json!({
                "model": model,
                "messages": [{ "role": "user", "content": brief }],
                "temperature": 0.2,
                "response_format": { "type": "json_object" },
            }))
            .await?;
        let answer: Value = serde_json::from_str(answer.trim())
            .context("cannot read the chapters mistral wrote")?;
        Ok(settle(&answer, length))
    }

    /// The text of the first choice of a chat completion.
    async fn complete(&self, body: Value) -> Result<String> {
        let answer = self
            .http
            .post(format!("{}/chat/completions", self.host))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .context("cannot reach mistral")?;
        let answer = checked(answer).await?;
        answer["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .context("mistral sent no answer")
    }
}

/// The body of a response that went through, or the reason Mistral gave for turning it down.
async fn checked(answer: reqwest::Response) -> Result<Value> {
    let status = answer.status();
    let body: Value = answer.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let reason = body["message"]
        .as_str()
        .or_else(|| body["detail"].as_str())
        .or_else(|| body["error"]["message"].as_str())
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("refused"));
    match status.as_u16() {
        401 | 403 => bail!("mistral turned down the API key: {reason}"),
        429 => bail!("mistral is rate limiting this key: {reason}"),
        _ => bail!("mistral answered {status}: {reason}"),
    }
}

/// The description as part of the brief, cut to [`DESCRIPTION`] characters, or nothing when the
/// video has none.
fn described(description: &str) -> String {
    let description = description.trim();
    if description.is_empty() {
        return String::new();
    }
    let kept: String = description.chars().take(DESCRIPTION).collect();
    format!(
        "\n\nThe uploader's description of the video follows. It may hold links, credits and \
         chapters the transcript does not.\n\nDescription:\n{kept}"
    )
}

/// The top comments as part of the brief, cut to [`COMMENTS`] characters, or nothing without any.
fn viewers(comments: &[String]) -> String {
    if comments.is_empty() {
        return String::new();
    }
    let mut written = String::from(
        "\n\nThe video's top comments follow, each with who wrote it and how many liked it. \
         They are what viewers think, not what the video says: draw on them when asked about \
         reactions or opinions, and say that a point comes from the comments.\n\nTop comments:",
    );
    let mut room = COMMENTS;
    for comment in comments {
        let length = comment.chars().count();
        if length > room {
            break;
        }
        room -= length;
        written.push_str("\n- ");
        written.push_str(&comment.replace('\n', " "));
    }
    written
}

/// The transcript as entries that each start with their moment, cut to [`READ`] characters.
fn transcript(lines: &[Line]) -> String {
    let mut out = String::new();
    let mut entry = String::new();
    for line in lines {
        let text = line.text.trim();
        if text.is_empty() {
            continue;
        }
        if entry.is_empty() {
            let _ = write!(entry, "[{}] ", stamp(line.start));
        } else {
            entry.push(' ');
        }
        entry.push_str(text);
        if entry.len() >= ENTRY {
            out.push_str(&entry);
            out.push('\n');
            entry.clear();
        }
        if out.len() >= READ {
            break;
        }
    }
    out.push_str(&entry);
    if out.trim().is_empty() {
        return "(this video has no transcript)".to_owned();
    }
    out
}

/// `seconds` as `m:ss`, or `h:mm:ss` past an hour.
pub fn stamp(seconds: f64) -> String {
    let whole = seconds.max(0.) as u64;
    let (hours, minutes, seconds) = (whole / 3600, whole / 60 % 60, whole % 60);
    match hours {
        0 => format!("{minutes}:{seconds:02}"),
        _ => format!("{hours}:{minutes:02}:{seconds:02}"),
    }
}

/// Seconds from a `m:ss`, `h:mm:ss` or bare number of seconds.
fn seconds(stamp: &Value) -> Option<f64> {
    if let Some(seconds) = stamp.as_f64() {
        return Some(seconds);
    }
    let text = stamp.as_str()?.trim().trim_matches(['[', ']']);
    text.split(':').try_fold(0., |sum, part| {
        part.trim().parse::<f64>().ok().map(|part| sum * 60. + part)
    })
}

/// The chapters of a model's JSON in order, inside the video, at least ten seconds apart and
/// the first at its start.
fn settle(answer: &Value, length: f64) -> Vec<Chapter> {
    let mut chapters: Vec<Chapter> = answer["chapters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|chapter| {
            let title = chapter["title"].as_str()?.trim().trim_end_matches('.');
            let start = seconds(&chapter["start"])?;
            (!title.is_empty() && start >= 0. && (length <= 0. || start < length)).then(|| {
                Chapter {
                    start,
                    title: title.to_owned(),
                }
            })
        })
        .collect();
    chapters.sort_by(|one, other| one.start.total_cmp(&other.start));
    chapters.dedup_by(|later, earlier| later.start - earlier.start < 10.);
    if let Some(first) = chapters.first_mut() {
        first.start = 0.;
    }
    match chapters.len() {
        0 | 1 => Vec::new(),
        _ => chapters,
    }
}
