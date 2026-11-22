//! Reads the Markdown an AI answer is written in into blocks the chat can draw: paragraphs,
//! headings, lists, quotes, code, tables and equations.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// How a stretch of text inside a line is drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Look {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub struck: bool,
    /// The address the stretch opens when pressed.
    pub link: Option<String>,
}

/// A run of text with the looks of its stretches.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Rich {
    pub text: String,
    pub looks: Vec<(Range<usize>, Look)>,
}

/// One block of an answer.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Block {
    Paragraph(Rich),
    Heading(u8, Rich),
    /// A list, numbered from its first number when it has one, each item blocks of its own.
    List(Option<u64>, Vec<Vec<Block>>),
    Quote(Vec<Block>),
    Code(String),
    /// A table: its head and its rows, cell by cell.
    Table(Vec<Rich>, Vec<Vec<Rich>>),
    /// An equation on a line of its own.
    Math(String),
    Rule,
}

/// What is being read, with everything still open.
#[derive(Default)]
struct Reading {
    /// The blocks of the answer under those of every open quote and list item.
    open: Vec<Vec<Block>>,
    lists: Vec<(Option<u64>, Vec<Vec<Block>>)>,
    line: Option<Rich>,
    look: Look,
    code: Option<String>,
    head: Vec<Rich>,
    rows: Vec<Vec<Rich>>,
    row: Vec<Rich>,
}

impl Reading {
    /// Adds `text` to the line being read, opening one when none is.
    fn write(&mut self, text: &str, look: Look) {
        let line = self.line.get_or_insert_default();
        let from = line.text.len();
        line.text.push_str(text);
        if look != Look::default() {
            line.looks.push((from..line.text.len(), look));
        }
    }

    fn push(&mut self, block: Block) {
        if let Some(blocks) = self.open.last_mut() {
            blocks.push(block);
        }
    }

    /// Ends a line that no paragraph wraps, as the lines of a tight list are.
    fn settle(&mut self) {
        if let Some(line) = self.line.take().filter(|line| !line.text.trim().is_empty()) {
            self.push(Block::Paragraph(line));
        }
    }
}

