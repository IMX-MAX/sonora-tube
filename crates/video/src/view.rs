//! The GPUI side of a video: an entity that repaints on every new frame.

use std::sync::Arc;

use futures::StreamExt as _;
use gpui::prelude::*;
use gpui::{Context, Entity, ObjectFit, Pixels, RenderImage, Subscription, Task, Window, div, img};

use crate::{Frames, Player};

/// Draws a [`Player`]'s frames, letterboxed into whatever space its parent gives it.
pub struct VideoView {
    player: Arc<Player>,
    /// The frame painted last, kept so its texture can be freed once a newer one replaces it.
    shown: Option<Arc<RenderImage>>,
    /// How round the picture's corners are drawn.
    radius: Pixels,
    /// What repaints it: the player's own frames, or for a mirror the view they repaint.
    _frames: Option<Task<()>>,
    _source: Option<Subscription>,
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
            radius: Pixels::ZERO,
            _frames: Some(task),
            _source: None,
        }
    }

    /// Rounds the picture's corners by `radius`, which a clipping parent cannot do for it.
    pub fn round(&mut self, radius: Pixels, cx: &mut Context<Self>) {
        if self.radius != radius {
            self.radius = radius;
            cx.notify();
        }
    }

    /// A second picture of `source`'s player for another window, repainted whenever `source` is.
    /// A frame's texture belongs to the window it was painted in, so each window needs its own.
    pub fn mirror(source: &Entity<Self>, cx: &mut Context<Self>) -> Self {
        Self {
            player: source.read(cx).player.clone(),
            shown: None,
            radius: Pixels::ZERO,
            _frames: None,
            _source: Some(cx.observe(source, |_, _, cx| cx.notify())),
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
            this.child(
                img(frame)
                    .size_full()
                    .rounded(self.radius)
                    .object_fit(ObjectFit::Contain),
            )
        })
    }
}
