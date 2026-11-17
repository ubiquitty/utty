// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    time::{Duration, Instant},
};
use utty::core::{Client, Frame, Image, Pane, Quad};

// These are outer-terminal reports, not an application terminal decoder.
// Everything else keeps its original bytes for Core's hosted application.
const PASTE_LIMIT: usize = 64 * 1024 * 1024;
pub const PROBE_ID: u32 = 2_147_480_001;
pub const PROBE: &[u8] =
    b"\x1b_Gi=2147480001,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[?1016$p\x1b[?u\x1b[c";
#[derive(Debug, PartialEq)]
pub enum Event {
    Bytes(Vec<u8>),
    Paste(Vec<u8>),
    Focus(bool),
    Pixels(u16, u16),
    Graphics(bool),
    Keyboard,
    PixelMouse(bool),
    Mouse {
        button: i32,
        x: i32,
        y: i32,
        release: bool,
    },
}
#[derive(Default)]
pub struct Reports {
    pending: Vec<u8>,
    since: Option<Instant>,
    paste: Option<Vec<u8>>,
    probing: bool,
}
impl Reports {
    pub fn new() -> Self {
        Self {
            probing: true,
            ..Self::default()
        }
    }
    pub fn timeout(&self) -> Option<i32> {
        self.since.map(|start| {
            Duration::from_millis(25)
                .saturating_sub(start.elapsed())
                .as_millis() as i32
        })
    }
    pub fn expire(&mut self) -> Vec<Event> {
        if self.timeout() == Some(0) && self.paste.is_none() {
            self.since = None;
            return vec![Event::Bytes(std::mem::take(&mut self.pending))];
        }
        Vec::new()
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        for &byte in bytes {
            self.pending.push(byte);
            if let Some(paste) = self.paste.as_mut() {
                if self.pending == b"\x1b[201~" {
                    events.push(Event::Paste(std::mem::take(paste)));
                    self.paste = None;
                    self.pending.clear();
                    self.since = None;
                } else if !b"\x1b[201~".starts_with(&self.pending) {
                    while !self.pending.is_empty() && !b"\x1b[201~".starts_with(&self.pending) {
                        let byte = self.pending.remove(0);
                        if paste.len() < PASTE_LIMIT {
                            paste.push(byte);
                        }
                    }
                }
                continue;
            }
            if self.pending[0] != 27 {
                if let Some(Event::Bytes(raw)) = events.last_mut()
                    && raw.first() != Some(&27)
                {
                    raw.append(&mut self.pending);
                } else {
                    events.push(Event::Bytes(std::mem::take(&mut self.pending)));
                }
                continue;
            }
            if self.pending.len() == 1 {
                self.since = Some(Instant::now());
                continue;
            }
            let complete = match self.pending[1] {
                b'[' => self.pending.len() > 2 && (0x40..=0x7e).contains(&byte),
                b'_' => self.pending.ends_with(b"\x1b\\"),
                _ => true,
            };
            if !complete && self.pending.len() < 4096 {
                continue;
            }
            self.since = None;
            let sequence = std::mem::take(&mut self.pending);
            if sequence == b"\x1b[200~" {
                self.paste = Some(Vec::new());
                continue;
            }
            let text = std::str::from_utf8(&sequence).unwrap_or("");
            let event = if sequence == b"\x1b[I" {
                Event::Focus(true)
            } else if sequence == b"\x1b[O" {
                Event::Focus(false)
            } else if let Some(report) = text
                .strip_prefix("\x1b[<")
                .and_then(|s| s.strip_suffix(['M', 'm']))
            {
                let fields = report
                    .split(';')
                    .map(str::parse::<i32>)
                    .collect::<Result<Vec<_>, _>>();
                match fields {
                    Ok(v) if v.len() == 3 && v[0] >= 0 && v[1] > 0 && v[2] > 0 => Event::Mouse {
                        button: v[0],
                        x: v[1] - 1,
                        y: v[2] - 1,
                        release: sequence.last() == Some(&b'm'),
                    },
                    _ => Event::Bytes(sequence),
                }
            } else if let Some(report) = text
                .strip_prefix("\x1b[6;")
                .and_then(|s| s.strip_suffix('t'))
            {
                let fields = report
                    .split(';')
                    .map(str::parse::<u16>)
                    .collect::<Result<Vec<_>, _>>();
                match fields {
                    Ok(v) if v.len() == 2 && v[0] > 0 && v[1] > 0 => Event::Pixels(v[1], v[0]),
                    _ => Event::Bytes(sequence),
                }
            } else if text.starts_with(&format!("\x1b_Gi={PROBE_ID};")) {
                Event::Graphics(text.ends_with(";OK\x1b\\"))
            } else if matches!(text, "\x1b[?1016;1$y" | "\x1b[?1016;2$y" | "\x1b[?1016;3$y") {
                Event::PixelMouse(true)
            } else if matches!(text, "\x1b[?1016;0$y" | "\x1b[?1016;4$y") {
                Event::PixelMouse(false)
            } else if self.probing && text.starts_with("\x1b[?") && text.ends_with('u') {
                Event::Keyboard
            } else if self.probing && text.starts_with("\x1b[?") && text.ends_with('c') {
                self.probing = false;
                continue;
            } else {
                Event::Bytes(sequence)
            };
            events.push(event);
        }
        events
    }
}