/// The blocks of `text`, read as Markdown with tables, strikethrough and LaTeX math.
pub(super) fn blocks(text: &str) -> Vec<Block> {
    let text = dollars(text);
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_MATH;
    let mut reading = Reading {
        open: vec![Vec::new()],
        ..Default::default()
    };
    for event in Parser::new_ext(&text, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell => {
                    reading.settle();
                    reading.line = Some(Rich::default());
                }
                Tag::List(first) => {
                    reading.settle();
                    reading.lists.push((first, Vec::new()));
                }
                Tag::Item | Tag::BlockQuote(_) => {
                    reading.settle();
                    reading.open.push(Vec::new());
                }
                Tag::CodeBlock(_) => {
                    reading.settle();
                    reading.code = Some(String::new());
                }
                Tag::Table(_) => reading.settle(),
                Tag::TableHead | Tag::TableRow => reading.row = Vec::new(),
                Tag::Emphasis => reading.look.italic = true,
                Tag::Strong => reading.look.bold = true,
                Tag::Strikethrough => reading.look.struck = true,
                Tag::Link { dest_url, .. } => reading.look.link = Some(dest_url.to_string()),
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Paragraph => reading.settle(),
                TagEnd::Heading(level) => {
                    if let Some(line) = reading.line.take() {
                        reading.push(Block::Heading(level as u8, line));
                    }
                }
                TagEnd::Item => {
                    reading.settle();
                    let item = reading.open.pop().unwrap_or_default();
                    if let Some((_, items)) = reading.lists.last_mut() {
                        items.push(item);
                    }
                }
                TagEnd::List(_) => {
                    if let Some((first, items)) = reading.lists.pop() {
                        reading.push(Block::List(first, items));
                    }
                }
                TagEnd::BlockQuote(_) => {
                    reading.settle();
                    let quoted = reading.open.pop().unwrap_or_default();
                    reading.push(Block::Quote(quoted));
                }
                TagEnd::CodeBlock => {
                    if let Some(code) = reading.code.take() {
                        reading.push(Block::Code(code.trim_end().to_owned()));
                    }
                }
                TagEnd::TableCell => {
                    let cell = reading.line.take().unwrap_or_default();
                    reading.row.push(cell);
                }
                TagEnd::TableHead => reading.head = std::mem::take(&mut reading.row),
                TagEnd::TableRow => {
                    let row = std::mem::take(&mut reading.row);
                    reading.rows.push(row);
                }
                TagEnd::Table => {
                    let (head, rows) = (
                        std::mem::take(&mut reading.head),
                        std::mem::take(&mut reading.rows),
                    );
                    reading.push(Block::Table(head, rows));
                }
                TagEnd::Emphasis => reading.look.italic = false,
                TagEnd::Strong => reading.look.bold = false,
                TagEnd::Strikethrough => reading.look.struck = false,
                TagEnd::Link => reading.look.link = None,
                _ => {}
            },
            Event::Text(text) => match &mut reading.code {
                Some(code) => code.push_str(&text),
                None => {
                    let look = reading.look.clone();
                    reading.write(&text, look);
                }
            },
            Event::Code(code) => {
                let look = Look {
                    code: true,
                    ..reading.look.clone()
                };
                reading.write(&code, look);
            }
            Event::InlineMath(math) => {
                let look = Look {
                    italic: true,
                    ..reading.look.clone()
                };
                reading.write(&plain(&math), look);
            }
            Event::DisplayMath(math) => {
                // An equation stands on a line of its own, splitting the paragraph around it.
                let alone = reading.line.is_none();
                reading.settle();
                reading.push(Block::Math(plain(&math)));
                if !alone {
                    reading.line = Some(Rich::default());
                }
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                let look = reading.look.clone();
                reading.write(&text, look);
            }
            Event::SoftBreak => reading.write(" ", Look::default()),
            Event::HardBreak => reading.write("\n", Look::default()),
            Event::Rule => {
                reading.settle();
                reading.push(Block::Rule);
            }
            _ => {}
        }
    }
    reading.settle();
    reading.open.swap_remove(0)
}

