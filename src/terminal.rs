// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
use crate::ui::clean;
use base64::{Engine, engine::general_purpose::STANDARD};
use crossterm::terminal;
use std::{
    cell::RefCell,
    io::{self, Read, Write},
    mem::MaybeUninit,
    os::{fd::AsRawFd, unix::net::UnixStream},
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
use utty::{core::Snapshot, options::Profile};

#[derive(Clone, Default)]
pub struct QueueWriter(pub Rc<RefCell<Vec<u8>>>);
impl Write for QueueWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut queue = self.0.borrow_mut();
        queue.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl QueueWriter {
    pub fn take(&self, profile: Profile) -> Vec<u8> {
        let bytes = std::mem::take(&mut *self.0.borrow_mut());
        if profile == Profile::Modern {
            return bytes;
        }
        legacy_output(&bytes, profile)
    }
}
pub fn legacy_output(bytes: &[u8], profile: Profile) -> Vec<u8> {
    let mut output = Vec::new();
    for cluster in String::from_utf8_lossy(bytes).graphemes(true) {
        if cluster.is_ascii() {
            output.extend_from_slice(cluster.as_bytes());
            continue;
        }
        let dec = match cluster {
            "─" => Some(b'q'),
            "│" => Some(b'x'),
            "┌" => Some(b'l'),
            "┐" => Some(b'k'),
            "└" => Some(b'm'),
            "┘" => Some(b'j'),
            "┼" => Some(b'n'),
            "├" => Some(b't'),
            "┤" => Some(b'u'),
            "┴" => Some(b'v'),
            "┬" => Some(b'w'),
            _ => None,
        };
        if profile == Profile::Dec
            && let Some(character) = dec
        {
            output.extend_from_slice(b"\x1b(0");
            output.push(character);
            output.extend_from_slice(b"\x1b(B");
            continue;
        }
        output.push(match cluster {
            "─" => b'-',
            "│" => b'|',
            "┌" | "┐" | "└" | "┘" | "┼" | "├" | "┤" | "┴" | "┬" => b'+',
            _ => b'?',
        });
        if cluster.width() > 1 {
            output.extend(std::iter::repeat_n(b' ', cluster.width() - 1));
        }
    }
    output
}

#[derive(Default)]
pub struct Output {
    bytes: Vec<u8>,
    offset: usize,
}
impl Output {
    pub fn empty(&self) -> bool {
        self.offset == self.bytes.len()
    }
    pub fn queue(&mut self, bytes: Vec<u8>) {
        assert!(self.empty());
        self.bytes = bytes;
        self.offset = 0;
    }
    pub fn flush(&mut self) -> io::Result<()> {
        self.flush_with(|bytes| {
            let written =
                unsafe { libc::write(libc::STDOUT_FILENO, bytes.as_ptr().cast(), bytes.len()) };
            if written < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(written as usize)
            }
        })
    }
    fn flush_with(&mut self, mut write: impl FnMut(&[u8]) -> io::Result<usize>) -> io::Result<()> {
        while !self.empty() {
            match write(&self.bytes[self.offset..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => self.offset += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

pub struct Tty {
    saved: libc::termios,
    input_flags: i32,
    output_flags: i32,
    profile: Profile,
    pub stopped: Arc<AtomicBool>,
    pub resized: Arc<AtomicBool>,
    wake: UnixStream,
    signals: Vec<signal_hook::SigId>,
}
impl Tty {
    pub fn open(profile: Profile) -> io::Result<Self> {
        if unsafe { libc::isatty(0) == 0 || libc::isatty(1) == 0 } {
            return Err(io::Error::other(
                "stdin and stdout must be terminal devices",
            ));
        }
        let mut saved = MaybeUninit::uninit();
        if unsafe { libc::tcgetattr(0, saved.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let saved = unsafe { saved.assume_init() };
        let input_flags = unsafe { libc::fcntl(0, libc::F_GETFL) };
        let output_flags = unsafe { libc::fcntl(1, libc::F_GETFL) };
        if input_flags < 0 || output_flags < 0 {
            return Err(io::Error::last_os_error());
        }
        // A signal that lands between the loop's flag checks and poll() must
        // still wake poll(), so each handler also writes to a self-pipe.
        let (wake, wake_writer) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        wake_writer.set_nonblocking(true)?;
        let mut tty = Self {
            saved,
            input_flags,
            output_flags,
            profile,
            stopped: Arc::new(AtomicBool::new(false)),
            resized: Arc::new(AtomicBool::new(false)),
            wake,
            signals: Vec::new(),
        };
        terminal::enable_raw_mode()?;
        if profile != Profile::Modern {
            let mut raw = saved;
            unsafe { libc::cfmakeraw(&mut raw) };
            raw.c_iflag |= saved.c_iflag & (libc::IXON | libc::IXOFF);
            if unsafe { libc::tcsetattr(0, libc::TCSANOW, &raw) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        if unsafe {
            libc::fcntl(0, libc::F_SETFL, input_flags | libc::O_NONBLOCK) < 0
                || libc::fcntl(1, libc::F_SETFL, output_flags | libc::O_NONBLOCK) < 0
        } {
            return Err(io::Error::last_os_error());
        }
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            tty.signals
                .push(signal_hook::flag::register(signal, tty.stopped.clone())?);
            tty.signals.push(signal_hook::low_level::pipe::register(
                signal,
                wake_writer.try_clone()?,
            )?);
        }
        tty.signals.push(signal_hook::flag::register(
            libc::SIGWINCH,
            tty.resized.clone(),
        )?);
        tty.signals.push(signal_hook::low_level::pipe::register(
            libc::SIGWINCH,
            wake_writer,
        )?);
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
            libc::signal(libc::SIGTSTP, libc::SIG_IGN);
        }
        Ok(tty)
    }
    pub fn wake_descriptor(&self) -> i32 {
        self.wake.as_raw_fd()
    }
    pub fn drain_wake(&mut self) {
        let mut sink = [0u8; 64];
        while matches!((&self.wake).read(&mut sink), Ok(count) if count > 0) {}
    }
    pub fn setup(&self) -> Vec<u8> {
        if self.profile == Profile::Modern {
            let mut setup = b"\x1b[22;0t\x1b[?1049h\x1b[>0u\x1b[?7l\x1b[?25l\x1b(B\x0f\x1b[?1003h\x1b[?1006h\x1b[?1004h\x1b[?2004h".to_vec();
            setup.extend_from_slice(crate::modern::PROBE);
            setup
        } else {
            b"\x1b[?7l\x1b[?25l\x1b(B\x0f".to_vec()
        }
    }
}
impl Drop for Tty {
    fn drop(&mut self) {
        let mut restore = b"\x18\x1b[0m\x1b(B\x0f\x1b[?1l\x1b>\x1b[?7h\x1b[?25h".to_vec();
        if self.profile == Profile::Modern {
            restore.extend_from_slice(
                b"\x1b[?2026l\x1b[<u\x1b[?1003l\x1b[?1016l\x1b[?1006l\x1b[?1004l\x1b[?2004l\x1b[0 q",
            );
            restore.extend_from_slice(
                b"\x1b]7501;id=utty:state=clear\x1b\\\x1b]9;4;0\x1b\\\x1b[?1049l\x1b[23;0t",
            );
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut offset = 0;
        while offset < restore.len() && Instant::now() < deadline {
            let count = unsafe {
                libc::write(1, restore[offset..].as_ptr().cast(), restore.len() - offset)
            };
            if count > 0 {
                offset += count as usize;
            } else {
                let mut fd = libc::pollfd {
                    fd: 1,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                unsafe { libc::poll(&mut fd, 1, 10) };
            }
        }
        let _ = terminal::disable_raw_mode();
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.saved);
            libc::fcntl(0, libc::F_SETFL, self.input_flags);
            libc::fcntl(1, libc::F_SETFL, self.output_flags);
        }
        for signal in self.signals.drain(..) {
            signal_hook::low_level::unregister(signal);
        }
    }
}
fn uri(text: &str, path: bool) -> String {
    let mut result = String::new();
    for byte in text.bytes().take(2048) {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) || (path && byte == b'/') {
            result.push(char::from(byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}
pub fn presentation(snapshot: &Snapshot) -> String {
    let tab = snapshot
        .tabs
        .iter()
        .find(|tab| tab.active)
        .map_or("", |tab| tab.title.as_str());
    let title = clean(
        &match (snapshot.name.is_empty(), tab.is_empty()) {
            (false, false) => format!("{} — {tab}", snapshot.name),
            (false, true) => snapshot.name.clone(),
            (true, _) => tab.to_string(),
        },
        false,
    );
    let state = match snapshot.kind {
        7 => "error",
        5 | 6 => "blocked",
        3 => "working",
        4 => "done",
        _ => "idle",
    };
    let app = if !snapshot.app.is_empty()
        && snapshot
            .app
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        &snapshot.app
    } else {
        "utty"
    };
    let mut value = format!(
        "\x1b]0;{title}\x1b\\\x1b]7501;id=utty:app={app}:state={state}:title={}",
        STANDARD.encode(clean(&snapshot.status_title, false).as_bytes())
    );
    if !snapshot.block.is_empty() && state == "blocked" {
        value.push_str(&format!(":kind={}", snapshot.block));
    }
    if !snapshot.message.is_empty() {
        value.push_str(&format!(
            ":msg={}",
            STANDARD.encode(clean(&snapshot.message, false).as_bytes())
        ));
    }
    if snapshot.progress >= 0 && ["working", "blocked"].contains(&state) {
        value.push_str(&format!(":progress={}", snapshot.progress));
    }
    value.push_str("\x1b\\");
    let mut progress_state = snapshot.progress_state;
    let mut progress_value = snapshot.progress_value;
    if progress_state == 0 && snapshot.progress >= 0 {
        progress_state = if state == "blocked" { 4 } else { 1 };
        progress_value = snapshot.progress.min(100) as u8;
    }
    if progress_state == 0 && state == "working" {
        progress_state = 3;
    }
    value.push_str(&format!("\x1b]9;4;{progress_state};{progress_value}\x1b\\"));
    if !snapshot.cwd.is_empty() {
        value.push_str(&format!(
            "\x1b]7;file://{}{}\x1b\\",
            uri(&snapshot.host, false),
            uri(&snapshot.cwd, true)
        ));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_drains_short_writes_and_resumes_after_backpressure() {
        let expected = vec![b'x'; 128 * 1024];
        let mut output = Output::default();
        output.queue(expected.clone());
        let mut received = Vec::new();
        output
            .flush_with(|bytes| {
                if received.len() == 32768 {
                    return Err(io::ErrorKind::WouldBlock.into());
                }
                let count = bytes.len().min(4096);
                received.extend_from_slice(&bytes[..count]);
                Ok(count)
            })
            .unwrap();
        assert_eq!(received.len(), 32768);
        assert!(!output.empty());
        let mut interrupted = false;
        output
            .flush_with(|bytes| {
                if !interrupted {
                    interrupted = true;
                    return Err(io::ErrorKind::Interrupted.into());
                }
                received.extend_from_slice(bytes);
                Ok(bytes.len())
            })
            .unwrap();
        assert!(output.empty());
        assert_eq!(received, expected);
    }
    #[test]
    fn legacy_output_preserves_cell_width_and_uses_only_seven_bits() {
        let dec = legacy_output("A─界🦉Z".as_bytes(), Profile::Dec);
        assert!(dec.is_ascii());
        assert!(dec.windows(7).any(|bytes| bytes == b"\x1b(0q\x1b(B"));
        assert_eq!(legacy_output("A─界Z".as_bytes(), Profile::Ascii), b"A-? Z");
    }
}
