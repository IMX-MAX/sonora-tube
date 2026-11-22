# YouTube+

A native desktop YouTube client written in Rust with GPUI and libmpv.

- search, watch pages, channels, playlists, history and Watch later
- comments and transcripts in side panels
- captions, theater mode, full screen and picture-in-picture
- themes, corner styles and icon packs

## Building

```sh
cargo run --locked --package sonora
```

libmpv and yt-dlp have to be installed. The Windows installers are built by
`.github/workflows/windows.yml`.

## Licence

GPL-3.0, see `COPYING`. Third-party notices are in `THIRD-PARTY.md`.
