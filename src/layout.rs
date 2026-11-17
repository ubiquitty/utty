// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
use utty::core::{Pane, Snapshot, Split};

pub fn place(snapshot: &mut Snapshot, columns: u16, rows: u16) {
    let rows = rows.saturating_sub(1);
    if snapshot.zoomed || snapshot.panes.len() == 1 {
        let focused = snapshot
            .panes
            .iter()
            .position(|pane| pane.focused)
            .unwrap_or(0);
        if let Some(mut pane) = snapshot.panes.get(focused).cloned() {
            pane.x = 0;
            pane.y = 0;
            pane.columns = columns;
            pane.rows = rows;
            pane.header = 0;
            snapshot.panes = vec![pane];
        }
        return;
    }
    let mut leaf = 0;
    visit(
        &snapshot.split,
        &mut snapshot.panes,
        &mut leaf,
        (0, 0, columns, rows),
        0,
    );
}
fn visit(
    node: &Split,
    panes: &mut [Pane],
    leaf: &mut usize,
    area: (u16, u16, u16, u16),
    depth: usize,
) {
    if depth > 64 {
        return;
    }
    let (x, y, columns, rows) = area;
    if node.children.is_empty() {
        if let Some(pane) = panes.get_mut(*leaf) {
            pane.x = x;
            pane.y = y;
            pane.columns = columns;
            pane.rows = rows;
            pane.header = 1;
        }
        *leaf += 1;
        return;
    }
    let extent = if node.horizontal { columns } else { rows };
    let mut position = 0u16;
    for (index, child) in node.children.iter().enumerate() {
        let remaining = extent.saturating_sub(position);
        let length = if index + 1 == node.children.len() {
            remaining
        } else {
            let ratio = if node.children.len() == 2 && node.ratio.is_finite() {
                node.ratio.clamp(0.05, 0.95)
            } else {
                1.0 / (node.children.len() - index) as f64
            };
            (f64::from(remaining.saturating_sub(1)) * ratio).floor() as u16
        };
        let child_area = if node.horizontal {
            (x.saturating_add(position), y, length, rows)
        } else {
            (x, y.saturating_add(position), columns, length)
        };
        visit(child, panes, leaf, child_area, depth + 1);
        position = position.saturating_add(length).saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_reserves_a_gutter_headers_and_the_workspace_bar() {
        let mut snapshot = Snapshot {
            split: Split {
                horizontal: true,
                ratio: 0.5,
                children: vec![Split::default(), Split::default()],
            },
            panes: vec![Pane::default(), Pane::default()],
            ..Snapshot::default()
        };
        place(&mut snapshot, 80, 24);
        assert_eq!(
            (
                snapshot.panes[0].columns,
                snapshot.panes[1].x,
                snapshot.panes[1].columns
            ),
            (39, 40, 40)
        );
        assert_eq!((snapshot.panes[0].rows, snapshot.panes[0].header), (23, 1));
        snapshot.zoomed = true;
        snapshot.panes[1].focused = true;
        place(&mut snapshot, 80, 24);
        assert_eq!(snapshot.panes.len(), 1);
        assert_eq!(
            (
                snapshot.panes[0].columns,
                snapshot.panes[0].rows,
                snapshot.panes[0].header
            ),
            (80, 23, 0)
        );
    }
}
