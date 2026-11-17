// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0

/// Makes host-supplied text safe to print: control and bidirectional
/// override characters become '?', as does non-ASCII text for seven-bit
/// profiles. Bounded to 4096 characters.
pub fn clean(text: &str, ascii: bool) -> String {
    text.chars()
        .take(4096)
        .map(|c| {
            if c.is_control()
                || matches!(c, '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                || (ascii && !c.is_ascii())
            {
                '?'
            } else {
                c
            }
        })
        .collect()
}
