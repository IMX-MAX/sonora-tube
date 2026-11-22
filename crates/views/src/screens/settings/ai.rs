//! The rows that set up the AI chat and chapters: the Mistral key and the models they run on.

use gpui::prelude::*;
use gpui::{AnyElement, Context, Entity, SharedString, div, px};
use i18n::t;
use state::Assistant;
use ui::{ActiveTheme as _, Button, MenuItem, Picker, Switch, Text};

use super::SettingsView;

const CHAT_MODEL: &str = "ai-chat-model";
const CHAPTER_MODEL: &str = "ai-chapter-model";
/// How wide the key field stands.
const KEY_WIDTH: gpui::Pixels = px(220.);
/// How big the dot saying whether the key works stands.
const DOT: gpui::Pixels = px(6.);

impl SettingsView {
    pub(super) fn ai_key_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(assistant) = Assistant::entity(cx) else {
            return div().into_any_element();
        };
        let held = assistant.read(cx);
        let (hint, refused) = (held.hint(), held.refused());
        let typed = !self.ai_key.read(cx).text().trim().is_empty();
        let control = match hint {
            Some(hint) => div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(match refused {
                            true => theme.danger,
                            false => theme.border,
                        })
                        .bg(theme.secondary)
                        .text_size(theme.text(Text::Small))
                        .child(div().size(DOT).rounded_full().bg(match refused {
                            true => theme.danger,
                            false => theme.primary,
                        }))
                        .child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(match refused {
                                    true => t!("settings-ai-key-refused"),
                                    false => t!("settings-ai-key-saved"),
                                }),
                        )
                        .child(div().text_color(theme.foreground).child(hint)),
                )
                .child(
                    Button::new("settings-ai-key-remove")
                        .label(t!("settings-ai-key-remove"))
                        .small()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            assistant.update(cx, |assistant, cx| assistant.set_key("", cx));
                        }),
                ),
            None => {
                let field = self.ai_key.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(KEY_WIDTH).child(field.clone()))
                    .child(
                        Button::new("settings-ai-key-save")
                            .label(t!("settings-ai-key-save"))
                            .small()
                            .secondary()
                            .disabled(!typed)
                            .on_click(move |_, _, cx| save(&field, &assistant, cx)),
                    )
            }
        };
        self.row(t!("settings-ai-key"), control.into_any_element())
    }

    pub(super) fn ai_chat_model_row(&self, cx: &mut Context<Self>) -> AnyElement {
        self.model_row(CHAT_MODEL, t!("settings-ai-chat-model"), false, cx)
    }

    pub(super) fn ai_chapter_model_row(&self, cx: &mut Context<Self>) -> AnyElement {
        self.model_row(CHAPTER_MODEL, t!("settings-ai-chapter-model"), true, cx)
    }

    pub(super) fn auto_chapters_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(assistant) = Assistant::entity(cx) else {
            return div().into_any_element();
        };
        let on = assistant.read(cx).auto_chapters();
        self.row(
            t!("settings-ai-auto-chapters"),
            Switch::new("ai-auto-chapters", on)
                .name(t!("settings-ai-auto-chapters"))
                .on_click(move |_, _, cx| {
                    assistant.update(cx, |assistant, cx| assistant.set_auto_chapters(!on, cx));
                })
                .into_any_element(),
        )
    }

    /// A picker of the models the key can use, for the chat or for chapters.
    fn model_row(
        &self,
        key: &'static str,
        title: SharedString,
        chapters: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(assistant) = Assistant::entity(cx) else {
            return div().into_any_element();
        };
        let held = assistant.read(cx);
        let chosen = match chapters {
            true => held.chapter_model(),
            false => held.chat_model(),
        }
        .to_owned();
        let picker = Picker::new(key, &self.popovers, chosen.clone())
            .width(Picker::WIDE)
            .items(held.models().into_iter().enumerate().map(|(place, model)| {
                let assistant = assistant.clone();
                MenuItem::new((key, place), model.clone())
                    .selected(model == chosen)
                    .on_click(move |_, _, cx| {
                        let model = model.clone();
                        assistant.update(cx, |assistant, cx| match chapters {
                            true => assistant.set_chapter_model(model, cx),
                            false => assistant.set_chat_model(model, cx),
                        });
                    })
            }));
        self.row(title.clone(), picker.name(title).into_any_element())
    }
}

/// Saves the key typed in `field` and empties it, so the key is never shown again.
fn save(field: &Entity<ui::Input>, assistant: &Entity<Assistant>, cx: &mut gpui::App) {
    let key = field.read(cx).text().trim().to_owned();
    if key.is_empty() {
        return;
    }
    assistant.update(cx, |assistant, cx| assistant.set_key(&key, cx));
    field.update(cx, |input, cx| input.set_text(String::new(), cx));
}
