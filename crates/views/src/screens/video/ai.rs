//! The AI pages of the watch page's side panel, the chat about the video and its chapters, and
//! the chapter named beside the time.

use std::ops::Range;
use std::time::Duration;

use gpui::accesskit::Role;
use gpui::prelude::*;
use gpui::{
    AnyElement, Bounds, ClipboardItem, Context, ElementId, Entity, FontStyle, FontWeight,
    HighlightStyle, Hsla, InteractiveText, PathBuilder, Pixels, SharedString, StrikethroughStyle,
    StyledText, TextLayout, UnderlineStyle, Window, canvas, div, point, px, relative, svg,
};
use i18n::t;
use router::{Destination, SettingsPage, navigate};
use state::{Assistant, Outcome, Toasts, Watching};
use tube::ai::Speaker;
use ui::{ActiveTheme as _, Button, Notice, Spinner, Submit, Text, Theme, Vacancy, clock};

use super::markdown::{self, Block, Rich};
use super::{Panel, WatchView};

/// How big a chapter's number stands.
const BADGE: Pixels = px(22.);
/// How far a moment's badge leans from the theme's colour to the page behind it.
const TINT: f32 = 0.8;
/// How thick the line under an address in an answer stands.
const LINE: Pixels = px(1.);
/// How many pieces one answer may draw before its ids run into the next answer's.
const PIECES: usize = 10_000;
/// How wide the name of the chapter playing may run beside the time.
const CHAPTER_NOW: Pixels = px(220.);
/// How thick the progress through the chapter playing stands.
const PROGRESS: Pixels = px(3.);
/// How big the dot before the chapter playing stands.
const DOT: Pixels = px(4.);
/// The questions offered before the first one is asked.
const SUGGESTIONS: [&str; 3] = [
    "video-ai-suggest-summary",
    "video-ai-suggest-points",
    "video-ai-suggest-moment",
];

/// A space the line never breaks at.
const SPACE: &str = "\u{a0}";
/// The room kept at the start of a moment's badge, a space either side of its triangle's.
const LEAD: &str = "\u{a0}\u{a0}\u{a0}\u{a0}";
/// The room kept at the end of a moment's badge.
const TAIL: &str = SPACE;

/// A moment or a stretch of the video named in a line, drawn as a badge that seeks.
#[derive(Clone)]
struct Badge {
    /// Where in the line the badge stands.
    range: Range<usize>,
    from: f64,
    /// Where the stretch ends, when the badge names one.
    until: Option<f64>,
}

/// Draws the blocks of one answer, numbering what it draws so each piece keeps its own id.
pub(super) struct Pen {
    pub theme: Theme,
    pub watching: Entity<Watching>,
    /// Which message of the chat the answer is, or a number of its own for text elsewhere.
    pub place: usize,
    pub drawn: usize,
}

impl Pen {
    /// An id no other piece of the chat has.
    fn id(&mut self, kind: &'static str) -> ElementId {
        self.drawn += 1;
        (
            kind,
            self.place.wrapping_mul(PIECES).wrapping_add(self.drawn),
        )
            .into()
    }

