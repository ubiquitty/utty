// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
//! Subsequence matching for the workspace picker, after fzf's first
//! algorithm: the shortest window holding the query in order, scored for
//! word starts and runs.

/// Scores `text` against `query`, ignoring case.
///
/// - Returns: The score, higher being better, and the matched character
///   indices into `text`; nothing when `text` lacks the query in order. An
///   empty query matches everything with score zero.
pub fn score(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let query = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<Vec<_>>();
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let original = text.chars().collect::<Vec<_>>();
    let folded = original
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect::<Vec<_>>();
    // Forward: the first place the whole query ends.
    let mut next = 0;
    let mut end = None;
    for (index, c) in folded.iter().enumerate() {
        if *c == query[next] {
            next += 1;
            if next == query.len() {
                end = Some(index);
                break;
            }
        }
    }
    let end = end?;
    // Backward from there: the latest start, so the window is shortest.
    let mut remaining = query.len();
    let mut start = end;
    for index in (0..=end).rev() {
        if folded[index] == query[remaining - 1] {
            remaining -= 1;
            if remaining == 0 {
                start = index;
                break;
            }
        }
    }
    let boundary = |index: usize| {
        index == 0 || {
            let previous = original[index - 1];
            let current = original[index];
            !previous.is_alphanumeric() || (previous.is_lowercase() && current.is_uppercase())
        }
    };
    let mut positions = Vec::with_capacity(query.len());
    let mut next = 0;
    let mut score = 0;
    let mut previous: Option<usize> = None;
    for (index, c) in folded.iter().enumerate().take(end + 1).skip(start) {
        if next < query.len() && *c == query[next] {
            score += 16;
            if boundary(index) {
                score += 10;
            }
            match previous {
                Some(last) if last + 1 == index => score += 12,
                Some(last) => score -= (index - last - 1).min(8) as i32,
                None => {}
            }
            positions.push(index);
            previous = Some(index);
            next += 1;
        }
    }
    // Earlier matches read as better: a leading gap costs a little.
    score -= (start.min(20) as i32) / 2;
    Some((score, positions))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_in_order_ignoring_case_and_prefers_word_starts_and_runs() {
        assert!(score("xyz", "Main").is_none());
        assert_eq!(score("", "anything"), Some((0, Vec::new())));
        let (_, positions) = score("mn", "Main").unwrap();
        assert_eq!(positions, vec![0, 3]);
        let run = score("cla", "Claude Code").unwrap().0;
        let scattered = score("cla", "cxlxa").unwrap().0;
        assert!(run > scattered);
        let start = score("cc", "Claude Code").unwrap();
        assert_eq!(start.1, vec![0, 7]);
        let word = score("api", "srv/api").unwrap().0;
        let inner = score("api", "rapid").unwrap().0;
        assert!(word > inner);
        // The shortest window wins over the first occurrence.
        assert_eq!(score("ab", "a__ab").unwrap().1, vec![3, 4]);
        assert!(score("w ma", "Workspace Main").is_some());
    }
}
