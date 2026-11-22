//! Plays one file or url in a bare window: `cargo run -p video --example watch -- <source>`.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::{App, Bounds, WindowBounds, WindowOptions, black, div, px, size};
use gpui_platform::application;
use video::{Player, VideoView};

struct Watch {
    video: gpui::Entity<VideoView>,
}

impl Render for Watch {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().bg(black()).child(self.video.clone())
    }
}

fn main() {
    env_logger::init();
    let source = std::env::args().nth(1).expect("usage: watch <file or url>");
    application().run(move |cx: &mut App| {
        let (player, frames) = Player::new().expect("mpv starts");
        player.load_from(&source, 0.).expect("source loads");
        let player = Arc::new(player);
        let bounds = Bounds::centered(None, size(px(1280.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                let video = cx.new(|cx| VideoView::new(player, frames, cx));
                cx.new(|_| Watch { video })
            },
        )
        .expect("window opens");
        cx.activate(true);
    });
}
