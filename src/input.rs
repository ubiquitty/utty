// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Bytes(Vec<u8>),
    Command(u8),
}
#[derive(Default)]
pub struct Prefix {
    pending: bool,
}
impl Prefix {
    pub fn pending(&self) -> bool {
        self.pending
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Input> {
        let mut result = Vec::new();
        let mut raw = Vec::new();
        for &byte in bytes {
            if self.pending {
                self.pending = false;
                if byte == 29 {
                    raw.push(byte);
                } else if b"npowWNC|-z=,RxX!tmS+_HLKJ123456789dlsc?".contains(&byte) {
                    if !raw.is_empty() {
                        result.push(Input::Bytes(std::mem::take(&mut raw)));
                    }
                    result.push(Input::Command(byte));
                    if byte == b'd' {
                        break;
                    }
                } else {
                    raw.extend_from_slice(&[29, byte]);
                }
            } else if byte == 29 {
                if !raw.is_empty() {
                    result.push(Input::Bytes(std::mem::take(&mut raw)));
                }
                self.pending = true;
            } else {
                raw.push(byte);
            }
        }
        if !raw.is_empty() {
            result.push(Input::Bytes(raw));
        }
        result
    }
}
pub fn kitty_key(bytes: &[u8]) -> Option<(u32, u32, u32)> {
    let text = std::str::from_utf8(bytes)
        .ok()?
        .strip_prefix("\x1b[")?
        .strip_suffix('u')?;
    let mut fields = text.split(';');
    let key = fields.next()?.split(':').next()?.parse().ok()?;
    let mut modifiers = fields.next().unwrap_or("1").split(':');
    Some((
        key,
        modifiers.next()?.parse().ok()?,
        modifiers.next().unwrap_or("1").parse().ok()?,
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Text(String),
    Enter,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Clear,
}
#[derive(Default)]
pub struct Decoder {
    escape: Vec<u8>,
    text: Vec<u8>,
}
impl Decoder {
    pub fn pending_escape(&self) -> bool {
        !self.escape.is_empty()
    }
    pub fn reset(&mut self) {
        self.escape.clear();
        self.text.clear();
    }
    pub fn feed(&mut self, byte: u8) -> Option<Key> {
        if !self.escape.is_empty() {
            self.escape.push(byte);
            if self.escape.len() == 2 && [b'[', b'O'].contains(&byte) {
                return None;
            }
            if self.escape.len() > 32 {
                self.reset();
                return None;
            }
            if self.escape.len() > 2 && (0x40..=0x7e).contains(&byte) {
                let sequence = std::mem::take(&mut self.escape);
                return match byte {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    b'C' => Some(Key::Right),
                    b'D' => Some(Key::Left),
                    b'H' => Some(Key::Home),
                    b'F' => Some(Key::End),
                    b'~' => match sequence.as_slice() {
                        b"\x1b[3~" => Some(Key::Delete),
                        b"\x1b[5~" => Some(Key::PageUp),
                        b"\x1b[6~" => Some(Key::PageDown),
                        b"\x1b[1~" | b"\x1b[7~" => Some(Key::Home),
                        b"\x1b[4~" | b"\x1b[8~" => Some(Key::End),
                        _ => None,
                    },
                    _ => None,
                };
            }
            if self.escape.len() == 2 {
                self.reset();
            }
            return None;
        }
        match byte {
            27 => {
                self.text.clear();
                self.escape.push(byte);
                None
            }
            10 | 13 => Some(Key::Enter),
            8 | 127 => Some(Key::Backspace),
            16 => Some(Key::Up),
            14 => Some(Key::Down),
            1 => Some(Key::Home),
            5 => Some(Key::End),
            21 => Some(Key::Clear),
            0..=31 => None,
            _ => {
                self.text.push(byte);
                match std::str::from_utf8(&self.text) {
                    Ok(value) => {
                        let value = value.to_string();
                        self.text.clear();
                        Some(Key::Text(value))
                    }
                    Err(error) if error.error_len().is_none() && self.text.len() < 4 => None,
                    Err(_) => {
                        self.text.clear();
                        None
                    }
                }
            }
        }
    }
}

#[derive(Default)]
pub struct Editor {
    pub text: String,
    cursor: usize,
}
impl Editor {
    pub fn apply(&mut self, key: &Key) {
        let previous = self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index);
        let next = self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |value| self.cursor + value.len());
        match key {
            Key::Text(value) if self.text.len() + value.len() <= 4096 => {
                self.text.insert_str(self.cursor, value);
                self.cursor += value.len();
            }
            Key::Left => self.cursor = previous,
            Key::Right => self.cursor = next,
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = self.text.len(),
            Key::Backspace => {
                self.text.replace_range(previous..self.cursor, "");
                self.cursor = previous;
            }
            Key::Delete => self.text.replace_range(self.cursor..next, ""),
            Key::Clear => {
                self.text.clear();
                self.cursor = 0;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_input_keeps_nuls_and_fragmented_prefixes() {
        let mut prefix = Prefix::default();
        assert_eq!(
            prefix.feed(b"a\0\x1b[A\x1d"),
            vec![Input::Bytes(b"a\0\x1b[A".to_vec())]
        );
        let events = prefix.feed(b"n\x1d\x1d\x1dq");
        assert_eq!(events.first(), Some(&Input::Command(b'n')));
        let bytes = events
            .into_iter()
            .filter_map(|event| match event {
                Input::Bytes(bytes) => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(bytes, b"\x1d\x1dq");
    }
    #[test]
    fn editor_moves_across_graphemes() {
        let mut editor = Editor::default();
        editor.apply(&Key::Text("e\u{301}🦉x".into()));
        editor.apply(&Key::Left);
        editor.apply(&Key::Backspace);
        assert_eq!(editor.text, "e\u{301}x");
        editor.apply(&Key::Backspace);
        assert_eq!(editor.text, "x");
    }
    #[test]
    fn modal_arrows_can_arrive_in_separate_reads() {
        let mut decoder = Decoder::default();
        assert_eq!(decoder.feed(27), None);
        assert_eq!(decoder.feed(b'['), None);
        assert_eq!(decoder.feed(b'D'), Some(Key::Left));
    }
}