/// A table as tab-separated lines, which a spreadsheet takes as cells when pasted.
pub(super) fn cells(head: &[Rich], rows: &[Vec<Rich>]) -> String {
    std::iter::once(head)
        .chain(rows.iter().map(Vec::as_slice))
        .filter(|row| !row.is_empty())
        .map(|row| {
            row.iter()
                .map(|cell| cell.text.replace(['\t', '\n'], " "))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` with the `\( \)` and `\[ \]` some models fence math in turned into the dollar signs
/// Markdown reads, leaving code as it was written.
fn dollars(text: &str) -> String {
    let mut turned = String::with_capacity(text.len());
    let mut fenced = false;
    // Fenced code is kept by the line, and what lies between two fences is read as one piece
    // since a code span may run over the end of a line.
    let mut prose = String::new();
    for line in text.split_inclusive('\n') {
        let fence = line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~");
        if !fence && !fenced {
            prose.push_str(line);
            continue;
        }
        turned.push_str(&spans(&std::mem::take(&mut prose)));
        turned.push_str(line);
        if fence {
            fenced = !fenced;
        }
    }
    turned.push_str(&spans(&prose));
    turned
}

/// Prose with its math fences as dollar signs outside its code spans. A run of backticks opens
/// a span only when a run as long closes it in the same paragraph, and an escaped backtick
/// opens none.
fn spans(prose: &str) -> String {
    let mut turned = String::with_capacity(prose.len());
    let mut plain = 0;
    let mut at = 0;
    while let Some(found) = prose[at..].find('`') {
        let start = at + found;
        let run = ticks(&prose[start..]);
        let escaped = prose[..start]
            .chars()
            .rev()
            .take_while(|c| *c == '\\')
            .count()
            % 2
            == 1;
        let body = match escaped {
            true => start + 1,
            false => start + run,
        };
        let close = (!escaped).then(|| closing(&prose[body..], run)).flatten();
        match close {
            Some(length) => {
                turned.push_str(&spelled(&prose[plain..start]));
                turned.push_str(&prose[start..body + length + run]);
                at = body + length + run;
                plain = at;
            }
            None => at = body,
        }
    }
    turned.push_str(&spelled(&prose[plain..]));
    turned
}

/// How many backticks `text` starts with.
fn ticks(text: &str) -> usize {
    text.bytes().take_while(|byte| *byte == b'`').count()
}

/// How far into `text` the run of exactly `run` backticks closing a code span starts, when one
/// does before the paragraph, the item of a list or the row of a table ends.
fn closing(text: &str, run: usize) -> Option<usize> {
    let end = text
        .match_indices('\n')
        .find(|(at, _)| starts_block(&text[at + 1..]))
        .map_or(text.len(), |(at, _)| at);
    let mut at = 0;
    while let Some(found) = text[at..end].find('`') {
        let start = at + found;
        let length = ticks(&text[start..]);
        if length == run {
            return Some(start);
        }
        at = start + length;
    }
    None
}

/// Whether the line `text` begins with ends the paragraph before it: an empty line, an item of
/// a list, a heading, a quote or a row of a table.
fn starts_block(text: &str) -> bool {
    let line = text.lines().next().unwrap_or_default().trim_start();
    let numbered = line
        .trim_start_matches(|letter: char| letter.is_ascii_digit())
        .strip_prefix(['.', ')'])
        .is_some_and(|rest| rest.starts_with(' ') && rest.len() < line.len() - 1);
    line.is_empty()
        || numbered
        || ["- ", "* ", "+ ", "#", ">", "|"]
            .iter()
            .any(|mark| line.starts_with(mark))
}

/// Text outside code with its math fences as dollar signs.
fn spelled(text: &str) -> String {
    text.replace("\\[", "$$")
        .replace("\\]", "$$")
        .replace("\\(", "$")
        .replace("\\)", "$")
}

/// LaTeX math as the Unicode nearest to it: `x^2` as x², `\alpha` as α, a fraction on one
/// line.
fn plain(math: &str) -> String {
    let mut math = sized(math.trim());
    for wrap in ["\\text", "\\mathrm", "\\mathbf", "\\operatorname"] {
        math = math.replace(wrap, "");
    }
    let math = fractions(&math)
        .replace("\\cdot", "·")
        .replace("\\times", "×")
        .replace("\\,", " ")
        .replace("\\;", " ")
        .replace("\\quad", "  ")
        .replace("\\\\", "\n");
    unicodeit::replace(&math).replace(['{', '}'], "")
}

/// `math` without the `\left` and `\right` that size a bracket, keeping the commands that only
/// start with them, as `\leftarrow` does.
fn sized(math: &str) -> String {
    let mut out = String::with_capacity(math.len());
    let mut rest = math;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let name = after
            .find(|letter: char| !letter.is_ascii_alphabetic())
            .map_or(after, |end| &after[..end]);
        if name != "left" && name != "right" {
            out.push('\\');
            out.push_str(name);
        }
        rest = &after[name.len()..];
    }
    out.push_str(rest);
    out
}

/// `math` with every `\frac{a}{b}` written on one line as `(a)/(b)`.
fn fractions(math: &str) -> String {
    let mut out = String::with_capacity(math.len());
    let mut rest = math;
    while let Some(at) = rest.find("\\frac{") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "\\frac".len()..];
        let Some((over, tail)) = braced(after) else {
            out.push_str("\\frac");
            rest = after;
            continue;
        };
        let Some((under, tail)) = braced(tail) else {
            out.push_str("\\frac");
            rest = after;
            continue;
        };
        out.push_str(&format!("{}/{}", part(over), part(under)));
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// One side of a fraction on one line, in brackets when it is more than a single term.
fn part(side: &str) -> String {
    let side = fractions(side.trim());
    match side.contains([' ', '+', '-', '/']) {
        true => format!("({side})"),
        false => side,
    }
}

/// What the braces `text` opens with hold, and what follows them.
fn braced(text: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (at, letter) in text.char_indices() {
        match letter {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((&text[1..at], &text[at + 1..]));
                }
            }
            _ if depth == 0 => return None,
            _ => {}
        }
    }
    None
}
