use gpui::App;
use state::Sonora;

/// Whether the signed-in provider is Jellyfin, whose device list is its remote control and
/// takes its device name from the sign-in.
pub(super) fn jellyfin(cx: &App) -> bool {
    Sonora::global(cx).session.read(cx).provider_slug() == Some("jellyfin")
}