    /// `blocks` one under the other.
    fn blocks(&mut self, blocks: &[Block]) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .min_w_0()
            .text_size(self.theme.text(Text::Label))
            .children(blocks.iter().map(|block| self.block(block)))
            .into_any_element()
    }

    fn block(&mut self, block: &Block) -> AnyElement {
        let theme = self.theme;
        match block {
            Block::Paragraph(line) => self.line(line),
            Block::Heading(level, line) => div()
                .pt_1()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(theme.text(match level {
                    1 | 2 => Text::Body,
                    _ => Text::Label,
                }))
                .child(self.line(line))
                .into_any_element(),
            Block::List(first, items) => div()
                .flex()
                .flex_col()
                .gap_1()
                .children(items.iter().enumerate().map(|(at, item)| {
                    let mark = match first {
                        Some(first) => format!("{}.", first + at as u64),
                        None => "•".to_owned(),
                    };
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .text_color(theme.muted_foreground)
                                .child(mark),
                        )
                        .child(div().flex_1().min_w_0().child(self.blocks(item)))
                }))
                .into_any_element(),
            Block::Quote(quoted) => div()
                .pl_3()
                .border_l_2()
                .border_color(theme.border)
                .text_color(theme.muted_foreground)
                .child(self.blocks(quoted))
                .into_any_element(),
            Block::Code(code) => self.code(code),
            Block::Table(head, rows) => self.table(head, rows),
            Block::Math(math) => div()
                .w_full()
                .py_1()
                .flex()
                .justify_center()
                .text_size(theme.text(Text::Body))
                .italic()
                .child(SharedString::from(math.clone()))
                .into_any_element(),
            Block::Rule => div().w_full().h(LINE).bg(theme.border).into_any_element(),
        }
    }

    /// A block of code in a box of its own, with a button copying it.
    fn code(&mut self, code: &str) -> AnyElement {
        let theme = self.theme;
        let copied = code.to_owned();
        div()
            .w_full()
            .flex()
            .items_start()
            .gap_1()
            .p_2()
            .rounded(theme.radius)
            .bg(theme.muted)
            .text_size(theme.text(Text::Small))
            .child(
                div()
                    .id(self.id("video-ai-code"))
                    .flex_1()
                    .min_w_0()
                    .overflow_x_scroll()
                    .child(SharedString::from(code.to_owned())),
            )
            .child(
                Button::new(self.id("video-ai-copy-code"))
                    .icon("icons/copy.svg")
                    .small()
                    .ghost()
                    .tooltip("video-ai-copy-code")
                    .on_click(move |_, _, cx| copy(copied.clone(), cx)),
            )
            .into_any_element()
    }

    /// A table drawn the way the app's lists are headed, with a button copying its cells so
    /// they paste into a spreadsheet.
    fn table(&mut self, head: &[Rich], rows: &[Vec<Rich>]) -> AnyElement {
        let theme = self.theme;
        let columns = head.len().max(1) as u16;
        let copied = markdown::cells(head, rows);
        let last = rows.len();
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_end()
            .gap_1()
            .child(
                div()
                    .id(self.id("video-ai-table"))
                    .role(Role::Table)
                    .w_full()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.table_row_border)
                    .overflow_hidden()
                    .text_size(theme.text(Text::Small))
                    .aria_label(t!("video-ai-table"))
                    .child(
                        div()
                            .id(self.id("video-ai-table-head"))
                            .role(Role::Row)
                            .aria_label(row_label(head))
                            .grid()
                            .grid_cols(columns)
                            .bg(theme.table_head)
                            .text_color(theme.table_head_foreground)
                            .font_weight(FontWeight::MEDIUM)
                            .children(head.iter().map(|cell| self.cell(Role::ColumnHeader, cell))),
                    )
                    .children(rows.iter().enumerate().map(|(at, row)| {
                        div()
                            .id(self.id("video-ai-table-row"))
                            .role(Role::Row)
                            .aria_label(row_label(row))
                            .grid()
                            .grid_cols(columns)
                            .border_t_1()
                            .border_color(theme.table_row_border)
                            .when(at + 1 == last, |this| this.rounded_b(theme.radius))
                            .children(row.iter().map(|cell| self.cell(Role::Cell, cell)))
                    })),
            )
            .child(
                Button::new(self.id("video-ai-copy-table"))
                    .icon("icons/copy.svg")
                    .label(t!("video-ai-copy-table"))
                    .small()
                    .ghost()
                    .on_click(move |_, _, cx| copy(copied.clone(), cx)),
            )
            .into_any_element()
    }

    /// One cell of a table, read out as the head of its column or a cell under one.
    fn cell(&mut self, role: Role, cell: &Rich) -> AnyElement {
        div()
            .id(self.id("video-ai-table-cell"))
            .role(role)
            .aria_label(SharedString::from(cell.text.clone()))
            .min_w_0()
            .px_2()
            .py_1p5()
            .child(self.line(cell))
            .into_any_element()
    }

    /// A line of text with its bold, italics and code. Its addresses open when pressed, and its
    /// moments stand as badges that seek: two joined by a dash are one badge for the stretch
    /// between them, which stays marked on the seek bar until it has played.
    pub(super) fn line(&mut self, line: &Rich) -> AnyElement {
        let theme = self.theme;
        let mut presses = tube::marks(&line.text);
        for (range, look) in &line.looks {
            let Some(address) = &look.link else { continue };
            presses.retain(|(other, _)| other.end <= range.start || other.start >= range.end);
            presses.push((range.clone(), tube::Mark::Link(address.clone())));
        }
        // A moment inside code is the code's own text, not somewhere to seek to.
        presses.retain(|(range, _)| {
            !line.looks.iter().any(|(coded, look)| {
                look.code && coded.start < range.end && range.start < coded.end
            })
        });
        presses.sort_by_key(|(range, _)| range.start);

        let mut links = Vec::new();
        let mut badges: Vec<Badge> = Vec::new();
        for (range, mark) in presses {
            let seconds = match mark {
                tube::Mark::Link(address) => {
                    links.push((range, address));
                    continue;
                }
                tube::Mark::Moment(seconds) => seconds,
            };
            // Only a dash between two moments makes them the ends of one stretch, whether or
            // not each moment sits in brackets of its own.
            let joined = badges.last_mut().filter(|last| {
                let gap = &line.text[last.range.end..range.start];
                let between = gap
                    .trim()
                    .trim_start_matches([']', ')'])
                    .trim_end_matches(['[', '(']);
                // A dash opening the next line is a list bullet, not a join.
                last.until.is_none()
                    && seconds > last.from
                    && !gap.contains('\n')
                    && matches!(between.trim(), "-" | "–" | "—")
            });
            match joined {
                Some(last) => {
                    last.range.end = range.end;
                    last.until = Some(seconds);
                }
                None => badges.push(Badge {
                    range,
                    from: seconds,
                    until: None,
                }),
            }
        }

        // Room is made inside each badge for its triangle, which moves everything after it.
        let mut text = String::with_capacity(line.text.len());
        let mut added = Vec::new();
        let mut copied = 0;
        for badge in &badges {
            text.push_str(&line.text[copied..badge.range.start]);
            text.push_str(LEAD);
            text.push_str(&line.text[badge.range.clone()]);
            text.push_str(TAIL);
            copied = badge.range.end;
            added.push((badge.range.start, LEAD.len()));
            added.push((badge.range.end, TAIL.len()));
        }
        text.push_str(&line.text[copied..]);
        let moved = |at: usize| {
            let before = added.iter().filter(|(place, _)| *place < at);
            at + before.map(|(_, length)| length).sum::<usize>()
        };
        let moved_range = |range: &Range<usize>| moved(range.start)..moved(range.end);
        let looks: Vec<_> = line
            .looks
            .iter()
            .map(|(range, look)| (moved_range(range), look))
            .collect();
        let links: Vec<_> = links
            .into_iter()
            .map(|(range, address)| (moved_range(&range), address))
            .collect();
        for badge in &mut badges {
            badge.range = moved(badge.range.start)..moved(badge.range.end) + TAIL.len();
        }

        // Highlights may not overlap, so the line is cut wherever any look starts or ends.
        let mut cuts: Vec<usize> = looks
            .iter()
            .map(|(range, _)| range)
            .chain(links.iter().map(|(range, _)| range))
            .chain(badges.iter().map(|badge| &badge.range))
            .flat_map(|range| [range.start, range.end])
            .collect();
        cuts.sort_unstable();
        cuts.dedup();
        let highlights: Vec<_> = cuts
            .windows(2)
            .filter_map(|cut| {
                let (from, to) = (cut[0], cut[1]);
                let within = |range: &Range<usize>| range.start <= from && to <= range.end;
                let mut style = HighlightStyle::default();
                let mut any = false;
                for (_, look) in looks.iter().filter(|(range, _)| within(range)) {
                    any = true;
                    if look.bold {
                        style.font_weight = Some(FontWeight::SEMIBOLD);
                    }
                    if look.italic {
                        style.font_style = Some(FontStyle::Italic);
                    }
                    if look.code {
                        style.background_color = Some(theme.muted);
                    }
                    if look.struck {
                        style.strikethrough = Some(StrikethroughStyle {
                            thickness: LINE,
                            ..Default::default()
                        });
                    }
                }
                if badges.iter().any(|badge| within(&badge.range)) {
                    any = true;
                    style.color = Some(theme.progress_bar);
                    style.font_weight = Some(FontWeight::SEMIBOLD);
                } else if links.iter().any(|(range, _)| within(range)) {
                    any = true;
                    style.color = Some(theme.progress_bar);
                    style.underline = Some(UnderlineStyle {
                        thickness: LINE,
                        ..Default::default()
                    });
                }
                any.then_some((from..to, style))
            })
            .collect();

        let ranges = links
            .iter()
            .map(|(range, _)| range.clone())
            .chain(badges.iter().map(|badge| badge.range.clone()))
            .collect();
        let watching = self.watching.clone();
        let styled = StyledText::new(SharedString::from(text)).with_highlights(highlights);
        let letters = styled.layout().clone();
        // The text only turns the pointer into a hand over a link; the press is read by the
        // box around it, which also hears a click a redraw falls inside of, as one does on
        // every frame of a playing video.
        let words =
            InteractiveText::new(self.id("video-ai-line"), styled).on_click(ranges, |_, _, _| {});
        let backs = {
            let (letters, badges) = (letters.clone(), badges.clone());
            let fill = ui::motion::mix(theme.progress_bar, theme.background, TINT);
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    for badge in &badges {
                        back(badge, &letters, bounds, fill, &theme, window);
                    }
                },
            )
            .absolute()
            .inset_0()
        };
        div()
            .id(self.id("video-ai-words"))
            .relative()
            .on_click(move |event, _, cx| {
                let Ok(at) = letters.index_for_position(event.position()) else {
                    return;
                };
                if let Some(badge) = badges.iter().find(|badge| badge.range.contains(&at)) {
                    let (from, until) = (badge.from, badge.until.unwrap_or(badge.from));
                    watching.update(cx, |watching, cx| watching.point_at(from, until, cx));
                } else if let Some((_, address)) =
                    links.iter().find(|(range, _)| range.contains(&at))
                {
                    cx.open_url(address);
                }
            })
            .child(backs)
            .child(words)
            .into_any_element()
    }
}

