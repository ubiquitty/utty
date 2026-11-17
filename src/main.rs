// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
mod bar;
mod input;
mod layout;
mod modern;
mod picker;
mod terminal;
mod ui;

use ratatui::{Terminal, TerminalOptions, Viewport, backend::CrosstermBackend, layout::Rect};
use std::{io, sync::atomic::Ordering};
use utty::{
    connect,
    core::{self, Client},
    options::{Command, HELP, Options, Profile},
};

/// How long `ls` and `attach` wait for the host's listing.
const LISTING_TIMEOUT_MS: i32 = 15_000;

fn geometry(options: &Options) -> io::Result<(u16, u16)> {
    let (columns, rows) = options
        .geometry
        .unwrap_or_else(|| crossterm::terminal::size().unwrap_or((80, 24)));
    let (columns, rows) = (columns.min(512), rows.min(256));
    if columns < 20 || rows < 4 {
        return Err(io::Error::other("Terminal must be at least 20x4"));
    }
    Ok((columns, rows))
}
fn run(options: &Options) -> Result<(), Box<dyn std::error::Error>> {
    // Application colors are terminal state, including when the launcher's
    // NO_COLOR preference suppresses decorative output from ordinary CLIs.
    crossterm::style::force_color_output(options.profile == Profile::Modern);
    let (mut columns, mut rows) = geometry(options)?;
    let mut client = Client::new(options, columns, rows)?;
    let final_snapshot;
    {
        let mut tty = terminal::Tty::open(options.profile)?;
        let writer = terminal::QueueWriter::default();
        let backend = CrosstermBackend::new(writer.clone());
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, columns, rows)),
            },
        )?;
        let mut output = terminal::Output::default();
        output.queue(tty.setup());
        let mut reports = modern::Reports::new();
        let mut presentation = modern::Presentation::new();
        let mut paste_wrapped = false;
        let mut application = true;
        let mut query_pixels = false;
        let mut ui = ui::Ui::new();
        ui.set_size(columns, rows);
        let mut snapshot = client.snapshot()?;
        let mut last_presentation = String::new();
        while !tty.stopped.load(Ordering::Relaxed) && !ui.quit {
            if !client.tick() {
                break;
            }
            if tty.resized.swap(false, Ordering::Relaxed) {
                query_pixels = options.profile == Profile::Modern;
                ui.dirty = true;
                let size = geometry(options)?;
                if size != (columns, rows) {
                    (columns, rows) = size;
                    client.resize(columns, rows);
                    ui.set_size(columns, rows);
                    terminal.resize(Rect::new(0, 0, columns, rows))?;
                    ui.dirty = true;
                }
            }
            ui.expire_escape(&mut client, &snapshot);
            for event in reports.expire() {
                ui.outer_event(
                    event,
                    &mut client,
                    &snapshot,
                    paste_wrapped,
                    presentation.pixels,
                    presentation.pixel_mouse,
                );
            }
            if output.empty() && (client.dirty() || ui.dirty) {
                snapshot = client.snapshot()?;
                layout::place(&mut snapshot, columns, rows);
                client.configure_views(&snapshot.panes, ui.application());
                let panes = snapshot
                    .panes
                    .iter()
                    .cloned()
                    .map(|pane| {
                        let data = client.frame(pane.session);
                        (pane, data)
                    })
                    .collect::<Vec<_>>();
                if ui.redraw || application != ui.application() {
                    // Redraw through the fixed viewport: Terminal::clear queries
                    // the physical cursor and would take ownership of raw input.
                    terminal.resize(Rect::new(0, 0, columns, rows))?;
                    ui.redraw = false;
                }
                application = ui.application();
                terminal.draw(|frame| ui.render(frame, &snapshot, &panes, options))?;
                let mut bytes = writer.take(options.profile);
                if query_pixels {
                    bytes.extend_from_slice(b"\x1b[16t");
                    query_pixels = false;
                }
                let active = panes
                    .iter()
                    .find(|(pane, _)| pane.focused)
                    .map(|(_, frame)| frame);
                paste_wrapped = active.is_some_and(|f| f.bracketed_paste);
                bytes.extend(presentation.modes(active, ui.application()));
                if options.profile == Profile::Modern {
                    bytes.extend(presentation.decorations(&panes, ui.application()));
                    bytes.extend(presentation.images(&mut client, &panes, ui.application()));
                    bytes.splice(0..0, b"\x1b[?2026h".iter().copied());
                    let presentation = terminal::presentation(&snapshot);
                    if presentation != last_presentation {
                        bytes.extend_from_slice(presentation.as_bytes());
                        last_presentation = presentation;
                    }
                    bytes.extend_from_slice(b"\x1b[?2026l");
                }
                output.queue(bytes);
                ui.dirty = false;
            }
            output.flush()?;
            let mut timeout = client.wait_timeout();
            if let Some(report) = reports.timeout()
                && (timeout < 0 || report < timeout)
            {
                timeout = report;
            }
            if let Some(escape) = ui.escape_timeout()
                && (timeout < 0 || escape < timeout)
            {
                timeout = escape;
            }
            if output.empty() && (client.dirty() || ui.dirty) {
                timeout = 0;
            }
            let mut descriptors = [
                libc::pollfd {
                    fd: 0,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if output.empty() { -1 } else { 1 },
                    events: libc::POLLOUT,
                    revents: 0,
                },
                libc::pollfd {
                    fd: client.wait_descriptor(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: tty.wake_descriptor(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let ready =
                unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, timeout) };
            if ready < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return Err(io::Error::last_os_error().into());
            }
            if descriptors[3].revents & libc::POLLIN != 0 {
                tty.drain_wake();
            }
            if descriptors[0].revents & libc::POLLIN != 0 {
                let mut bytes = [0u8; 4096];
                let count = unsafe { libc::read(0, bytes.as_mut_ptr().cast(), bytes.len()) };
                if count > 0 {
                    if options.profile == Profile::Modern {
                        for event in reports.feed(&bytes[..count as usize]) {
                            match event {
                                modern::Event::Graphics(supported) => {
                                    presentation.graphics = supported;
                                    ui.dirty = true;
                                }
                                modern::Event::Keyboard => {
                                    presentation.keyboard = true;
                                    ui.dirty = true;
                                }
                                modern::Event::PixelMouse(supported) => {
                                    presentation.pixel_mouse = supported;
                                    ui.dirty = true;
                                }
                                modern::Event::Pixels(width, height) => {
                                    presentation.pixels = (width, height);
                                    client.set_cell_pixels((width, height));
                                    ui.dirty = true;
                                }
                                event => ui.outer_event(
                                    event,
                                    &mut client,
                                    &snapshot,
                                    paste_wrapped,
                                    presentation.pixels,
                                    presentation.pixel_mouse,
                                ),
                            }
                        }
                    } else {
                        ui.feed(&bytes[..count as usize], &mut client, &snapshot);
                    }
                } else if count == 0 {
                    break;
                } else {
                    let error = io::Error::last_os_error();
                    if ![io::ErrorKind::WouldBlock, io::ErrorKind::Interrupted]
                        .contains(&error.kind())
                    {
                        return Err(error.into());
                    }
                }
            }
            if descriptors[0].revents & (libc::POLLERR | libc::POLLNVAL | libc::POLLHUP) != 0 {
                break;
            }
        }
        while !output.empty() {
            output.flush()?;
            if !output.empty() {
                let mut descriptor = libc::pollfd {
                    fd: 1,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                unsafe {
                    libc::poll(&mut descriptor, 1, 10);
                }
            }
        }
        output.queue(presentation.cleanup());
        output.flush()?;
        final_snapshot = client.snapshot()?;
    }
    if !final_snapshot.error.is_empty() {
        return Err(final_snapshot.error.into());
    }
    if final_snapshot.ended {
        eprintln!("Session {} ended.", final_snapshot.session);
    } else if final_snapshot.session != 0 {
        let program = std::env::args().next().unwrap_or_else(|| "utty".into());
        eprintln!(
            "Session {} detached. Reattach with: {}",
            final_snapshot.session,
            connect::reattach_command(
                &program,
                options,
                &final_snapshot.name,
                final_snapshot.session
            )
        );
    }
    Ok(())
}
/// Lists the host's workspaces, starting its daemon as connecting would:
/// recorded workspaces outlive the daemon and are listed only by a running
/// one. Reports whether this call started it.
fn listing(options: &Options) -> Result<(core::Directory, bool), String> {
    let running = connect::daemon_running(options);
    if running == Some(false) && !options.socket.is_empty() {
        return Err(format!(
            "no host daemon is running at --socket {}",
            options.socket
        ));
    }
    let directory = core::directory(options, LISTING_TIMEOUT_MS)?;
    Ok((directory, running == Some(false)))
}
fn list(options: &Options) -> Result<(), String> {
    let (directory, started) = listing(options)?;
    if started {
        core::retire_idle_daemon(options, &directory.socket);
    }
    if directory.workspaces.is_empty() && directory.instances.is_empty() {
        println!("No workspaces.");
    } else {
        print!("{}", connect::table(&directory));
    }
    Ok(())
}
fn attach(options: &mut Options, requested: Option<&str>) -> Result<(), String> {
    let (directory, started) = listing(options)?;
    match connect::choose(&directory, requested) {
        Ok(id) => {
            // Pinned to the daemon that listed it, which also lets the host
            // recreate a workspace nothing runs in.
            options.workspace = id;
            options.daemon_instance = directory.daemon_instance;
        }
        Err(error) => {
            if started {
                core::retire_idle_daemon(options, &directory.socket);
            }
            return Err(error);
        }
    }
    Ok(())
}
fn main() {
    let mut options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    if options.help {
        println!("{HELP}");
        return;
    }
    if let Err(error) = options.resolve_host() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    let prepared = match options.command.clone() {
        Command::Run => Ok(()),
        Command::List => {
            if let Err(error) = list(&options) {
                eprintln!("{error}");
                std::process::exit(1);
            }
            return;
        }
        Command::Attach(requested) => attach(&mut options, requested.as_deref()),
    };
    if let Err(error) = prepared {
        eprintln!("{error}");
        std::process::exit(1);
    }
    if let Err(error) = run(&options) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
