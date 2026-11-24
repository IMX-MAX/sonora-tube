//! Videos saved to the machine as files, fetched by the yt-dlp shipped with the app.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Context as _, Result, bail};
use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tube::Video;

use crate::{Io, Outcome, Toasts, join};

/// The folder inside the user's downloads the files go to.
const FOLDER: &str = "YouTube+";
/// What yt-dlp prints before each share of the file it has fetched.
const MARK: &str = "fetched:";
/// The tallest picture a download asks for when the parts can be joined.
const TALLEST: u32 = 1080;

/// The one [`VideoDownloads`] of the app.
struct Held(Entity<VideoDownloads>);

impl Global for Held {}

/// The downloads under way, each by the id of its video with the share of it fetched so far.
pub struct VideoDownloads {
    client: Option<tube::Client>,
    running: HashMap<String, (f32, Task<()>)>,
}

impl VideoDownloads {
    /// The downloads of the app, made the first time they are asked for.
    pub fn entity(cx: &mut App) -> Entity<Self> {
        if cx.try_global::<Held>().is_none() {
            let downloads = cx.new(|_| Self {
                client: None,
                running: HashMap::new(),
            });
            cx.set_global(Held(downloads));
        }
        cx.global::<Held>().0.clone()
    }

    /// Gives the downloads the client whose own streams they fall back on when yt-dlp is
    /// turned away.
    pub fn install(client: tube::Client, cx: &mut App) {
        Self::entity(cx).update(cx, |this, _| this.client = Some(client));
    }

    /// How much of the video is fetched, from 0 to 1, while it is being downloaded.
    pub fn progress(video: &str, cx: &App) -> Option<f32> {
        let held = cx.try_global::<Held>()?;
        held.0.read(cx).running.get(video).map(|running| running.0)
    }

    /// The folder downloads are saved in.
    pub fn folder() -> Option<PathBuf> {
        let downloads =
            dirs::download_dir().or_else(|| Some(dirs::home_dir()?.join("Downloads")))?;
        Some(downloads.join(FOLDER))
    }

    /// Starts saving the video as a file, telling how it went in a toast. A video already being
    /// downloaded is left to finish.
    pub fn start(&mut self, video: &Video, cx: &mut Context<Self>) {
        if self.running.contains_key(&video.id) {
            return;
        }
        let (id, title) = (video.id.clone(), video.title.clone());
        let (heard, mut shares) = unbounded_channel();
        let (client, wanted) = (self.client.clone(), video.clone());
        let fetch = Io::global(cx).spawn(async move {
            let Err(refused) = fetch(wanted.id.clone(), heard.clone()).await else {
                return Ok(());
            };
            let Some(client) = client else {
                return Err(refused);
            };
            log::warn!("downloads: yt-dlp failed, taking the stream itself: {refused:#}");
            stream(&client, &wanted, heard).await
        });
        Toasts::about(
            Outcome::Done,
            "toast-video-download-started",
            title.clone(),
            cx,
        );

        let task = cx.spawn({
            let id = id.clone();
            async move |this, cx| {
                while let Some(share) = shares.recv().await {
                    let known = this.update(cx, |this, cx| {
                        let Some(running) = this.running.get_mut(&id) else {
                            return;
                        };
                        // Only a whole percent more is worth a redraw.
                        if (share - running.0).abs() >= 0.01 {
                            running.0 = share;
                            cx.notify();
                            cx.refresh_windows();
                        }
                    });
                    if known.is_err() {
                        return;
                    }
                }
                let ended = join(fetch).await;
                let _ = this.update(cx, |this, cx| {
                    this.running.remove(&id);
                    match ended {
                        Ok(()) => {
                            Toasts::about(Outcome::Done, "toast-video-download-done", title, cx)
                        }
                        Err(error) => {
                            log::warn!("downloads: cannot save {id}: {error:#}");
                            Toasts::about(
                                Outcome::Failed,
                                "toast-video-download-failed",
                                title,
                                cx,
                            );
                        }
                    }
                    cx.notify();
                    cx.refresh_windows();
                });
            }
        });
        self.running.insert(id, (0., task));
        cx.notify();
        cx.refresh_windows();
    }

    /// Stops the download of the video and leaves what was fetched of it where it is.
    pub fn cancel(&mut self, video: &str, cx: &mut Context<Self>) {
        if self.running.remove(video).is_some() {
            cx.notify();
            cx.refresh_windows();
        }
    }
}

