// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
use crate::{
    bar,
    input::{Decoder, Editor, Input, Key, Prefix, kitty_key},
    modern::Event,
    picker,
};
use ratatui::{
    Frame,
    buffer::{Buffer, CellDiffOption},
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Paragraph, Widget},
};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;
use utty::{
    core::{Client, DirectoryRow, Frame as CoreFrame, Pane, Snapshot},
    options::{Options, Profile},
};

pub const BASE: u32 = 0x18212f;
pub const TEAL: u32 = 0x62d6c5;
pub const INK: u32 = 0x102b31;
pub use utty::text::clean;
pub fn rgb(value: u32) -> Color {
    if value == u32::MAX {
        Color::Reset
    } else {
        Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }
}
pub fn band(foreground: u32, background: u32, modern: bool) -> Style {
    if modern {
        Style::default().fg(rgb(foreground)).bg(rgb(background))
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
    }
}
/// The color a program status is drawn in: working, done, needs attention,
/// failed.
pub fn status_color(kind: u8) -> Option<u32> {
    match kind {
        3 => Some(0x6cb6ff),
        4 => Some(0x7ee2a8),
        5 | 6 => Some(0xffcf70),
        7 => Some(0xff7777),
        _ => None,
    }
}
pub fn icon(kind: u8, modern: bool) -> &'static str {
    match (kind, modern) {
        (3, true) => "◌",
        (4, true) => "✓",
        (5 | 6, true) => "⚠",
        (7, true) => "✕",
        (3, false) => "working",
        (4, false) => "done",
        (5, false) => "attention",
        (6, false) => "blocked",
        (7, false) => "failed",
        _ => "",
    }
}
#[derive(Debug, Default, PartialEq, Eq)]
enum View {
    #[default]
    Terminal,
    Picker,
    Help,
}
struct Prompt {
    key: u8,
    confirm: bool,
    session: u32,
    target: Snapshot,
    editor: Editor,
}
#[derive(Default)]
pub struct Ui {
    view: View,
    prompt: Option<Prompt>,
    prefix: Prefix,
    decoder: Decoder,
    escape_deadline: Option<Instant>,
    selected: usize,
    help_offset: usize,
    columns: u16,
    rows: u16,
    pub dirty: bool,
    pub quit: bool,
    pub redraw: bool,
    pub notice: String,
    tab_hits: Vec<(Rect, u32)>,
    picker_offset: usize,
    /// The picker's filter.
    query: String,
    mouse_capture: Option<Pane>,
}
impl Ui {
    pub fn application(&self) -> bool {
        self.view == View::Terminal && self.prompt.is_none()
    }
    pub fn outer_event(
        &mut self,
        event: Event,
        client: &mut Client,
        snapshot: &Snapshot,
        paste_wrapped: bool,
        pixels: (u16, u16),
        pixel_mouse: bool,
    ) {
        match event {
            Event::Bytes(bytes) => self.feed(&bytes, client, snapshot),
            Event::Paste(bytes) if self.application() => {
                if paste_wrapped {
                    client.input(b"\x1b[200~");
                }
                client.input(&bytes);
                if paste_wrapped {
                    client.input(b"\x1b[201~");
                }
            }
            Event::Paste(bytes) => {
                if let Some(prompt) = self.prompt.as_mut().filter(|p| !p.confirm) {
                    let text = String::from_utf8_lossy(&bytes)
                        .chars()
                        .filter(|c| !c.is_control())
                        .collect();
                    prompt.editor.apply(&Key::Text(text));
                    self.dirty = true;
                }
            }
            Event::Focus(focused) => client.surface_focus(focused, &snapshot.panes),
            Event::Mouse {
                button,
                x,
                y,
                release,
            } => {
                let raw_x = x;
                let raw_y = y;
                let (x, y) = if pixel_mouse {
                    (x / i32::from(pixels.0), y / i32::from(pixels.1))
                } else {
                    (x, y)
                };
                let wheel = button & 64 != 0;
                let motion = button & 32 != 0;
                if !self.application() {
                    if wheel {
                        self.key(
                            if button & 1 == 0 { Key::Up } else { Key::Down },
                            client,
                            snapshot,
                        );
                    } else if !release
                        && !motion
                        && button & 3 == 0
                        && self.view == View::Picker
                        && y > 0
                        && y < i32::from(self.rows) - 1
                    {
                        self.selected = self.picker_offset + y as usize - 1;
                        self.key(Key::Enter, client, snapshot);
                    }
                    return;
                }
                if !release
                    && !motion
                    && button & 3 == 0
                    && !wheel
                    && let Some((_, id)) = self
                        .tab_hits
                        .iter()
                        .find(|(r, _)| r.contains((x as u16, y as u16).into()))
                {
                    client.select(snapshot, *id, None);
                    return;
                }
                let pane = if release || motion {
                    self.mouse_capture.as_ref()
                } else {
                    None
                }
                .or_else(|| {
                    snapshot.panes.iter().find(|p| {
                        x >= i32::from(p.x)
                            && x < i32::from(p.x + p.columns)
                            && y >= i32::from(p.y)
                            && y < i32::from(p.y + p.rows)
                    })
                })
                .cloned();
                if let Some(pane) = pane {
                    if !release && !motion && !wheel {
                        if !pane.focused {
                            client.select(snapshot, snapshot.tab_id, Some(pane.id));
                        }
                        self.mouse_capture = Some(pane.clone());
                    }
                    let column =
                        (x - i32::from(pane.x)).clamp(0, i32::from(pane.columns.saturating_sub(1)));
                    let row = y - i32::from(pane.y + pane.header);
                    if row >= 0 || release || motion {
                        let row =
                            row.clamp(0, i32::from(pane.rows.saturating_sub(pane.header + 1)));
                        client.mouse(
                            pane.session,
                            if release {
                                1
                            } else if motion {
                                2
                            } else {
                                0
                            },
                            button & !(4 | 8 | 16 | 32),
                            (column, row),
                            button & (4 | 8 | 16),
                            (
                                if pixel_mouse {
                                    raw_x - i32::from(pane.x) * i32::from(pixels.0) + 1
                                } else {
                                    column * i32::from(pixels.0) + 1
                                },
                                if pixel_mouse {
                                    raw_y - i32::from(pane.y + pane.header) * i32::from(pixels.1)
                                        + 1
                                } else {
                                    row * i32::from(pixels.1) + 1
                                },
                            ),
                        );
                    }
                }
                if release {
                    self.mouse_capture = None;
                }
            }
            _ => {}
        }
    }
    pub fn new() -> Self {
        Self {
            dirty: true,
            ..Self::default()
        }
    }
    pub fn feed(&mut self, bytes: &[u8], client: &mut Client, snapshot: &Snapshot) {
        if let Some((key, modifiers, event)) = kitty_key(bytes) {
            if key == 93 && modifiers == 5 {
                if event != 3 {
                    self.feed(&[29], client, snapshot);
                }
                return;
            }
            if self.prefix.pending() || !self.application() {
                if event == 3 {
                    return;
                }
                if let Some(character) = char::from_u32(key) {
                    let mut buffer = [0; 4];
                    let text = character.encode_utf8(&mut buffer);
                    self.feed(text.as_bytes(), client, snapshot);
                    return;
                }
            }
        }
        for event in self.prefix.feed(bytes) {
            match event {
                Input::Command(key) => self.command(key, client, snapshot),
                Input::Bytes(bytes) if self.prompt.is_some() || self.view != View::Terminal => {
                    for byte in bytes {
                        let pending = self.decoder.pending_escape();
                        let event = self.decoder.feed(byte);
                        if !pending && self.decoder.pending_escape() {
                            self.escape_deadline =
                                Some(Instant::now() + Duration::from_millis(200));
                        }
                        if let Some(key) = event {
                            self.key(key, client, snapshot);
                        }
                        if self.prompt.is_none() && self.view == View::Terminal {
                            self.decoder.reset();
                            break;
                        }
                    }
                }
                Input::Bytes(bytes) => client.input(&bytes),
            }
        }
    }
    pub fn expire_escape(&mut self, client: &mut Client, snapshot: &Snapshot) {
        if self.decoder.pending_escape()
            && self
                .escape_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.decoder.reset();
            self.key(Key::Escape, client, snapshot);
        }
    }
    pub fn escape_timeout(&self) -> Option<i32> {
        self.decoder
            .pending_escape()
            .then_some(self.escape_deadline)
            .flatten()
            .map(|deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_nanos()
                    .div_ceil(1_000_000) as i32
            })
    }
    fn command(&mut self, key: u8, client: &mut Client, snapshot: &Snapshot) {
        self.dirty = true;
        match key {
            b'd' => {
                self.quit = true;
                return;
            }
            b'l' => {
                self.redraw = true;
                return;
            }
            _ => {}
        }
        if self.prompt.is_some() {
            self.notice = "Answer the prompt or press Esc first".into();
            return;
        }
        match key {
            b'?' => {
                self.view = if self.view == View::Help {
                    View::Terminal
                } else {
                    View::Help
                };
                self.help_offset = 0;
                self.decoder.reset();
            }
            b'w' | b's' => {
                self.view = View::Picker;
                self.query.clear();
                self.selected = 0;
                self.picker_offset = 0;
                self.decoder.reset();
                client.action(b'w', "", snapshot);
            }
            _ if self.view != View::Terminal => {
                self.notice = "Close this view with Esc before changing the workspace".into()
            }
            b',' | b'R' | b'x' | b'X' | b'!' => {
                self.prompt = Some(Prompt {
                    key,
                    confirm: matches!(key, b'x' | b'X' | b'!'),
                    session: snapshot.session,
                    target: snapshot.clone(),
                    editor: Editor::default(),
                })
            }
            _ => client.action(key, "", snapshot),
        }
    }
    fn key(&mut self, key: Key, client: &mut Client, snapshot: &Snapshot) {
        self.dirty = true;
        if key == Key::Escape {
            if self.prompt.is_none() && self.view == View::Picker && !self.query.is_empty() {
                self.query.clear();
                self.selected = 0;
                self.picker_offset = 0;
                return;
            }
            if self.prompt.take().is_none() {
                self.view = View::Terminal;
            }
            self.notice.clear();
            self.decoder.reset();
            return;
        }
        if let Some(prompt) = &mut self.prompt {
            let accept = if prompt.confirm {
                matches!(&key, Key::Text(value) if value == "y" || value == "Y")
            } else {
                key == Key::Enter
            };
            if accept {
                let prompt = self.prompt.take().unwrap();
                if prompt.session != snapshot.session {
                    self.notice = "The focused pane changed; the operation was cancelled".into();
                } else {
                    client.action(prompt.key, &prompt.editor.text, &prompt.target);
                }
            } else if prompt.confirm {
                if matches!(&key, Key::Text(value) if value == "n" || value == "N") {
                    self.prompt = None;
                }
            } else {
                prompt.editor.apply(&key);
            }
            return;
        }
        if self.view == View::Picker {
            self.picker_key(key, client, snapshot);
            return;
        }
        if matches!(&key, Key::Text(value) if value == "q") {
            self.view = View::Terminal;
            return;
        }
        let page = usize::from(self.rows.saturating_sub(2).max(1));
        let count = help_lines(self.columns)
            .len()
            .saturating_sub(page)
            .saturating_add(1);
        let offset = &mut self.help_offset;
        match &key {
            Key::Up => *offset = offset.saturating_sub(1),
            Key::Down => *offset = offset.saturating_add(1),
            Key::PageUp => *offset = offset.saturating_sub(page),
            Key::PageDown => *offset = offset.saturating_add(page),
            Key::Home => *offset = 0,
            Key::End => *offset = count.saturating_sub(1),
            Key::Text(value) if value == "k" => *offset = offset.saturating_sub(1),
            Key::Text(value) if value == "j" => *offset = offset.saturating_add(1),
            _ => {}
        }
        *offset = (*offset).min(count.saturating_sub(1));
    }
    /// Typing filters; arrows and Ctrl-P/N move; Enter opens the row.
    fn picker_key(&mut self, key: Key, client: &mut Client, snapshot: &Snapshot) {
        let page = usize::from(self.rows.saturating_sub(3).max(1));
        let mut filtered = false;
        match &key {
            Key::Text(value) if self.query.chars().count() < 256 => {
                self.query.push_str(value);
                filtered = true;
            }
            Key::Backspace => {
                self.query.pop();
                filtered = true;
            }
            Key::Clear => {
                self.query.clear();
                filtered = true;
            }
            _ => {}
        }
        if filtered {
            self.selected = 0;
            self.picker_offset = 0;
            return;
        }
        let entries = picker::entries(&snapshot.directory, &self.query);
        let last = entries.len().saturating_sub(1);
        match &key {
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = self.selected.saturating_add(1),
            Key::PageUp => self.selected = self.selected.saturating_sub(page),
            Key::PageDown => self.selected = self.selected.saturating_add(page),
            Key::Home => self.selected = 0,
            Key::End => self.selected = last,
            Key::Enter => {
                if let Some(entry) = entries.get(self.selected.min(last)) {
                    self.open(&snapshot.directory[entry.index], client, snapshot);
                }
                return;
            }
            _ => {}
        }
        self.selected = self.selected.min(last);
    }
    /// Opens a picker row. A row of the attached workspace focuses its pane
    /// without reconnecting; any other row attaches, and a restorable
    /// workspace's rows recreate it.
    fn open(&mut self, row: &DirectoryRow, client: &mut Client, snapshot: &Snapshot) {
        let attached =
            snapshot.workspace_id != 0 && row.owner == snapshot.workspace_id && !row.restorable;
        if attached {
            if row.session != 0 && row.session != snapshot.session {
                client.reveal(row.session);
            }
        } else {
            client.attach(row.session, row.workspace);
            self.redraw = true;
        }
        self.view = View::Terminal;
        self.query.clear();
    }
    pub fn set_size(&mut self, columns: u16, rows: u16) {
        self.columns = columns;
        self.rows = rows;
    }
    pub fn render(
        &mut self,
        frame: &mut Frame,
        snapshot: &Snapshot,
        panes: &[(Pane, CoreFrame)],
        options: &Options,
    ) {
        self.tab_hits.clear();
        let modern = options.profile == Profile::Modern;
        let area = frame.area();
        let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        if self.view == View::Terminal {
            for (pane, data) in panes {
                if pane.columns == 0 || pane.rows <= pane.header {
                    continue;
                }
                let content = Rect::new(
                    pane.x,
                    pane.y + pane.header,
                    pane.columns,
                    pane.rows - pane.header,
                )
                .intersection(body);
                TerminalPane { data, modern }.render(content, frame.buffer_mut());
                if pane.header != 0 {
                    let text = format!(
                        "{}{} {}",
                        if pane.focused { "> " } else { "  " },
                        clean(&pane.title, !modern),
                        icon(pane.kind, modern)
                    );
                    frame.render_widget(
                        Paragraph::new(text).style(band(
                            if pane.focused { INK } else { 0x9caec4 },
                            if pane.focused { TEAL } else { 0x283548 },
                            modern,
                        )),
                        Rect::new(pane.x, pane.y, pane.columns, 1).intersection(body),
                    );
                }
                // A pane still connecting has no screen yet; its cursor
                // would be a lone block at the top left.
                if pane.focused
                    && self.prompt.is_none()
                    && snapshot.state.is_empty()
                    && let Some((x, y)) = data
                        .cursor
                        .filter(|(x, y)| *x < content.width && *y < content.height)
                {
                    frame.set_cursor_position((content.x + x, content.y + y));
                }
            }
        } else if self.view == View::Picker {
            self.picker_offset = picker::render(
                frame,
                body,
                snapshot,
                &self.query,
                self.selected,
                self.picker_offset,
                modern,
            );
        } else {
            frame.render_widget(Paragraph::new("").style(band(0xdce6f2, BASE, modern)), body);
            frame.render_widget(
                Paragraph::new(" Help / arrows, PgUp/PgDn; Esc close")
                    .style(band(0xdce6f2, 0x354961, modern)),
                Rect::new(0, 0, body.width, 1),
            );
            let contents = Rect::new(0, 1, body.width, body.height.saturating_sub(1));
            let lines = help_lines(body.width)
                .into_iter()
                .skip(self.help_offset)
                .map(Line::from)
                .collect::<Vec<_>>();
            frame.render_widget(Paragraph::new(Text::from(lines)), contents);
        }
        let bar = Rect::new(0, area.height.saturating_sub(1), area.width, 1);
        let notice = if let Some(prompt) = &self.prompt {
            if prompt.confirm {
                format!(
                    "Close {} and its programs? y/n (Esc cancels)",
                    match prompt.key {
                        b'x' => "pane",
                        b'X' => "tab",
                        _ => "window",
                    }
                )
            } else {
                format!(
                    "Rename {}: {}{} (Enter/Esc)",
                    if prompt.key == b',' {
                        "tab"
                    } else {
                        "workspace"
                    },
                    prompt.editor.text,
                    if modern { "▏" } else { "_" }
                )
            }
        } else if !self.notice.is_empty() {
            self.notice.clone()
        } else if !snapshot.notice.is_empty() {
            snapshot.notice.clone()
        } else if !snapshot.state.is_empty() {
            format!("{}  #{}", snapshot.state, snapshot.session)
        } else {
            String::new()
        };
        let look = match (modern, options.plain) {
            (true, false) => bar::Look::Powerline,
            (true, true) => bar::Look::Plain,
            (false, _) => bar::Look::Ascii,
        };
        self.tab_hits = bar::render(bar, frame.buffer_mut(), snapshot, look, &notice);
    }
}

