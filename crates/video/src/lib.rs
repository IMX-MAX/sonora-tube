//! Video playback: libmpv decodes and times the frames, and a GPUI view draws them.
//!
//! [`Player`] is the mpv side and knows nothing about GPUI beyond the image type it hands over.
//! [`VideoView`] is the element side: it wakes on every new frame and paints the latest one.

mod player;
mod view;

pub use player::{Frames, Player};
pub use view::VideoView;
