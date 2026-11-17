// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
//! The workspace bar: the workspace and its tabs on the left, the focused
//! program's status and the session on the right.
//!
//! Modern terminals get powerline segments, whose arrows take the colors of
//! the segments they join; `--no-powerline` keeps the colors without the
//! glyphs, and the seven-bit profiles mark the active tab by reverse video.
use crate::ui::{INK, TEAL, icon, rgb, status_color};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;
use utty::{core::Snapshot, text::clean};

const BAR: u32 = 0x1e2734;
const ACTIVE: u32 = 0x3a4e6a;
const ACTIVE_TEXT: u32 = 0xf1f5fa;
const IDLE_TEXT: u32 = 0x93a4bb;
const THIN: u32 = 0x4a5a72;
const SESSION: u32 = 0x2c3a4f;
const SESSION_TEXT: u32 = 0xc9d6e6;
const NOTICE: u32 = 0xffcf70;

const RIGHT: &str = "\u{e0b0}";
const RIGHT_THIN: &str = "\u{e0b1}";
const LEFT: &str = "\u{e0b2}";
const LEFT_THIN: &str = "\u{e0b3}";

/// How the bar is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Powerline,
    Plain,
    Ascii,
}

struct Segment {
    text: String,
    foreground: u32,
    background: u32,
    bold: bool,
    /// The status icon after the text, in its status color.
    mark: Option<(String, u32)>,
    tab: Option<u32>,
}
impl Segment {
    fn new(text: String, foreground: u32, background: u32) -> Self {
        Self {
            text,
            foreground,
            background,
            bold: false,
            mark: None,
            tab: None,
        }
    }
    fn width(&self) -> usize {
        self.text.width() + self.mark.as_ref().map_or(0, |(mark, _)| mark.width())
    }
}

/// Writes text from left to right, clipped to the bar.
struct Pen<'a> {
    buffer: &'a mut Buffer,
    area: Rect,
    x: u16,
    look: Look,
}
impl Pen<'_> {
    fn style(&self, foreground: u32, background: u32, bold: bool, reverse: bool) -> Style {
        let mut style = Style::default();
        if self.look == Look::Ascii {
            if reverse {
                style = style.add_modifier(Modifier::REVERSED);
            }
        } else {
            style = style.fg(rgb(foreground)).bg(rgb(background));
        }
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    }
    fn write(&mut self, text: &str, style: Style) {
        let right = self.area.x + self.area.width;
        if self.x >= right {
            return;
        }
        let (x, _) = self.buffer.set_stringn(
            self.x,
            self.area.y,
            text,
            usize::from(right - self.x),
            style,
        );
        self.x = x;
    }
}

fn status_text(snapshot: &Snapshot, look: Look) -> Option<Segment> {
    let modern = look != Look::Ascii;
    let mark = icon(snapshot.kind, modern);
    let label = if !snapshot.status_title.is_empty() {
        snapshot.status_title.as_str()
    } else if !snapshot.app.is_empty() && snapshot.app != "utty" {
        snapshot.app.as_str()
    } else {
        ""
    };
    let detail = if !snapshot.message.is_empty() {
        snapshot.message.clone()
    } else {
        match snapshot.block.as_str() {
            "permission" => "needs permission".into(),
            "question" => "has a question".into(),
            "auth" => "needs sign-in".into(),
            _ => String::new(),
        }
    };
    let progress = (snapshot.progress >= 0).then(|| format!("{}%", snapshot.progress.min(100)));
    let parts = [label.to_string(), detail]
        .into_iter()
        .chain(progress)
        .filter(|part| !part.is_empty())
        .map(|part| clean(&part, !modern))
        .collect::<Vec<_>>();
    if mark.is_empty() && parts.is_empty() {
        return None;
    }
    let mut text = String::new();
    if !mark.is_empty() {
        text.push_str(mark);
    }
    for part in parts {
        if !text.is_empty() {
            text.push_str(if modern { " · " } else { " - " });
        }
        text.push_str(&part);
    }
    Some(Segment::new(
        format!(" {text} "),
        status_color(snapshot.kind).unwrap_or(IDLE_TEXT),
        BAR,
    ))
}

fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.into();
    }
    let mut result = String::new();
    for c in text.chars() {
        if result.width() + c.to_string().width() + 2 > width {
            break;
        }
        result.push(c);
    }
    format!("{result}… ")
}

/// The width of the right side as drawn: each segment after the arrow out of
/// its left neighbour, or a separator between plain segments.
fn right_width(segments: &[Segment], look: Look) -> usize {
    let mut previous = BAR;
    let mut width = 0;
    for (index, segment) in segments.iter().enumerate() {
        let separated = match look {
            Look::Powerline => segment.background != previous || index > 0,
            _ => index > 0,
        };
        width += segment.width() + usize::from(separated);
        previous = segment.background;
    }
    width
}

