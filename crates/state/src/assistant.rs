//! The Mistral key and the models the AI features use, kept apart from the other settings so
//! the key sits in a file only this user can read.

use std::path::PathBuf;

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use tube::ai::{MODEL, MODELS, Mistral};

use crate::tube::write_private;
use crate::{Io, join};

/// Where the AI choices are written, under the app's data folder.
const CHOICES: &str = "ai.json";
/// Where the key is written, beside the accounts' cookies.
const KEY: &str = "accounts/mistral-key";

struct Held(Entity<Assistant>);

impl Global for Held {}

/// The models each AI feature runs on, and whether chapters are made on their own.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Choices {
    chat: String,
    chapters: String,
    auto_chapters: bool,
}

impl Default for Choices {
    fn default() -> Self {
        Self {
            chat: MODEL.to_owned(),
            chapters: MODEL.to_owned(),
            auto_chapters: true,
        }
    }
}

/// The Mistral key and model choices the AI chat and chapters read, shared app-wide.
pub struct Assistant {
    io: Io,
    folder: PathBuf,
    key: Option<String>,
    choices: Choices,
    /// The models the key can use, once Mistral listed them.
    models: Vec<String>,
    /// Whether Mistral turned the key down the last time it was used.
    refused: bool,
    listing: Option<Task<()>>,
}

impl Assistant {
    /// The app's one assistant, made the first time it is asked for.
    pub fn shared(io: Io, cx: &mut App) -> Entity<Self> {
        if let Some(held) = cx.try_global::<Held>() {
            return held.0.clone();
        }
        let made = cx.new(|cx| {
            let mut made = Self::new(io);
            made.list(cx);
            made
        });
        cx.set_global(Held(made.clone()));
        made
    }

    /// The app's assistant, once [`Assistant::shared`] ran.
    pub fn entity(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<Held>().map(|held| held.0.clone())
    }

    fn new(io: Io) -> Self {
        let folder = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("youtube-plus");
        let key = std::fs::read_to_string(folder.join(KEY))
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty());
        let choices = std::fs::read(folder.join(CHOICES))
            .ok()
            .and_then(|read| serde_json::from_slice(&read).ok())
            .unwrap_or_default();
        Self {
            io,
            folder,
            key,
            choices,
            models: Vec::new(),
            refused: false,
            listing: None,
        }
    }

    /// Whether a key is saved, which every AI feature needs.
    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    /// Whether Mistral turned the saved key down.
    pub fn refused(&self) -> bool {
        self.refused
    }

    /// The saved key with all but its last four letters hidden, for showing that one is set.
    pub fn hint(&self) -> Option<String> {
        let key = self.key.as_deref()?;
        let tail: String = key
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Some(format!("•••• {tail}"))
    }

    /// Mistral reached with the saved key, `None` without one.
    pub fn client(&self) -> Option<Mistral> {
        self.key.clone().map(Mistral::new)
    }

    /// Saves `key`, or forgets the saved one when it is blank, and lists its models.
    pub fn set_key(&mut self, key: &str, cx: &mut Context<Self>) {
        let key = key.trim();
        let path = self.folder.join(KEY);
        self.key = match key.is_empty() {
            true => {
                let _ = std::fs::remove_file(&path);
                None
            }
            false => {
                if let Err(error) = write_private(&path, key.as_bytes()) {
                    log::warn!("assistant: cannot save the mistral key: {error}");
                }
                Some(key.to_owned())
            }
        };
        self.refused = false;
        self.models.clear();
        self.list(cx);
        cx.notify();
    }

    /// Notes that Mistral turned the key down, which settings then point out.
    pub fn mark_refused(&mut self, cx: &mut Context<Self>) {
        if !self.refused {
            self.refused = true;
            cx.notify();
        }
    }

    /// The models the pickers offer: the key's own once listed, the usual ones before that,
    /// and the chosen ones always.
    pub fn models(&self) -> Vec<String> {
        let mut models: Vec<String> = match self.models.is_empty() {
            true => MODELS.iter().map(|&model| model.to_owned()).collect(),
            false => self.models.clone(),
        };
        for chosen in [&self.choices.chat, &self.choices.chapters] {
            if !models.contains(chosen) {
                models.insert(0, chosen.clone());
            }
        }
        models
    }

    pub fn chat_model(&self) -> &str {
        &self.choices.chat
    }

    pub fn chapter_model(&self) -> &str {
        &self.choices.chapters
    }

    /// Whether videos without chapters get ones the AI cuts from their transcript.
    pub fn auto_chapters(&self) -> bool {
        self.choices.auto_chapters
    }

    pub fn set_chat_model(&mut self, model: String, cx: &mut Context<Self>) {
        self.choices.chat = model;
        self.save(cx);
    }

    pub fn set_chapter_model(&mut self, model: String, cx: &mut Context<Self>) {
        self.choices.chapters = model;
        self.save(cx);
    }

    pub fn set_auto_chapters(&mut self, on: bool, cx: &mut Context<Self>) {
        self.choices.auto_chapters = on;
        self.save(cx);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let written = serde_json::to_vec_pretty(&self.choices)
            .map_err(std::io::Error::other)
            .and_then(|json| {
                std::fs::create_dir_all(&self.folder)?;
                std::fs::write(self.folder.join(CHOICES), json)
            });
        if let Err(error) = written {
            log::warn!("assistant: cannot save the ai choices: {error}");
        }
        cx.notify();
    }

    /// Asks Mistral which models the saved key can use.
    fn list(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client() else {
            self.listing = None;
            return;
        };
        let request = self.io.spawn(async move { client.models().await });
        self.listing = Some(cx.spawn(async move |this, cx| {
            let listed = join(request).await;
            this.update(cx, |this, cx| {
                match listed {
                    Ok(models) => this.models = models,
                    Err(error) => {
                        log::warn!("assistant: cannot list the mistral models: {error:#}");
                        this.refused = format!("{error:#}").contains("API key");
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }
}
