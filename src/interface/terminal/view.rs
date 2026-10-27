//! src/interface/terminal/view.rs renders bounded ANSI frames, selectable lists and tables.
//! Lists draw dim section headings and icons; tables add a header and columns that drop from
//! the right on narrow screens. All external text is escaped before rendering.

use super::{
    brand,
    text::{cells, clipped, tail, wrapped},
    tty::{Key, Terminal},
};
use anyhow::Result;
use std::collections::BTreeMap;
use std::fmt::Write;

const ACCENT: &str = "\x1b[48;5;216m\x1b[30m";
const DIM: &str = "\x1b[90m";
const RESET: &str = "\x1b[0m";
const PROMPT_BYTES: usize = 80;
const REMEMBERED_FIELDS: usize = 32;

/// Match every wrapped field to the actual frame, including on very wide terminals.
fn content_width(terminal_width: usize) -> usize {
    terminal_width.saturating_sub(2).min(110)
}

pub(super) struct Choice {
    pub label: String,
    pub description: String,
    /// Additional action guidance shown with Tab and included in search.
    pub detail: String,
    /// Guidance for the selected action; excluded from search to avoid cross-action matches.
    pub next_step: String,
    /// Single-cell symbol drawn before the label; empty for plain lists.
    pub icon: &'static str,
    /// Heading drawn once above the first consecutive choice of a section.
    pub section: Option<&'static str>,
    /// Table cells, one per column; empty for an action rendered as label and description.
    pub cells: Vec<String>,
}

impl Choice {
    pub(super) fn new(label: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: description.into(),
            detail: String::new(),
            next_step: String::new(),
            icon: "",
            section: None,
            cells: Vec::new(),
        }
    }

    /// A table record; the first cell doubles as its label for search and details.
    pub(super) fn row(cells: Vec<String>, description: impl Into<String>) -> Self {
        Self {
            label: cells.first().cloned().unwrap_or_default(),
            cells,
            ..Self::new("", description)
        }
    }

    fn searchable_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.label,
            self.description,
            self.detail,
            self.cells.join(" ")
        )
        .to_lowercase()
    }
}

/// How a table column takes width: fit its content up to a cap, or absorb what is left.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Width {
    Fit(usize),
    Fill,
}

pub(super) struct Column {
    pub title: &'static str,
    pub width: Width,
}

impl Column {
    pub(super) const fn new(title: &'static str, width: Width) -> Self {
        Self { title, width }
    }
}

/// Narrow screens keep at least this much of the fill column before dropping later ones.
const MIN_FILL: usize = 8;
const GAP: usize = 2;

/// Column widths that fit `available` cells: columns are kept left to right while they fit,
/// and the fill column receives the remainder.
fn layout(columns: &[Column], rows: &[&Choice], available: usize) -> Vec<usize> {
    let mut widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let content = rows
                .iter()
                .filter_map(|row| row.cells.get(index))
                .map(|cell| cells(cell))
                .max()
                .unwrap_or(0);
            match column.width {
                Width::Fit(cap) => content.max(cells(column.title)).min(cap),
                Width::Fill => cells(column.title).max(MIN_FILL),
            }
        })
        .collect();
    let mut used = 0;
    let mut shown = 0;
    for width in &widths {
        let need = if shown == 0 { *width } else { *width + GAP };
        if used + need > available {
            break;
        }
        used += need;
        shown += 1;
    }
    if shown == 0 {
        return vec![available.max(1)];
    }
    widths.truncate(shown);
    if let Some(fill) = columns
        .iter()
        .take(shown)
        .position(|column| column.width == Width::Fill)
    {
        widths[fill] += available - used;
    }
    widths
}

fn padded(text: &str, width: usize) -> String {
    let mut cell = clipped(text, width);
    cell.push_str(&" ".repeat(width.saturating_sub(cells(&cell))));
    cell
}

