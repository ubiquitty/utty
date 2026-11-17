// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
//! The in-session workspace picker: the host's directory as a tree, or, once
//! a query is typed, its rows ranked by fuzzy match.
use crate::ui::{BASE, INK, TEAL, band, icon, rgb, status_color};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
};
use unicode_width::UnicodeWidthStr;
use utty::{
    connect::ago,
    core::{DirectoryRow, Snapshot},
    fuzzy,
    text::clean,
};

const DIM: u32 = 0x7f8ea3;
const AMBER: u32 = 0xffcf70;
const TEXT: u32 = 0xdce6f2;

/// A row shown by the picker, with the characters of its search text that
/// matched the query.
pub struct Entry {
    pub index: usize,
    positions: Vec<usize>,
}

/// A row's own words: what it is called in the tree.
fn label(row: &DirectoryRow) -> String {
    match row.kind {
        0 if !row.name.is_empty() => row.name.clone(),
        0 if !row.title.is_empty() => row.title.clone(),
        0 => format!("Workspace {}", row.owner),
        3 if row.session != 0 => format!("#{} {}", row.session, row.title),
        _ => row.title.clone(),
    }
}

fn home(detail: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && detail.starts_with(&home) => {
            format!("~{}", &detail[home.len()..])
        }
        _ => detail.to_string(),
    }
}

/// Each row's search text: its workspace, tab and own words, so a query
/// can name a pane by where it is.
fn paths(rows: &[DirectoryRow], separator: &str) -> Vec<String> {
    let (mut workspace, mut tab) = (String::new(), String::new());
    rows.iter()
        .map(|row| {
            let own = clean(&label(row), false);
            let detail = clean(&home(&row.detail), false);
            match row.kind {
                0 => {
                    workspace = own.clone();
                    format!("{own} #{}", row.owner)
                }
                1 => format!("{workspace}{separator}{own}"),
                2 => {
                    tab = own.clone();
                    format!("{workspace}{separator}{own}")
                }
                _ if detail.is_empty() => format!("{workspace}{separator}{tab}{separator}{own}"),
                _ => format!("{workspace}{separator}{tab}{separator}{own}  {detail}"),
            }
        })
        .collect()
}

/// The rows to show: every row in tree order without a query; otherwise the
/// matching rows, best first, ties kept in tree order.
pub fn entries(rows: &[DirectoryRow], query: &str) -> Vec<Entry> {
    if query.trim().is_empty() {
        return (0..rows.len())
            .map(|index| Entry {
                index,
                positions: Vec::new(),
            })
            .collect();
    }
    let mut ranked = paths(rows, " › ")
        .iter()
        .enumerate()
        .filter_map(|(index, path)| {
            fuzzy::score(query, path).map(|(score, positions)| (score, index, positions))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked
        .into_iter()
        .map(|(_, index, positions)| Entry { index, positions })
        .collect()
}

fn style(color: u32, modern: bool) -> Style {
    if modern {
        Style::default().fg(rgb(color))
    } else {
        Style::default()
    }
}

/// What a workspace row says on the right: whether it is the one attached,
/// recorded only, how big, who is on it and when it was last used.
fn info(row: &DirectoryRow, snapshot: &Snapshot, modern: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut push = |text: String, color: u32| {
        if !spans.is_empty() {
            spans.push(Span::styled("  ", style(DIM, modern)));
        }
        spans.push(Span::styled(text, style(color, modern)));
    };
    if row.owner == snapshot.workspace_id {
        push("attached".into(), TEAL);
    }
    if row.people > 0 && row.owner != snapshot.workspace_id {
        push(format!("{} attached", row.people), TEXT);
    }
    push(
        format!(
            "{} pane{}",
            row.panes,
            if row.panes == 1 { "" } else { "s" }
        ),
        DIM,
    );
    if row.restorable {
        push("restorable".into(), AMBER);
    } else {
        push(ago(snapshot.now_ms, row.active_ms), DIM);
    }
    spans.push(Span::raw(" "));
    spans
}

fn status(row: &DirectoryRow, modern: bool) -> Option<Span<'static>> {
    let mark = icon(row.status, modern);
    (!mark.is_empty()).then(|| {
        Span::styled(
            format!(" {mark}"),
            style(status_color(row.status).unwrap_or(TEXT), modern),
        )
    })
}

/// Splits `text` into spans, emphasizing the characters at `positions`.
fn highlighted(text: &str, positions: &[usize], base: Style, modern: bool) -> Vec<Span<'static>> {
    let emphasis = if modern {
        base.fg(rgb(TEAL)).add_modifier(Modifier::BOLD)
    } else {
        base.add_modifier(Modifier::UNDERLINED)
    };
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut matched = false;
    for (index, c) in text.chars().enumerate() {
        let here = positions.binary_search(&index).is_ok();
        if here != matched && !run.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if matched { emphasis } else { base },
            ));
        }
        matched = here;
        run.push(c);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if matched { emphasis } else { base }));
    }
    spans
}