/// A run of left segments' width, one separator after each: an upper bound,
/// since the last plain segment has none.
fn left_width(segments: &[Segment]) -> usize {
    segments.iter().map(|segment| segment.width() + 1).sum()
}

/// The tabs that fit in `budget`, grown outward from the active one.
fn visible(tabs: &[Segment], active: usize, budget: usize) -> (usize, usize) {
    if tabs.is_empty() {
        return (0, 0);
    }
    // Room is kept for the "‹" and "›" markers and their separators.
    let fits = |start: usize, end: usize| left_width(&tabs[start..end]) + 6 <= budget;
    let (mut start, mut end) = (active, active + 1);
    loop {
        let grew_right = end < tabs.len() && fits(start, end + 1);
        if grew_right {
            end += 1;
        }
        let grew_left = start > 0 && fits(start - 1, end);
        if grew_left {
            start -= 1;
        }
        if !grew_right && !grew_left {
            return (start, end);
        }
    }
}

/// Draws the bar and returns each visible tab's clickable area.
pub fn render(
    area: Rect,
    buffer: &mut Buffer,
    snapshot: &Snapshot,
    look: Look,
    notice: &str,
) -> Vec<(Rect, u32)> {
    let modern = look != Look::Ascii;
    let mut pen = Pen {
        buffer,
        area,
        x: area.x,
        look,
    };
    let fill = pen.style(IDLE_TEXT, BAR, false, false);
    pen.buffer.set_style(area, fill);
    let name = if !snapshot.name.is_empty() {
        clean(&snapshot.name, !modern)
    } else if snapshot.workspace_id != 0 {
        format!("Workspace {}", snapshot.workspace_id)
    } else {
        "Workspace".into()
    };
    let mut left = vec![Segment {
        bold: true,
        ..Segment::new(format!(" {name} "), INK, TEAL)
    }];
    let mut hits = Vec::new();
    let mut right = Vec::new();
    if notice.is_empty() {
        let tabs = snapshot
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let mark = icon(tab.kind, modern);
                let (foreground, background) = if tab.active {
                    (ACTIVE_TEXT, ACTIVE)
                } else {
                    (IDLE_TEXT, BAR)
                };
                Segment {
                    bold: tab.active,
                    mark: (!mark.is_empty()).then(|| {
                        (
                            format!("{mark} "),
                            status_color(tab.kind).unwrap_or(foreground),
                        )
                    }),
                    tab: Some(tab.id),
                    ..Segment::new(
                        format!(" {}:{} ", index + 1, clean(&tab.title, !modern)),
                        foreground,
                        background,
                    )
                }
            })
            .collect::<Vec<_>>();
        right.push(Segment::new(
            format!(" #{} ", snapshot.session),
            SESSION_TEXT,
            SESSION,
        ));
        let width = usize::from(area.width);
        if let Some(mut status) = status_text(snapshot, look) {
            let room = width.saturating_sub(left_width(&left) + right_width(&right, look) + 12);
            let limit = room.min(width * 2 / 5).max(12);
            status.text = truncate(&status.text, limit);
            right.insert(0, status);
        }
        let budget = width
            .saturating_sub(left_width(&left))
            .saturating_sub(right_width(&right, look));
        let active = snapshot.tabs.iter().position(|tab| tab.active).unwrap_or(0);
        let (start, end) = visible(&tabs, active, budget);
        let (before, after) = (start > 0, end < tabs.len());
        let mut tabs = tabs;
        left.extend(tabs.drain(start..end));
        if before {
            left.insert(
                1,
                Segment::new(if modern { " ‹" } else { " <" }.into(), IDLE_TEXT, BAR),
            );
        }
        if after {
            left.push(Segment::new(
                if modern { "› " } else { "> " }.into(),
                IDLE_TEXT,
                BAR,
            ));
        }
    } else {
        left.push(Segment {
            bold: true,
            ..Segment::new(format!(" {} ", clean(notice, !modern)), NOTICE, BAR)
        });
    }
    // Left: each segment, then the arrow into whatever follows it.
    for (index, segment) in left.iter().enumerate() {
        let start = pen.x;
        let reverse = segment.background != BAR;
        let style = pen.style(
            segment.foreground,
            segment.background,
            segment.bold,
            reverse,
        );
        pen.write(&segment.text, style);
        if let Some((mark, color)) = &segment.mark {
            let style = pen.style(*color, segment.background, segment.bold, reverse);
            pen.write(mark, style);
        }
        if let Some(id) = segment.tab {
            hits.push((Rect::new(start, area.y, pen.x - start, 1), id));
        }
        let next = left.get(index + 1).map_or(BAR, |next| next.background);
        match look {
            Look::Powerline if segment.background != next => {
                let style = pen.style(segment.background, next, false, false);
                pen.write(RIGHT, style);
            }
            Look::Powerline if index + 1 < left.len() => {
                let style = pen.style(THIN, next, false, false);
                pen.write(RIGHT_THIN, style);
            }
            Look::Powerline => {}
            _ if index == 0 => {
                let style = pen.style(IDLE_TEXT, BAR, false, false);
                pen.write(if modern { " " } else { "|" }, style);
            }
            _ if index + 1 < left.len() => {
                let style = pen.style(IDLE_TEXT, BAR, false, false);
                pen.write(" ", style);
            }
            _ => {}
        }
    }
    // Right: right-aligned, each segment after the arrow out of the previous.
    let width = right_width(&right, look) as u16;
    if width <= area.width.saturating_sub(pen.x - area.x) {
        pen.x = area.x + area.width - width;
        let mut previous = BAR;
        for (index, segment) in right.iter().enumerate() {
            match look {
                Look::Powerline if segment.background != previous => {
                    let style = pen.style(segment.background, previous, false, false);
                    pen.write(LEFT, style);
                }
                Look::Powerline if index > 0 => {
                    let style = pen.style(THIN, previous, false, false);
                    pen.write(LEFT_THIN, style);
                }
                Look::Powerline => {}
                _ if index > 0 => {
                    let style = pen.style(IDLE_TEXT, BAR, false, false);
                    pen.write(if modern { " " } else { "|" }, style);
                }
                _ => {}
            }
            let reverse = segment.background != BAR;
            let style = pen.style(
                segment.foreground,
                segment.background,
                segment.bold,
                reverse,
            );
            pen.write(&segment.text, style);
            previous = segment.background;
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use utty::core::Tab;
    fn text(buffer: &Buffer) -> String {
        buffer.content.iter().map(|cell| cell.symbol()).collect()
    }
    fn snapshot(tabs: usize) -> Snapshot {
        Snapshot {
            name: "Main".into(),
            session: 12,
            tabs: (0..tabs)
                .map(|index| Tab {
                    id: index as u32 + 1,
                    title: format!("tab{index}"),
                    active: index == 1,
                    ..Tab::default()
                })
                .collect(),
            ..Snapshot::default()
        }
    }
    #[test]
    fn powerline_joins_segments_and_puts_status_and_session_right() {
        let area = Rect::new(0, 0, 80, 1);
        let mut buffer = Buffer::empty(area);
        let mut value = snapshot(3);
        value.kind = 3;
        value.app = "build".into();
        value.progress = 40;
        let hits = render(area, &mut buffer, &value, Look::Powerline, "");
        let line = text(&buffer);
        assert!(line.starts_with(" Main \u{e0b0} 1:tab0 \u{e0b0} 2:tab1 \u{e0b0} 3:tab2"));
        assert!(line.trim_end().ends_with(" ◌ · build · 40% \u{e0b2} #12"));
        assert_eq!(
            hits.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        // The active tab's arrows carry its color into its neighbours.
        let arrow = &buffer[(6, 0)];
        assert_eq!(arrow.fg, rgb(TEAL));
        assert_eq!(arrow.bg, rgb(BAR));
    }
    #[test]
    fn narrow_bars_keep_the_active_tab_and_the_session() {
        let area = Rect::new(0, 0, 40, 1);
        let mut buffer = Buffer::empty(area);
        let mut value = snapshot(12);
        for tab in &mut value.tabs {
            tab.active = tab.id == 9;
        }
        let hits = render(area, &mut buffer, &value, Look::Powerline, "");
        let line = text(&buffer);
        assert!(line.contains("9:tab8"), "{line}");
        assert!(line.contains("‹") && line.contains("›"));
        assert!(line.trim_end().ends_with("#12"), "{line}");
        assert!(hits.iter().any(|(_, id)| *id == 9));
        let mut ascii = Buffer::empty(area);
        render(area, &mut ascii, &value, Look::Ascii, "");
        assert!(text(&ascii).is_ascii());
    }
    #[test]
    fn a_notice_replaces_the_tabs() {
        let area = Rect::new(0, 0, 60, 1);
        let mut buffer = Buffer::empty(area);
        let hits = render(
            area,
            &mut buffer,
            &snapshot(3),
            Look::Plain,
            "Rename tab: x",
        );
        assert!(text(&buffer).contains("Rename tab: x"));
        assert!(hits.is_empty());
    }
}