fn line(parts: impl Iterator<Item = String>) -> String {
    parts.collect::<Vec<_>>().join(&" ".repeat(GAP))
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Style {
    Plain,
    Selected,
    Heading,
}

enum Row<'a> {
    Heading(&'static str),
    /// Column titles, drawn once above the first table record.
    Header,
    Item(usize, &'a Choice),
}

#[derive(Default)]
pub(super) struct Selection {
    pub index: usize,
    query: String,
    searching: bool,
}

pub(super) struct View {
    pub terminal: Terminal,
    pub compact: bool,
    pub context: String,
    previous: String,
    inputs: Option<BTreeMap<(String, String), String>>,
}

impl View {
    pub(super) fn new(terminal: Terminal, compact: bool) -> Self {
        Self {
            terminal,
            compact,
            context: String::new(),
            previous: String::new(),
            inputs: None,
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.previous.clear();
    }

    /// Keep ordinary non-secret field values only within one main-menu action.
    pub(super) fn remember_inputs(&mut self, enabled: bool) {
        self.inputs = enabled.then(BTreeMap::new);
    }

    /// A writer for the footer row that works while a blocking operation owns the screen.
    pub(super) fn footer_writer(
        &mut self,
    ) -> Result<impl Fn(&str) -> std::io::Result<()> + Send + Sync + 'static> {
        let (width, height) = self.terminal.size()?;
        // The next frame must redraw the row this writer overwrites.
        self.invalidate();
        Ok(move |text: &str| {
            use std::io::Write as _;
            let mut stdout = std::io::stdout().lock();
            write!(
                stdout,
                "\x1b[{height};1H\x1b[2K {DIM}{}{RESET}",
                clipped(text, width.saturating_sub(2))
            )?;
            stdout.flush()
        })
    }

    fn content_rows(&self, width: usize, height: usize) -> usize {
        let chrome = if height <= 12 { 3 } else { 4 };
        height
            .saturating_sub(brand::rows(brand::tall(width, height, self.compact)) + chrome)
            .max(1)
    }

    pub(super) fn frame(
        &mut self,
        title: &str,
        lines: &[(String, Style)],
        footer: &str,
    ) -> Result<()> {
        let (width, height) = self.terminal.size()?;
        let width = content_width(width);
        let mut frame = String::from("\x1b[H\x1b[2J");
        if width < 30 || height < 10 {
            writeln!(frame, "{}\r", clipped("Resize: need 32x10", width))?;
            write!(frame, "{}", clipped("Ctrl-C to exit", width))?;
        } else {
            let tall = brand::tall(width + 2, height, self.compact);
            // Titles that would wrap beside the mascot move to the divider instead.
            let room = width.saturating_sub(if tall { 28 } else { 17 });
            let long_title = cells(title) > room;
            brand::draw(&mut frame, tall, if long_title { "" } else { title })?;
            writeln!(frame, " {DIM}{}{RESET}\r", clipped(&self.context, width))?;
            let divider = if long_title {
                format!("-- {title}")
            } else {
                "-".repeat(width)
            };
            writeln!(frame, " {DIM}{}{RESET}\r", clipped(&divider, width))?;
            for (text, style) in lines.iter().take(self.content_rows(width + 2, height)) {
                let mut text = clipped(text, width);
                if *style == Style::Selected {
                    let used = cells(&text);
                    text.push_str(&" ".repeat(width.saturating_sub(used)));
                }
                let color = match style {
                    Style::Plain => "",
                    Style::Selected => ACCENT,
                    Style::Heading => DIM,
                };
                writeln!(frame, " {color}{text}{RESET}\r")?;
            }
            write!(
                frame,
                "\x1b[{};1H {DIM}{}{RESET}",
                height,
                clipped(footer, width)
            )?;
        }
        if frame != self.previous {
            self.terminal.write(&frame)?;
            self.previous = frame;
        }
        Ok(())
    }

    pub(super) fn select(
        &mut self,
        title: &str,
        choices: &[Choice],
        state: &mut Selection,
        searchable: bool,
    ) -> Result<Option<usize>> {
        self.list(title, &[], choices, state, searchable)
    }

    /// A list whose record choices carry cells drawn under `columns`; actions render as usual.
    pub(super) fn table(
        &mut self,
        title: &str,
        columns: &[Column],
        choices: &[Choice],
        state: &mut Selection,
        searchable: bool,
    ) -> Result<Option<usize>> {
        self.list(title, columns, choices, state, searchable)
    }

    fn list(
        &mut self,
        title: &str,
        columns: &[Column],
        choices: &[Choice],
        state: &mut Selection,
        searchable: bool,
    ) -> Result<Option<usize>> {
        loop {
            let query = state.query.to_lowercase();
            let filtered: Vec<_> = choices
                .iter()
                .enumerate()
                .filter(|(_, c)| c.searchable_text().contains(&query))
                .collect();
            state.index = state.index.min(filtered.len().saturating_sub(1));
            let (width, height) = self.terminal.size()?;
            let rows = self.content_rows(width, height).saturating_sub(3).max(1);
            let records: Vec<_> = filtered
                .iter()
                .map(|(_, choice)| *choice)
                .filter(|choice| !choice.cells.is_empty())
                .collect();
            // Frame margins and the selection prefix leave this much room for table cells.
            let widths = layout(columns, &records, content_width(width).saturating_sub(2));
            // Headings structure the full list only; a search shows flat matches.
            let mut table = Vec::new();
            let mut section = None;
            let mut header = columns.is_empty();
            for (position, (_, choice)) in filtered.iter().enumerate() {
                if let Some(name) = choice.section
                    && state.query.is_empty()
                    && choice.section != section
                {
                    section = choice.section;
                    table.push(Row::Heading(name));
                }
                if !header && !choice.cells.is_empty() {
                    header = true;
                    table.push(Row::Header);
                }
                table.push(Row::Item(position, choice));
            }
            // Scroll in drawn rows so the selected item stays visible below its heading.
            let current = table
                .iter()
                .position(|row| matches!(row, Row::Item(position, _) if *position == state.index))
                .unwrap_or(0);
            let start = current.saturating_sub(rows - 1);
            let mut lines = vec![(
                if searchable {
                    format!(
                        "/ {}{}",
                        tail(&state.query, content_width(width).saturating_sub(4)),
                        if state.searching {
                            "_"
                        } else {
                            "Search actions"
                        }
                    )
                } else {
                    "Choose an option".into()
                },
                Style::Plain,
            )];
            for row in table.iter().skip(start).take(rows) {
                let (position, choice) = match row {
                    Row::Heading(name) => {
                        lines.push(((*name).into(), Style::Heading));
                        continue;
                    }
                    Row::Header => {
                        let titles = columns
                            .iter()
                            .zip(&widths)
                            .map(|(column, width)| padded(column.title, *width));
                        lines.push((format!("  {}", line(titles)), Style::Heading));
                        continue;
                    }
                    Row::Item(position, choice) => (*position, *choice),
                };
                let prefix = if position == state.index { ">" } else { " " };
                let icon = if choice.icon.is_empty() {
                    String::new()
                } else {
                    format!("{} ", choice.icon)
                };
                let label = if !choice.cells.is_empty() {
                    let cells = choice
                        .cells
                        .iter()
                        .zip(&widths)
                        .map(|(cell, width)| padded(cell, *width));
                    format!("{prefix} {}", line(cells))
                } else if width >= 76 && !self.compact {
                    format!("{prefix} {icon}{:<25} {}", choice.label, choice.description)
                } else {
                    format!("{prefix} {icon}{}", choice.label)
                };
                let style = if position == state.index {
                    Style::Selected
                } else {
                    Style::Plain
                };
                lines.push((label, style));
            }
            let description = filtered
                .get(state.index)
                .map_or("No matching actions", |(_, c)| {
                    if width >= 76 && !self.compact && !c.next_step.is_empty() {
                        c.next_step.as_str()
                    } else {
                        c.description.as_str()
                    }
                });
            lines.push((description.into(), Style::Plain));
            let position = if filtered.is_empty() {
                0
            } else {
                state.index + 1
            };
            lines.push((
                format!(
                    "{position}/{}  Up/Down{}",
                    filtered.len(),
                    if searchable { "  / search" } else { "" }
                ),
                Style::Plain,
            ));
            let footer = if state.searching {
                "Enter open Tab info Esc clear"
            } else if searchable {
                "Enter open Tab info Esc back"
            } else {
                "Enter pick Tab info Esc cancel"
            };
            self.frame(title, &lines, footer)?;
            let key = if state.searching {
                self.terminal.text_key()?
            } else {
                self.terminal.key()?
            };
            if width < 32 || height < 10 {
                continue;
            }
            match key {
                Key::Up => state.index = state.index.saturating_sub(1),
                Key::Down => state.index = (state.index + 1).min(filtered.len().saturating_sub(1)),
                Key::PageUp => state.index = state.index.saturating_sub(rows),
                Key::PageDown => {
                    state.index = (state.index + rows).min(filtered.len().saturating_sub(1))
                }
                Key::Details if !filtered.is_empty() => {
                    let choice = filtered[state.index].1;
                    let mut text = if choice.cells.is_empty() {
                        choice.label.clone()
                    } else {
                        columns
                            .iter()
                            .zip(&choice.cells)
                            .map(|(column, cell)| format!("{}: {cell}", column.title))
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    text.push('\n');
                    text.push_str(&choice.description);
                    if !choice.next_step.is_empty() {
                        text.push_str("\n\n");
                        text.push_str(&choice.next_step);
                    }
                    if !choice.detail.is_empty() {
                        text.push_str("\n\n");
                        text.push_str(&choice.detail);
                    }
                    self.result("Details", &text)?;
                }
                Key::Character('k') if !state.searching => {
                    state.index = state.index.saturating_sub(1)
                }
                Key::Character('j') if !state.searching => {
                    state.index = (state.index + 1).min(filtered.len().saturating_sub(1))
                }
                Key::Enter if !filtered.is_empty() => return Ok(Some(filtered[state.index].0)),
                Key::Escape if state.searching || !state.query.is_empty() => {
                    state.searching = false;
                    state.query.clear();
                    state.index = 0;
                }
                Key::Escape => return Ok(None),
                Key::Character('/') if searchable && !state.searching => {
                    state.searching = true;
                    state.query.clear();
                    state.index = 0;
                }
                Key::Character(c) if state.searching && state.query.len() < 80 => {
                    state.query.push(c);
                    state.index = 0;
                }
                Key::Backspace if state.searching => {
                    state.query.pop();
                    state.index = 0;
                }
                _ => {}
            }
        }
    }

    pub(super) fn prompt(
        &mut self,
        title: &str,
        label: &str,
        show_length: bool,
    ) -> Result<Option<String>> {
        self.input(title, label, PROMPT_BYTES, false, true, show_length)
    }

    pub(super) fn setup_input(
        &mut self,
        title: &str,
        label: &str,
        hidden: bool,
    ) -> Result<Option<String>> {
        self.input(title, label, 4096, hidden, true, false)
    }

    fn input(
        &mut self,
        title: &str,
        label: &str,
        limit: usize,
        hidden: bool,
        paste: bool,
        show_length: bool,
    ) -> Result<Option<String>> {
        let key = (title.to_owned(), label.to_owned());
        let remember = !hidden && limit == PROMPT_BYTES;
        let mut value = if remember {
            self.inputs
                .as_ref()
                .and_then(|inputs| inputs.get(&key))
                .cloned()
                .unwrap_or_default()
        } else {
            String::new()
        };
        loop {
            if remember
                && let Some(inputs) = &mut self.inputs
                && (inputs.contains_key(&key) || inputs.len() < REMEMBERED_FIELDS)
            {
                inputs.insert(key.clone(), value.clone());
            }
            let (width, height) = self.terminal.size()?;
            // Keep the editing cursor visible even on the smallest viewport.
            let visible = if hidden {
                "[hidden]".into()
            } else {
                tail(&value, content_width(width).saturating_sub(3))
            };
            let mut lines: Vec<_> = wrapped(label, content_width(width).max(1))
                .into_iter()
                .map(|line| (line, Style::Plain))
                .collect();
            // Reserve the editing row and optional count even when instructions wrap.
            let label_rows = self
                .content_rows(width, height)
                .saturating_sub(1 + usize::from(hidden || show_length))
                .max(1);
            if lines.len() > label_rows {
                lines.truncate(label_rows);
                lines[label_rows - 1].0 = "Tab: full instructions".into();
            }
            lines.push((format!("> {visible}_"), Style::Selected));
            // Hidden fields report only a count, so a paste is visible without its value.
            if hidden {
                lines.push((
                    if value.is_empty() {
                        "Nothing received yet; paste, then press Enter".into()
                    } else {
                        format!("{} characters received", value.chars().count())
                    },
                    Style::Plain,
                ));
            } else if show_length {
                lines.push((format!("{}/{limit} bytes", value.len()), Style::Plain));
            }
            self.frame(title, &lines, "Enter continue  Esc cancel")?;
            let key = if paste {
                self.terminal.text_key()?
            } else {
                self.terminal.key()?
            };
            if width < 32 || height < 10 {
                continue;
            }
            match key {
                Key::Enter if hidden && value.is_empty() => {}
                Key::Enter => return Ok(Some(value)),
                Key::Escape => return Ok(None),
                Key::Details => {
                    self.result(title, label)?;
                }
                Key::Backspace => {
                    value.pop();
                }
                // Keystrokes beyond the limit are ignored; the field keeps what was typed.
                Key::Character(c) if value.len() + c.len_utf8() <= limit => value.push(c),
                _ => {}
            }
        }
    }

    pub(super) fn result(&mut self, title: &str, text: &str) -> Result<bool> {
        self.read(title, text, "Enter/Esc back  ^C exit")
    }

    pub(super) fn review(&mut self, title: &str, text: &str) -> Result<bool> {
        self.read(title, text, "Enter next  Esc cancel")
    }

    fn read(&mut self, title: &str, text: &str, footer: &str) -> Result<bool> {
        let mut offset = 0usize;
        loop {
            let (width, height) = self.terminal.size()?;
            let lines = wrapped(text, content_width(width).saturating_sub(1).max(1));
            let rows = self.content_rows(width, height).saturating_sub(1).max(1);
            let total = lines.len();
            offset = offset.min(lines.len().saturating_sub(rows));
            let mut visible: Vec<_> = lines
                .into_iter()
                .skip(offset)
                .take(rows)
                .map(|s| (s, Style::Plain))
                .collect();
            visible.push((
                format!(
                    "{}-{}/{}  Up/Dn PgUp/PgDn",
                    (offset + 1).min(total),
                    (offset + rows).min(total),
                    total
                ),
                Style::Plain,
            ));
            self.frame(title, &visible, footer)?;
            let key = self.terminal.key()?;
            if width < 32 || height < 10 {
                continue;
            }
            match key {
                Key::Enter => return Ok(true),
                Key::Escape => return Ok(false),
                Key::Up | Key::Character('k') => offset = offset.saturating_sub(1),
                Key::Down | Key::Character('j') => offset += 1,
                Key::PageUp => offset = offset.saturating_sub(rows),
                Key::PageDown => offset += rows,
                _ => {}
            }
        }
    }
}