fn line(
    row: &DirectoryRow,
    entry: &Entry,
    path: &str,
    filtering: bool,
    snapshot: &Snapshot,
    width: usize,
    modern: bool,
) -> Line<'static> {
    let text = style(TEXT, modern);
    let dim = style(DIM, modern);
    let current = row.owner == snapshot.workspace_id && snapshot.workspace_id != 0;
    let mut spans = vec![Span::styled(
        if current && row.kind == 0 {
            if modern { "● " } else { "* " }
        } else {
            "  "
        },
        style(TEAL, modern),
    )];
    if filtering {
        spans.extend(highlighted(path, &entry.positions, text, modern));
    } else {
        spans.push(Span::raw("  ".repeat(usize::from(row.kind))));
        match row.kind {
            0 => {
                spans.push(Span::styled(
                    clean(&label(row), !modern),
                    text.add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(format!(" #{}", row.owner), dim));
                if !row.name.is_empty() && !row.title.is_empty() && row.title != row.name {
                    spans.push(Span::styled(
                        format!("  {}", clean(&row.title, !modern)),
                        dim,
                    ));
                }
            }
            1 => spans.push(Span::styled(clean(&row.title, !modern), dim)),
            2 => spans.push(Span::styled(clean(&row.title, !modern), text)),
            _ => {
                if row.session != 0 {
                    spans.push(Span::styled(format!("#{} ", row.session), dim));
                }
                spans.push(Span::styled(clean(&row.title, !modern), text));
                if !row.detail.is_empty() {
                    spans.push(Span::styled(
                        format!("  {}", clean(&home(&row.detail), !modern)),
                        dim,
                    ));
                }
            }
        }
    }
    if row.kind != 0
        && let Some(mark) = status(row, modern)
    {
        spans.push(mark);
    }
    if row.restorable && row.kind != 0 {
        spans = spans
            .into_iter()
            .map(|span| {
                let style = span.style.add_modifier(Modifier::DIM);
                span.style(style)
            })
            .collect();
    }
    if row.kind == 0 {
        let right = info(row, snapshot, modern);
        let used = spans.iter().map(|span| span.content.width()).sum::<usize>();
        let wanted = right.iter().map(|span| span.content.width()).sum::<usize>();
        if used + wanted + 2 <= width {
            spans.push(Span::raw(" ".repeat(width - used - wanted)));
            spans.extend(right);
        }
    }
    Line::from(spans)
}

