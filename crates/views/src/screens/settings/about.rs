use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::{AnyElement, Context, FontWeight, SharedString, div};
use i18n::t;
use ui::{ActiveTheme as _, Text};

use super::rows::outward;
use super::{Group, SettingsView, Slot};

impl SettingsView {
    pub(super) fn about_groups(&self) -> Vec<Group> {
        vec![Group::untitled().rows([Slot::Version, Slot::Log])]
    }

    /// The version row, which for a nightly also names the commit it was built from.
    pub(super) fn version_row(&self) -> AnyElement {
        let version: SharedString = match state::commit() {
            Some(commit) => t!(
                "settings-version-commit",
                version = state::version(),
                commit = commit
            ),
            None => state::version().into(),
        };
        self.row(
            t!("settings-version"),
            ui::spoken("settings-version-value", version).into_any_element(),
        )
    }

    /// The row that opens a copy of the current log file in whatever the system reads text
    /// with. It is disabled when the platform names no state or cache folder to log into.
    pub(super) fn log_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let path = state::log_file();

        outward("open-log", t!("settings-log-open"), None, cx)
            .disabled(path.is_none())
            .on_click(move |_, _, _| {
                let Some(path) = &path else { return };
                if let Err(error) = snapshot(path).and_then(open::that_detached) {
                    log::warn!(
                        "settings: cannot open a copy of {}: {error}",
                        path.display()
                    );
                }
            })
            .into_any_element()
    }

    /// Opens About with the app's name, aligned with the settings below.
    pub(super) fn mark(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();

        div()
            .id("settings-about-name")
            .role(gpui::accesskit::Role::Heading)
            .aria_level(2)
            .aria_label(t!("common-sonora"))
            .text_size(theme.text(Text::Title))
            .font_weight(FontWeight::SEMIBOLD)
            .child(t!("common-sonora"))
    }
}

/// Notepad on Windows refuses a file that Sonora still writes to, so the button opens a copy.
/// `File::open` shares write access, and `fs::copy` documents no share mode.
fn snapshot(log: &Path) -> io::Result<PathBuf> {
    let copy = log.with_file_name("youtube-plus.copy.log");
    io::copy(&mut File::open(log)?, &mut File::create(&copy)?)?;
    Ok(copy)
}