#[derive(Default)]
pub struct Presentation {
    pub graphics: bool,
    pub keyboard: bool,
    pub pixel_mouse: bool,
    pub pixels: (u16, u16),
    modes: String,
    images: BTreeMap<(u32, usize), CachedImage>,
    next_id: u32,
    decorated: BTreeSet<(u16, u16)>,
}
struct CachedImage {
    id: u32,
    stamp: u64,
    geometry: Vec<u32>,
}
impl Presentation {
    pub fn decorations(&mut self, panes: &[(Pane, Frame)], application: bool) -> Vec<u8> {
        use crossterm::{
            cursor::MoveTo,
            style::{
                Attribute, Attributes, Color, Print, SetAttribute, SetAttributes,
                SetBackgroundColor, SetForegroundColor, SetUnderlineColor,
            },
        };
        let mut bytes = Vec::new();
        let mut decorated = BTreeSet::new();
        if application {
            for (pane, frame) in panes {
                for row in 0..frame.rows.min(pane.rows.saturating_sub(pane.header)) {
                    for column in 0..frame.columns.min(pane.columns) {
                        let cell = &frame.cells
                            [usize::from(row) * usize::from(frame.columns) + usize::from(column)];
                        if cell.width == 0 {
                            continue;
                        }
                        let position = (pane.x + column, pane.y + pane.header + row);
                        let semantic = !cell.hyperlink.is_empty() || cell.underline_style > 1;
                        if semantic {
                            decorated.insert(position);
                        }
                        if !semantic && !self.decorated.contains(&position) {
                            continue;
                        }
                        if bytes.is_empty() {
                            bytes.extend_from_slice(b"\x1b7");
                        }
                        let rgb = |value: u32| Color::Rgb {
                            r: (value >> 16) as u8,
                            g: (value >> 8) as u8,
                            b: value as u8,
                        };
                        let mut attributes = Attributes::default();
                        for (flag, attribute) in [
                            (1, Attribute::Bold),
                            (4, Attribute::SlowBlink),
                            (16, Attribute::Italic),
                            (32, Attribute::CrossedOut),
                        ] {
                            if cell.attributes & flag != 0 {
                                attributes.set(attribute);
                            }
                        }
                        if cell.attributes & 2 != 0 {
                            attributes.set(match cell.underline_style {
                                2 => Attribute::DoubleUnderlined,
                                3 => Attribute::Undercurled,
                                4 => Attribute::Underdotted,
                                5 => Attribute::Underdashed,
                                _ => Attribute::Underlined,
                            });
                        }
                        crossterm::queue!(
                            bytes,
                            MoveTo(position.0, position.1),
                            SetAttribute(Attribute::Reset),
                            SetForegroundColor(rgb(cell.foreground)),
                            SetBackgroundColor(rgb(cell.background)),
                            SetUnderlineColor(rgb(cell.underline)),
                            SetAttributes(attributes)
                        )
                        .expect("write to Vec");
                        if !cell.hyperlink.chars().any(char::is_control) {
                            write!(bytes, "\x1b]8;;{}\x1b\\", cell.hyperlink)
                                .expect("write to Vec");
                        }
                        crossterm::queue!(bytes, Print(&cell.text)).expect("write to Vec");
                        bytes.extend_from_slice(b"\x1b]8;;\x1b\\");
                    }
                }
            }
        }
        self.decorated = decorated;
        if !bytes.is_empty() {
            bytes.extend_from_slice(b"\x1b[0m\x1b8");
        }
        bytes
    }
    pub fn cleanup(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for (_, image) in std::mem::take(&mut self.images) {
            bytes.extend_from_slice(format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", image.id).as_bytes());
        }
        bytes
    }
    pub fn new() -> Self {
        Self {
            pixels: (8, 16),
            next_id: PROBE_ID + 1,
            ..Self::default()
        }
    }
    pub fn modes(&mut self, active: Option<&Frame>, application: bool) -> Vec<u8> {
        let mode = if application { active } else { None };
        let cursor = mode.map_or(0, |f| f.cursor_style);
        let flags = mode.map_or(0, |f| f.keyboard_flags);
        let mut value = format!(
            "\x1b[?1{}\x1b{}\x1b[{cursor} q",
            if mode.is_some_and(|f| f.application_cursor) {
                'h'
            } else {
                'l'
            },
            if mode.is_some_and(|f| f.application_keypad) {
                '='
            } else {
                '>'
            }
        );
        if self.pixel_mouse {
            value.push_str("\x1b[?1016h");
        }
        if self.keyboard {
            value.push_str(&format!("\x1b[={flags}u"));
        }
        if value == self.modes {
            return Vec::new();
        }
        self.modes = value.clone();
        value.into_bytes()
    }
    pub fn images(
        &mut self,
        client: &mut Client,
        panes: &[(Pane, Frame)],
        application: bool,
    ) -> Vec<u8> {
        let mut output = Vec::new();
        let mut present = BTreeSet::new();
        let mut sources = BTreeMap::new();
        if self.graphics && application {
            for (pane, frame) in panes {
                for (index, quad) in frame.quads.iter().enumerate() {
                    let key = (pane.session, index);
                    let previous = self.images.get(&key);
                    let Some(image) = client.image(pane.session, quad.image_id, false) else {
                        continue;
                    };
                    let geometry = vec![
                        quad.image_id,
                        quad.x.to_bits(),
                        quad.y.to_bits(),
                        quad.width.to_bits(),
                        quad.height.to_bits(),
                        quad.u0.to_bits(),
                        quad.v0.to_bits(),
                        quad.u1.to_bits(),
                        quad.v1.to_bits(),
                        quad.z as u32,
                        quad.filter.into(),
                        pane.x.into(),
                        pane.y.into(),
                        pane.header.into(),
                    ];
                    present.insert(key);
                    if previous.is_some_and(|p| p.stamp == image.stamp && p.geometry == geometry) {
                        continue;
                    }
                    if let Some(old) = self.images.remove(&key) {
                        output.extend_from_slice(
                            format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", old.id).as_bytes(),
                        );
                    }
                    // Placeholder tiles share one source copy and conversion per frame.
                    let source =
                        sources
                            .entry((pane.session, quad.image_id))
                            .or_insert_with(|| {
                                rgba(
                                    client
                                        .image(pane.session, quad.image_id, true)
                                        .expect("image read on owner thread"),
                                )
                            });
                    let id = self.next_id;
                    self.next_id = self.next_id.wrapping_add(1).max(PROBE_ID + 1);
                    let raster = rasterize(source, quad);
                    let payload = STANDARD.encode(raster.as_raw());
                    let chunks = payload.as_bytes().chunks(4096);
                    let total = chunks.len();
                    for (part, chunk) in chunks.enumerate() {
                        let header = if part == 0 {
                            format!(
                                "\x1b_Ga=t,t=d,f={},s={},v={},i={id},q=2,m={};",
                                32,
                                raster.width(),
                                raster.height(),
                                u8::from(part + 1 < total)
                            )
                        } else {
                            format!("\x1b_Gq=2,m={};", u8::from(part + 1 < total))
                        };
                        output.extend_from_slice(header.as_bytes());
                        output.extend_from_slice(chunk);
                        output.extend_from_slice(b"\x1b\\");
                    }
                    let (x, y) = (
                        quad.x.max(0.0).round() as u32,
                        quad.y.max(0.0).round() as u32,
                    );
                    output.extend_from_slice(
                        format!(
                            "\x1b7\x1b[{};{}H\x1b_Ga=p,i={id},q=2,C=1,X={},Y={},z={}\x1b\\\x1b8",
                            u32::from(pane.y + pane.header) + y / u32::from(self.pixels.1) + 1,
                            u32::from(pane.x) + x / u32::from(self.pixels.0) + 1,
                            x % u32::from(self.pixels.0),
                            y % u32::from(self.pixels.1),
                            quad.z
                        )
                        .as_bytes(),
                    );
                    self.images.insert(
                        key,
                        CachedImage {
                            id,
                            stamp: image.stamp,
                            geometry,
                        },
                    );
                }
            }
        }
        self.images.retain(|key, image| {
            if present.contains(key) {
                true
            } else {
                output.extend_from_slice(
                    format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", image.id).as_bytes(),
                );
                false
            }
        });
        output
    }
}

// Core supplies the crop, destination and filter. The outer terminal receives
// pixels at that exact size, avoiding cell rounding for clipped placements.
fn rgba(image: Image) -> image::RgbaImage {
    if image.format == 32 {
        image::RgbaImage::from_raw(image.width, image.height, image.pixels).unwrap()
    } else {
        image::DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(image.width, image.height, image.pixels).unwrap(),
        )
        .into_rgba8()
    }
}
fn rasterize(image: &image::RgbaImage, quad: &Quad) -> image::RgbaImage {
    let x = ((quad.u0 * image.width() as f32).round() as u32).min(image.width() - 1);
    let y = ((quad.v0 * image.height() as f32).round() as u32).min(image.height() - 1);
    let right = ((quad.u1 * image.width() as f32).round() as u32).clamp(x + 1, image.width());
    let bottom = ((quad.v1 * image.height() as f32).round() as u32).clamp(y + 1, image.height());
    let crop = image::imageops::crop_imm(image, x, y, right - x, bottom - y);
    image::imageops::resize(
        &*crop,
        quad.width.round().max(1.0) as u32,
        quad.height.round().max(1.0) as u32,
        if quad.filter == 0 {
            image::imageops::FilterType::Nearest
        } else {
            image::imageops::FilterType::Triangle
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reports_are_fragmented_and_unknown_input_stays_exact() {
        let mut parser = Reports::new();
        assert!(parser.feed(b"\x1b[<0;12;").is_empty());
        assert_eq!(
            parser.feed(b"8M"),
            vec![Event::Mouse {
                button: 0,
                x: 11,
                y: 7,
                release: false
            }]
        );
        assert_eq!(parser.feed(b"\x1b[6;24;12t"), vec![Event::Pixels(12, 24)]);
        assert_eq!(
            parser.feed(b"\x1b[97;5:2u"),
            vec![Event::Bytes(b"\x1b[97;5:2u".to_vec())]
        );
        assert_eq!(parser.feed(b"\x1b[?31u"), vec![Event::Keyboard]);
        assert_eq!(
            parser.feed(b"\x1b[93;5ud"),
            vec![
                Event::Bytes(b"\x1b[93;5u".to_vec()),
                Event::Bytes(b"d".to_vec())
            ]
        );
        assert_eq!(
            parser.feed(b"\x1b[?1016;0$y"),
            vec![Event::PixelMouse(false)]
        );
        assert_eq!(
            parser.feed(b"\x1b_Gi=2147480001;OK\x1b\\"),
            vec![Event::Graphics(true)]
        );
    }
    #[test]
    fn pasted_prefix_and_reports_remain_literal() {
        let mut parser = Reports::new();
        assert!(parser.feed(b"\x1b[200~abc\x1dd\x1b[I\x1b[20").is_empty());
        assert_eq!(
            parser.feed(b"1~"),
            vec![Event::Paste(b"abc\x1dd\x1b[I".to_vec())]
        );
        assert_eq!(
            parser.feed(b"\x1b[200~\x1b\x1b[201~"),
            vec![Event::Paste(vec![27])]
        );
    }
    #[test]
    fn image_crop_preserves_exact_pixel_extent_and_source() {
        let image = Image {
            pixels: vec![255, 0, 0, 0, 0, 255],
            stamp: 0,
            width: 2,
            height: 1,
            format: 24,
        };
        let quad = Quad {
            image_id: 1,
            x: 0.0,
            y: 0.0,
            width: 7.0,
            height: 3.0,
            u0: 0.5,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
            z: 0,
            filter: 0,
        };
        let raster = rasterize(&rgba(image), &quad);
        assert_eq!(raster.dimensions(), (7, 3));
        assert!(raster.pixels().all(|pixel| pixel.0 == [0, 0, 255, 255]));
    }
}