/// Draws the picker over `area` and returns the list offset it scrolled to.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    snapshot: &Snapshot,
    query: &str,
    selected: usize,
    offset: usize,
    modern: bool,
) -> usize {
    frame.render_widget(Paragraph::new("").style(band(TEXT, BASE, modern)), area);
    let shown = entries(&snapshot.directory, query);
    let total = snapshot
        .directory
        .iter()
        .filter(|row| row.kind == 0)
        .count();
    let header = band(TEXT, 0x354961, modern);
    // "Workspaces /" appears once the listing is live: it says the rows can
    // be acted on.
    let mut heading = vec![Span::styled(
        match (snapshot.listing, modern) {
            (true, true) => " Loading workspaces…  ",
            (true, false) => " Loading workspaces...  ",
            (false, _) => " Workspaces / ",
        },
        header.add_modifier(Modifier::BOLD),
    )];
    if query.is_empty() {
        heading.push(Span::styled(
            "type to filter",
            header.patch(style(0xa9b8cc, modern)),
        ));
    } else {
        heading.push(Span::styled(clean(query, !modern), header));
        heading.push(Span::styled(if modern { "▏" } else { "_" }, header));
    }
    let hint = if query.is_empty() {
        format!(
            "{total} workspace{}  ↑↓ select  Enter open  Esc close ",
            if total == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "{} match{}  ↑↓ select  Enter open  Esc clear ",
            shown.len(),
            if shown.len() == 1 { "" } else { "es" }
        )
    };
    let hint = if modern {
        hint
    } else {
        hint.replace("↑↓", "Up/Down")
    };
    let used = heading
        .iter()
        .map(|span| span.content.width())
        .sum::<usize>();
    if used + hint.width() < usize::from(area.width) {
        heading.push(Span::styled(
            " ".repeat(usize::from(area.width) - used - hint.width()),
            header,
        ));
        heading.push(Span::styled(hint, header.patch(style(0xa9b8cc, modern))));
    }
    frame.render_widget(
        Paragraph::new(Line::from(heading)).style(header),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let list = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    let filtering = !query.trim().is_empty();
    let paths = paths(&snapshot.directory, if modern { " › " } else { " > " });
    let width = usize::from(list.width);
    let items = shown
        .iter()
        .map(|entry| {
            ListItem::new(line(
                &snapshot.directory[entry.index],
                entry,
                &paths[entry.index],
                filtering,
                snapshot,
                width,
                modern,
            ))
        })
        .collect::<Vec<_>>();
    if items.is_empty() && filtering {
        frame.render_widget(Paragraph::new("  No match").style(style(DIM, modern)), list);
        return 0;
    }
    let mut state = ListState::default()
        .with_offset(offset)
        .with_selected(Some(selected.min(items.len().saturating_sub(1))));
    frame.render_stateful_widget(
        List::new(items).highlight_style(band(INK, TEAL, modern)),
        list,
        &mut state,
    );
    state.offset()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(kind: u8, owner: u32, session: u32, title: &str) -> DirectoryRow {
        DirectoryRow {
            kind,
            owner,
            session,
            title: title.into(),
            ..DirectoryRow::default()
        }
    }
    #[test]
    fn queries_rank_rows_by_their_place_and_keep_tree_order_on_ties() {
        let mut main = row(0, 1, 0, "stas");
        main.name = "Main".into();
        let rows = vec![
            main,
            row(1, 1, 3, "Window 1"),
            row(2, 1, 3, "1:Claude Code"),
            row(3, 1, 3, "Claude Code"),
            row(0, 2, 0, "api"),
            row(1, 2, 9, "Window 1"),
            row(2, 2, 9, "1:zsh"),
            row(3, 2, 9, "zsh"),
        ];
        assert_eq!(entries(&rows, "").len(), rows.len());
        let shown = entries(&rows, "claude")
            .iter()
            .map(|entry| entry.index)
            .collect::<Vec<_>>();
        assert_eq!(shown, vec![2, 3]);
        // A workspace's name reaches its panes too; the workspace row leads.
        assert_eq!(entries(&rows, "main")[0].index, 0);
        assert_eq!(entries(&rows, "api zsh")[0].index, 6);
        assert!(entries(&rows, "#9").iter().any(|entry| entry.index == 7));
        assert!(entries(&rows, "nothing").is_empty());
    }
}
