use std::time::Duration;

use anyhow::{Context as _, Result};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

const ENDPOINT: &str = "https://sponsor.ajay.app/api/skipSegments";
const TIMEOUT: Duration = Duration::from_secs(8);
const CATEGORIES: &str = "[\"sponsor\",\"selfpromo\"]";

/// A paid or self-promoting stretch of a video, in seconds, and who marked it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sponsor {
    pub start: f64,
    pub end: f64,
    /// Whether the AI found it in the transcript rather than SponsorBlock's viewers marking it.
    pub ai: bool,
}

#[derive(Deserialize)]
struct Listed {
    #[serde(rename = "videoID")]
    id: String,
    segments: Vec<Marked>,
}

#[derive(Deserialize)]
struct Marked {
    segment: [f64; 2],
    #[serde(default, rename = "actionType")]
    action: String,
}

/// The stretches SponsorBlock's viewers marked in `video`, in order. Only the first four hex
/// digits of the id's hash leave the app, and the video is picked out of the answer here.
pub async fn marked(http: &reqwest::Client, video: &str) -> Result<Vec<Sponsor>> {
    let hash = Sha256::digest(video.as_bytes());
    let prefix = format!("{:02x}{:02x}", hash[0], hash[1]);
    let answer = http
        .get(format!("{ENDPOINT}/{prefix}"))
        .query(&[("categories", CATEGORIES), ("actionTypes", "[\"skip\"]")])
        .timeout(TIMEOUT)
        .send()
        .await
        .context("cannot reach sponsorblock")?;
    if answer.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    let listed: Vec<Listed> = answer
        .error_for_status()
        .context("sponsorblock refused the lookup")?
        .json()
        .await
        .context("cannot read what sponsorblock sent")?;
    let found = listed
        .into_iter()
        .find(|listed| listed.id == video)
        .map(|listed| listed.segments)
        .unwrap_or_default();
    Ok(settle(
        found
            .into_iter()
            .filter(|marked| marked.action.is_empty() || marked.action == "skip")
            .map(|marked| Sponsor {
                start: marked.segment[0],
                end: marked.segment[1],
                ai: false,
            })
            .collect(),
    ))
}

/// The stretches in order, without the empty or broken ones, and with overlapping ones joined.
pub fn settle(mut found: Vec<Sponsor>) -> Vec<Sponsor> {
    found.retain(|sponsor| {
        sponsor.start.is_finite()
            && sponsor.end.is_finite()
            && sponsor.start >= 0.
            && sponsor.end > sponsor.start
    });
    found.sort_by(|one, other| one.start.total_cmp(&other.start));
    let mut settled: Vec<Sponsor> = Vec::new();
    for sponsor in found {
        match settled.last_mut() {
            Some(last) if sponsor.start <= last.end => last.end = last.end.max(sponsor.end),
            _ => settled.push(sponsor),
        }
    }
    settled
}