/// Paints what stands behind a moment's badge: its rounded back, and the triangle in the room
/// kept at its start. A badge the line breaks inside of is backed on both of its lines.
fn back(
    badge: &Badge,
    letters: &TextLayout,
    within: Bounds<Pixels>,
    fill: Hsla,
    theme: &Theme,
    window: &mut Window,
) {
    let (Some(start), Some(end)) = (
        letters.position_for_index(badge.range.start),
        letters.position_for_index(badge.range.end),
    ) else {
        return;
    };
    let height = letters.line_height();
    let radius = theme.radius.min(height / 2.);
    let mut piece = |left: Pixels, right: Pixels, top: Pixels| {
        let bounds = Bounds::from_corners(point(left, top), point(right, top + height));
        window.paint_quad(gpui::fill(bounds, fill).corner_radii(radius));
    };
    match start.y == end.y {
        true => piece(start.x, end.x, start.y),
        false => {
            piece(start.x, within.right(), start.y);
            piece(within.left(), end.x, end.y);
        }
    }

    // The triangle fills the middle of the room the leading spaces keep.
    let (Some(left), Some(right)) = (
        letters.position_for_index(badge.range.start + SPACE.len()),
        letters.position_for_index(badge.range.start + LEAD.len() - SPACE.len()),
    ) else {
        return;
    };
    if left.y != start.y || right.y != start.y {
        return;
    }
    let side = (right.x - left.x).min(height / 2.);
    let top = start.y + (height - side) / 2.;
    let mut path = PathBuilder::fill();
    path.add_polygon(
        &[
            point(left.x, top),
            point(left.x + side, top + side / 2.),
            point(left.x, top + side),
        ],
        true,
    );
    if let Ok(path) = path.build() {
        window.paint_path(path, theme.progress_bar);
    }
}