/// Runs yt-dlp on the video until the file is whole, sending each share fetched on the way.
/// Dropping the future stops yt-dlp.
async fn fetch(id: String, heard: UnboundedSender<f32>) -> Result<()> {
    let folder = VideoDownloads::folder().context("cannot find the downloads folder")?;
    tokio::fs::create_dir_all(&folder)
        .await
        .context("cannot make the downloads folder")?;

    let joiner = video::bundled_ffmpeg();
    let joins = joiner.is_some() || on_path("ffmpeg");
    let mut command = tokio::process::Command::new(video::ytdl());
    command
        .arg("--no-playlist")
        .arg("--newline")
        .arg("--no-warnings")
        .args([
            "--progress-template",
            &format!("download:{MARK}%(progress._percent_str)s"),
        ])
        .arg("--paths")
        .arg(&folder)
        .args(["--output", "%(title).150B [%(id)s].%(ext)s"]);
    match joins {
        // The sharper pictures come without sound, so they are only asked for when the two
        // can be joined into one file.
        true => command
            .args([
                "--format",
                &format!(
                    "bv*[height<={TALLEST}]+ba[format_note*=original]/bv*[height<={TALLEST}]+ba/b"
                ),
            ])
            // At a height, the kinds every player opens are taken ahead of the newer ones.
            .args(["--format-sort", "res,vcodec:h264,acodec:m4a"])
            .args(["--merge-output-format", "mp4"]),
        false => command.args(["--format", "b"]),
    };
    if let Some(joiner) = joiner {
        command.args(["--ffmpeg-location", &joiner]);
    }
    command
        .arg(format!("https://www.youtube.com/watch?v={id}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(NO_WINDOW);

    let mut child = command.spawn().context("cannot start yt-dlp")?;
    let printed = child.stdout.take().context("cannot hear yt-dlp")?;
    let mut lines = BufReader::new(printed).lines();
    // The picture and the sound are fetched one after the other, each counted from nothing, so
    // with two parts the picture, by far the larger, is given most of the way.
    let (mut part, mut last) = (0u8, 0f32);
    while let Some(line) = lines.next_line().await? {
        let Some(share) = line
            .strip_prefix(MARK)
            .and_then(|share| share.trim().trim_end_matches('%').parse::<f32>().ok())
        else {
            continue;
        };
        let share = share / 100.;
        if share < last - 0.5 {
            part += 1;
        }
        last = share;
        let whole = match (joins, part) {
            (false, _) => share,
            (true, 0) => share * PICTURE,
            (true, _) => PICTURE + share * (1. - PICTURE),
        };
        let _ = heard.send(whole.clamp(0., 1.));
    }
    let ended = child.wait_with_output().await?;
    if !ended.status.success() {
        let said = String::from_utf8_lossy(&ended.stderr);
        bail!("{}", said.lines().last().unwrap_or("yt-dlp failed"));
    }
    Ok(())
}

/// Saves the video from the stream the player itself would show, joined by ffmpeg, for when
/// YouTube turns yt-dlp away.
async fn stream(client: &tube::Client, video: &Video, heard: UnboundedSender<f32>) -> Result<()> {
    let folder = VideoDownloads::folder().context("cannot find the downloads folder")?;
    let program = match video::bundled_ffmpeg() {
        Some(folder) => PathBuf::from(folder).join("ffmpeg"),
        None => PathBuf::from("ffmpeg"),
    };
    let qualities = client.streams(&video.id).await?.qualities?;
    let quality = qualities
        .iter()
        .filter(|quality| quality.height <= TALLEST)
        .max_by_key(|quality| (quality.height, quality.rate))
        .or_else(|| qualities.first())
        .context("the video has no stream to save")?;
    let name: String = video
        .title
        .chars()
        .filter(|letter| !letter.is_control() && !UNSAFE.contains(*letter))
        .take(NAME_MOST)
        .collect();
    let file = folder.join(format!("{} [{}].mp4", name.trim(), video.id));
    let length = video
        .length
        .as_deref()
        .map(seconds)
        .filter(|length| *length > 0.);

    let mut command = tokio::process::Command::new(program);
    command
        .args([
            "-y",
            "-loglevel",
            "error",
            "-nostats",
            "-progress",
            "pipe:1",
        ])
        .args(["-i", &quality.picture, "-i", &quality.sound])
        .args(["-map", "0:v:0", "-map", "1:a:0", "-c", "copy"])
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(NO_WINDOW);

    let mut child = command.spawn().context("cannot start ffmpeg")?;
    let printed = child.stdout.take().context("cannot hear ffmpeg")?;
    let mut lines = BufReader::new(printed).lines();
    while let Some(line) = lines.next_line().await? {
        let written = line
            .strip_prefix("out_time_us=")
            .and_then(|written| written.trim().parse::<f64>().ok());
        if let (Some(written), Some(length)) = (written, length) {
            let _ = heard.send((written / 1_000_000. / length).clamp(0., 1.) as f32);
        }
    }
    let ended = child.wait_with_output().await?;
    if !ended.status.success() {
        let said = String::from_utf8_lossy(&ended.stderr);
        bail!("{}", said.lines().last().unwrap_or("ffmpeg failed"));
    }
    Ok(())
}

/// The seconds in a clock text such as `1:02:35`.
fn seconds(clock: &str) -> f64 {
    clock.split(':').fold(0., |whole, part| {
        whole * 60. + part.trim().parse::<f64>().unwrap_or(0.)
    })
}

/// Whether the PATH holds a program of this name.
fn on_path(name: &str) -> bool {
    let name = match cfg!(windows) {
        true => format!("{name}.exe"),
        false => name.to_owned(),
    };
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|folder| folder.join(&name).is_file()))
}

/// The letters a file name cannot hold on one system or another, and the most of a title it
/// keeps.
const UNSAFE: &str = "<>:\"/\\|?*";
const NAME_MOST: usize = 120;
/// How much of a download in two parts is the picture.
const PICTURE: f32 = 0.9;
/// Keeps Windows from opening a console window for yt-dlp.
#[cfg(windows)]
const NO_WINDOW: u32 = 0x0800_0000;