pub struct TerminalPane<'a> {
    pub data: &'a CoreFrame,
    pub modern: bool,
}
impl Widget for TerminalPane<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        for y in 0..area.height.min(self.data.rows) {
            for x in 0..area.width.min(self.data.columns) {
                let Some(cell) = self
                    .data
                    .cells
                    .get(usize::from(y) * usize::from(self.data.columns) + usize::from(x))
                else {
                    continue;
                };
                let target = &mut buffer[(area.x + x, area.y + y)];
                if cell.width == 0 {
                    target.set_diff_option(CellDiffOption::Skip);
                    continue;
                }
                let mut style = Style::default();
                if self.modern {
                    style = style.fg(rgb(cell.foreground)).bg(rgb(cell.background));
                    if cell.attributes & 2 != 0 {
                        style = style.underline_color(rgb(cell.underline));
                    }
                }
                for (flag, modifier) in [
                    (1, Modifier::BOLD),
                    (2, Modifier::UNDERLINED),
                    (4, Modifier::SLOW_BLINK),
                    (8, Modifier::REVERSED),
                    (16, Modifier::ITALIC),
                    (32, Modifier::CROSSED_OUT),
                ] {
                    if cell.attributes & flag != 0 {
                        style = style.add_modifier(modifier);
                    }
                }
                target.set_symbol(&cell.text).set_style(style);
            }
        }
    }
}
fn help_lines(width: u16) -> Vec<String> {
    let sections = [
        "TABS: c new tab; n/p next/previous; 1..9 select; , rename",
        "PANES: | split side by side; - split above/below; o next pane",
        "FOCUS: H/J/K/L left/down/up/right; z zoom; = balance",
        "LAYOUT: +/_ grow/shrink; S swap next; t extract to tab; m move to next tab",
        "CLOSE: x pane; X tab; ! window (confirmation required)",
        "WORKSPACES: w/s picker, type to filter; R rename; C new workspace; W next window; N new window",
        "CLIENT: l redraw; d detach; Ctrl-] twice sends a literal prefix",
        "STATUS: working; done; attention/blocked; failed",
        "All commands use Ctrl-] then the listed key.",
    ];
    let mut lines = Vec::new();
    for section in sections {
        let mut line = String::from(" ");
        for word in section.split_whitespace() {
            if line.width() + word.width() + 1 > usize::from(width) && line.len() > 1 {
                lines.push(std::mem::replace(&mut line, " ".into()));
            }
            if line.len() > 1 {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use utty::core::DirectoryRow;
    #[test]
    fn picker_handles_fragmented_arrows_and_control_navigation() {
        let mut client = Client::new(&Options::default(), 132, 53).unwrap();
        let snapshot = Snapshot {
            directory: (0..7)
                .map(|index| DirectoryRow {
                    kind: 3,
                    session: index + 1,
                    title: format!("pane{index}"),
                    ..DirectoryRow::default()
                })
                .collect(),
            ..Snapshot::default()
        };
        let mut ui = Ui::new();
        ui.set_size(132, 53);
        // Typing filters; the first Esc clears the filter, the second closes.
        ui.feed(b"\x1dwpane5", &mut client, &snapshot);
        assert_eq!(ui.query, "pane5");
        assert_eq!(picker::entries(&snapshot.directory, &ui.query).len(), 1);
        for open in [true, false] {
            ui.feed(b"\x1b", &mut client, &snapshot);
            ui.escape_deadline = Some(Instant::now());
            ui.expire_escape(&mut client, &snapshot);
            assert_eq!(ui.view == View::Picker, open);
            assert!(ui.query.is_empty());
        }
        ui.feed(b"\x1dw", &mut client, &snapshot);
        assert!(ui.view == View::Picker);
        ui.feed(b"\x1b", &mut client, &snapshot);
        ui.feed(b"[B\x1b[B\x1b[A\x0e\x0e\x10\x0e\r", &mut client, &snapshot);
        assert_eq!(ui.selected, 3);
        assert!(ui.view == View::Terminal);
    }
}