impl WatchView {
    /// Sends what the question field holds to the AI and empties it.
    pub(super) fn send(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        let question = self.query.read(cx).text().to_owned();
        if question.trim().is_empty() || self.watching.read(cx).asking() {
            return;
        }
        self.watching
            .update(cx, |watching, cx| watching.ask(&question, cx));
        self.query
            .update(cx, |input, cx| input.set_text(String::new(), cx));
    }

    /// The chat about the video: its messages over the field a question is written in, or what
    /// to set up first when no key is saved.
    pub(super) fn chat(&self, cx: &Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let assistant = Assistant::entity(cx);
        let (keyed, refused) = assistant
            .as_ref()
            .map(|assistant| {
                let assistant = assistant.read(cx);
                (assistant.has_key(), assistant.refused())
            })
            .unwrap_or_default();
        if !keyed {
            return self.setup();
        }
        let watching = self.watching.read(cx);
        let said = watching.said();
        let (asking, failed) = (watching.asking(), watching.chat_failed());
        let count = said.len() + usize::from(asking) + usize::from(failed.is_some());
        if self.heard.replace(count) != count {
            self.chat_scroll.scroll_to_bottom();
        }
        let empty = self.query.read(cx).text().trim().is_empty();
        let retried = self.watching.clone();

        let messages = div()
            .id("video-ai-messages")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_3()
            .overflow_y_scroll()
            .track_scroll(&self.chat_scroll)
            .when(said.is_empty() && !asking && failed.is_none(), |this| {
                this.child(self.intro())
            })
            .children(
                said.iter()
                    .enumerate()
                    .map(|(place, said)| match said.speaker {
                        Speaker::User => self.question(place, &said.text, cx),
                        Speaker::Assistant => self.reply(place, &said.text, cx),
                    }),
            )
            .when(asking, |this| {
                this.child(
                    div()
                        .id("video-ai-thinking")
                        .role(Role::Status)
                        .aria_label(t!("video-ai-thinking"))
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_size(theme.text(Text::Label))
                        .text_color(theme.muted_foreground)
                        .child(Spinner::new("video-ai-spinner"))
                        .child(t!("video-ai-thinking")),
                )
            })
            .when_some(failed, |this, reason| {
                let reason = match watching.chat_blind() {
                    true => t!("video-ai-no-transcript"),
                    false => SharedString::from(reason.to_owned()),
                };
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_2()
                        .child(
                            Notice::new(t!("video-ai-failed"), reason)
                                .failed()
                                .spoken("video-ai-failed"),
                        )
                        .child(
                            Button::new("video-ai-retry")
                                .label(t!("video-ai-retry"))
                                .secondary()
                                .on_click(move |_, _, cx| {
                                    retried.update(cx, |watching, cx| watching.ask_again(cx));
                                }),
                        ),
                )
            });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .when(refused, |this| {
                this.child(
                    div()
                        .id("video-ai-refused")
                        .role(Role::Alert)
                        .aria_label(t!("video-ai-refused"))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex_1()
                                .text_size(theme.text(Text::Small))
                                .text_color(theme.danger)
                                .child(t!("video-ai-refused")),
                        )
                        .child(
                            Button::new("video-ai-refused-open")
                                .label(t!("video-ai-setup-open"))
                                .small()
                                .ghost()
                                .on_click(|_, _, cx| {
                                    navigate(Destination::Settings(SettingsPage::Playback), cx)
                                }),
                        ),
                )
            })
            .child(messages)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .on_action(cx.listener(Self::send))
                    .child(div().flex_1().min_w_0().child(self.query.clone()))
                    .child(
                        Button::new("video-ai-send")
                            .label(t!("video-ai-send"))
                            .secondary()
                            .disabled(empty || asking)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.send(&Submit, window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// What the chat shows before the first question: what it can do and a few to start with.
    fn intro(&self) -> AnyElement {
        Vacancy::new(t!("video-ai-intro-title"))
            .detail(t!("video-ai-intro"))
            .spoken("video-ai-intro")
            .action(div().flex().flex_col().items_center().gap_2().children(
                SUGGESTIONS.iter().map(|&key| {
                    let watching = self.watching.clone();
                    Button::new(key)
                        .label(i18n::lookup(key, None))
                        .secondary()
                        .on_click(move |_, _, cx| {
                            let asked = i18n::lookup(key, None);
                            watching.update(cx, |watching, cx| watching.ask(&asked, cx));
                        })
                }),
            ))
            .into_any_element()
    }

    /// What the chat shows without a key: what it is for and the way to settings.
    fn setup(&self) -> AnyElement {
        div()
            .id("video-ai-setup-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .p_3()
            .overflow_y_scroll()
            .child(
                Vacancy::new(t!("video-ai-setup-title"))
                    .detail(t!("video-ai-setup"))
                    .spoken("video-ai-setup")
                    .action(
                        Button::new("video-ai-setup-open")
                            .label(t!("video-ai-setup-open"))
                            .secondary()
                            .on_click(|_, _, cx| {
                                navigate(Destination::Settings(SettingsPage::Playback), cx)
                            }),
                    ),
            )
            .into_any_element()
    }

    /// A question, in a quiet bubble at the trailing edge, the way a conversation is read.
    fn question(&self, place: usize, text: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let text = SharedString::from(text.to_owned());
        div()
            .id(("video-ai-question", place))
            .role(Role::Group)
            .aria_label(t!("video-ai-you-said", text = text.as_ref()))
            .flex()
            .justify_end()
            .w_full()
            .pl_8()
            .child(
                div()
                    .min_w_0()
                    .px_3()
                    .py_1p5()
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .text_size(theme.text(Text::Label))
                    .child(text),
            )
            .into_any_element()
    }

    /// An answer as plain text across the panel, its Markdown drawn block by block and its moments
    /// and addresses seeking or opening when pressed.
    fn reply(&self, place: usize, text: &str, cx: &Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let shown = SharedString::from(unbracket(text));
        let mut pen = Pen {
            theme,
            watching: self.watching.clone(),
            place,
            drawn: 0,
        };
        let body = pen.blocks(&markdown::blocks(&shown));
        let copied = shown.clone();

        div()
            .id(("video-ai-reply", place))
            .role(Role::Group)
            .aria_label(shown)
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .child(body)
            .child(
                div().flex().child(
                    Button::new(("video-ai-copy", place))
                        .icon("icons/copy.svg")
                        .small()
                        .ghost()
                        .tooltip("video-ai-copy")
                        .on_click(move |_, _, cx| copy(copied.to_string(), cx)),
                ),
            )
            .into_any_element()
    }

    /// The video's chapters, each seeking to its start, the one playing lit with how far into
    /// it the video is.
    pub(super) fn chapter_list(&self, now: f64, cx: &Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let watching = self.watching.read(cx);
        let chapters = watching.chapters();
        if chapters.is_empty() {
            return div()
                .id("video-chapters-making")
                .role(Role::Status)
                .aria_label(t!("video-chapters-making"))
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .py_8()
                .child(Spinner::new("video-chapters-spinner"))
                .child(
                    div()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(t!("video-chapters-making")),
                )
                .into_any_element();
        }
        let total = watching
            .player()
            .and_then(|player| player.duration())
            .unwrap_or(0.);
        let playing = watching.chapter_at(now);
        let count = chapters.len();

        div()
            .id("video-chapters")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .p_1()
            .gap_0p5()
            .overflow_y_scroll()
            .when(watching.chapters_ai(), |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_2()
                        .pt_1p5()
                        .pb_1()
                        .text_size(theme.text(Text::Tiny))
                        .text_color(theme.muted_foreground)
                        .child(t!("video-chapters-ai")),
                )
            })
            .children(chapters.iter().enumerate().map(|(place, chapter)| {
                let end = chapters
                    .get(place + 1)
                    .map(|next| next.start)
                    .unwrap_or_else(|| total.max(chapter.start));
                let length = (end - chapter.start).max(0.);
                let lit = playing == Some(place);
                let through = match lit && length > 0. {
                    true => ((now - chapter.start) / length).clamp(0., 1.) as f32,
                    false => 0.,
                };
                let (sought, start) = (self.watching.clone(), chapter.start);
                let title = SharedString::from(chapter.title.clone());
                let minutes = (length / 60.).round() as u64;
                let meta = match length > 0. {
                    true => t!(
                        "video-chapter-meta",
                        start = clock(Duration::from_secs_f64(start)).to_string(),
                        minutes = minutes
                    ),
                    false => clock(Duration::from_secs_f64(start)),
                };
                div()
                    .id(("video-chapter", (start * 1000.) as usize))
                    .role(Role::Button)
                    .aria_label(title.clone())
                    .aria_position_in_set(place + 1)
                    .aria_size_of_set(count)
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_2()
                    .py_2()
                    .rounded(theme.radius)
                    .corner_smoothing(theme.smoothing)
                    .cursor_pointer()
                    .when(lit, |this| this.bg(theme.muted))
                    .hover(|this| this.bg(theme.secondary_hover))
                    .on_click(move |_, _, cx| {
                        sought.update(cx, |watching, cx| watching.seek_to(start, cx));
                    })
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(BADGE)
                            .rounded(theme.radius)
                            .corner_smoothing(theme.smoothing)
                            .text_size(theme.text(Text::Tiny))
                            .font_weight(FontWeight::SEMIBOLD)
                            .map(|this| match lit {
                                true => this.bg(theme.primary).text_color(theme.primary_foreground),
                                false => this.bg(theme.muted).text_color(theme.muted_foreground),
                            })
                            .child(SharedString::from((place + 1).to_string())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(theme.text(Text::Small))
                                    .font_weight(match lit {
                                        true => FontWeight::SEMIBOLD,
                                        false => FontWeight::MEDIUM,
                                    })
                                    .text_color(theme.foreground)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(theme.text(Text::Tiny))
                                    .text_color(theme.muted_foreground)
                                    .child(meta),
                            )
                            .when(lit, |this| {
                                this.child(
                                    div()
                                        .mt_1()
                                        .h(PROGRESS)
                                        .w_full()
                                        .rounded_full()
                                        .bg(theme.muted)
                                        .child(
                                            div()
                                                .h_full()
                                                .w(relative(through))
                                                .rounded_full()
                                                .bg(theme.progress_bar),
                                        ),
                                )
                            }),
                    )
            }))
            .into_any_element()
    }

    /// The chapter playing, beside the time, which opens the chapters when pressed.
    pub(super) fn chapter_now(&self, position: f64, cx: &Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let watching = self.watching.read(cx);
        let chapters = watching.chapters();
        let chapter = chapters.get(watching.chapter_at(position)?)?;
        let title = SharedString::from(chapter.title.clone());
        Some(
            div()
                .id("video-chapter-now")
                .role(Role::Button)
                .aria_label(t!("video-chapter-now", title = title.as_ref()))
                .flex()
                .items_center()
                .gap_1p5()
                .min_w_0()
                .max_w(CHAPTER_NOW)
                .px_1p5()
                .py_0p5()
                .rounded(theme.radius)
                .corner_smoothing(theme.smoothing)
                .cursor_pointer()
                .text_size(theme.text(Text::Small))
                .text_color(theme.muted_foreground)
                .hover(|this| this.bg(theme.secondary_hover).text_color(theme.foreground))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.panel = Panel::Chapters;
                    this.watching.update(cx, |watching, cx| {
                        if !watching.transcript_open() {
                            watching.toggle_transcript(cx);
                        }
                    });
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_none()
                        .size(DOT)
                        .rounded_full()
                        .bg(theme.muted_foreground),
                )
                .child(div().min_w_0().truncate().child(title))
                .child(
                    svg()
                        .flex_none()
                        .path(icons::path("icons/chevron-right.svg"))
                        .size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground),
                )
                .into_any_element(),
        )
    }
}

/// The answer with the brackets taken off its moments, so `[4:05]` reads and seeks as `4:05`
/// and `[1:00-2:30]` as two moments.
fn unbracket(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let inside = after.find(']').map(|close| &after[..close]);
        match inside {
            Some(inside)
                if inside.contains(':')
                    && inside.chars().all(|letter| {
                        letter.is_ascii_digit() || matches!(letter, ':' | '-' | '–' | ' ')
                    }) =>
            {
                out.push_str(&inside.replace(['-', '–'], " – ").replace("  ", " "));
                rest = &after[inside.len() + 1..];
            }
            _ => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Puts `text` on the clipboard and says so.
fn copy(text: String, cx: &mut gpui::App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    Toasts::show(Outcome::Done, "toast-ai-copied", cx);
}

/// What a screen reader says for a row of a table: its cells, one after the other.
fn row_label(row: &[Rich]) -> SharedString {
    row.iter()
        .map(|cell| cell.text.as_str())
        .collect::<Vec<_>>()
        .join(", ")
        .into()
}
