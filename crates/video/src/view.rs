//! The GPUI side of a video: an entity that repaints on every new frame.

use std::sync::Arc;

use futures::StreamExt as _;
use gpui::prelude::*;
use gpui::{Context, ObjectFit, RenderImage, Task, Window, div, img};

use crate::{Frames, Player};

/// Draws a [`Player`]'s frames, letterboxed into whatever space its parent gives it.
pub struct VideoView {
    player: Arc<Player>,
    /// The frame painted last, kept so its texture can be freed once a newer one replaces it.
    shown: Option<Arc<RenderImage>>,
    _frames: Task<()>,
}

impl VideoView {
    pub fn new(player: Arc<Player>, mut frames: Frames, cx: &mut Context<Self>) -> Self {
        let task = cx.spawn(async move |this, cx| {
            while frames.next().await.is_some() {
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            player,
            shown: None,
            _frames: task,
        }
    }

    pub fn player(&self) -> &Arc<Player> {
        &self.player
    }
}

impl Render for VideoView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.player.frame();
        if let Some(previous) = self.shown.take()
            && frame.as_ref().is_none_or(|frame| frame.id != previous.id)
            && let Err(error) = window.drop_image(previous)
        {
            log::warn!("video: cannot free a frame: {error:#}");
        }
        self.shown.clone_from(&frame);

        div().size_full().when_some(frame, |this, frame| {
            this.child(img(frame).size_full().object_fit(ObjectFit::Contain))
        })
    }
}
