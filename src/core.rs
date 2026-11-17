// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
use crate::options::Options;
use serde::Deserialize;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
    slice,
};

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Tab {
    pub id: u32,
    pub focused_pane: u32,
    pub pane_ids: Vec<u32>,
    pub title: String,
    pub kind: u8,
    pub active: bool,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Pane {
    pub id: u32,
    pub session: u32,
    #[serde(skip)]
    pub x: u16,
    #[serde(skip)]
    pub y: u16,
    #[serde(skip)]
    pub columns: u16,
    #[serde(skip)]
    pub rows: u16,
    #[serde(skip)]
    pub header: u16,
    pub focused: bool,
    pub title: String,
    pub kind: u8,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Split {
    pub horizontal: bool,
    pub ratio: f64,
    pub children: Vec<Split>,
}
/// One row of the in-session workspace picker.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DirectoryRow {
    /// 0 workspace, 1 window, 2 tab, 3 pane.
    pub kind: u8,
    /// Attach target: a live row's session, or zero with `workspace` set.
    pub session: u32,
    pub workspace: u32,
    /// The workspace the row belongs to.
    pub owner: u32,
    /// The owning workspace's name, empty when unnamed.
    pub name: String,
    pub title: String,
    pub detail: String,
    pub status: u8,
    pub restorable: bool,
    pub people: u32,
    pub panes: usize,
    pub active_ms: u64,
}
/// The host's workspaces, listed without attaching.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Directory {
    pub error: String,
    /// The socket leaf of the daemon that answered.
    pub socket: String,
    /// The process identity of the daemon that answered; attaching by id with
    /// it restores a workspace nothing runs in.
    pub daemon_instance: u64,
    /// The host's wall clock, which every instant here is measured against.
    pub now_ms: u64,
    pub workspaces: Vec<ListedWorkspace>,
    /// Other live daemons on the host, each with its own workspaces.
    pub instances: Vec<HostInstance>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListedWorkspace {
    pub id: u32,
    /// The name `--workspace` reaches it by; empty when unnamed.
    pub name: String,
    pub title: String,
    /// Nothing runs in it; attaching recreates it.
    pub restorable: bool,
    pub windows: usize,
    pub tabs: usize,
    pub panes: usize,
    pub people: u32,
    pub agents: u32,
    pub last_input_ms: u64,
    pub last_output_ms: u64,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct HostInstance {
    pub socket: String,
    pub version: String,
    pub started_s: u64,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Snapshot {
    pub surface_id: u32,
    pub tab_id: u32,
    pub pane_id: u32,
    pub surfaces: Vec<u32>,
    pub zoomed: bool,
    pub split: Split,
    pub listing: bool,
    pub session: u32,
    pub socket: String,
    pub error: String,
    pub notice: String,
    pub ended: bool,
    pub state: String,
    pub name: String,
    pub tabs: Vec<Tab>,
    pub panes: Vec<Pane>,
    pub directory: Vec<DirectoryRow>,
    /// The attached workspace's id; zero before its structure arrives.
    pub workspace_id: u32,
    /// The host's wall clock when the directory was listed.
    pub now_ms: u64,
    pub kind: u8,
    pub detail: String,
    pub host: String,
    pub cwd: String,
    pub message: String,
    pub progress: i32,
    pub block: String,
    pub app: String,
    pub status_title: String,
    pub progress_state: u8,
    pub progress_value: u8,
}
#[repr(C)]
struct NativeCell {
    text: *const c_char,
    length: usize,
    foreground: u32,
    background: u32,
    underline: u32,
    attributes: u8,
    width: u8,
    underline_style: u8,
    hyperlink: *const c_char,
    hyperlink_length: usize,
}
#[repr(C)]
struct NativeIntent {
    verb: *const c_char,
    surface_id: u32,
    tab_id: u32,
    pane_id: u32,
    target_tab_id: u32,
    target_pane_id: u32,
    direction: u8,
    orientation: u8,
    side: u8,
    ratio: f64,
    title: *const u8,
    title_length: usize,
}
#[repr(C)]
struct NativeFrame {
    cells: *const NativeCell,
    count: usize,
    columns: c_int,
    rows: c_int,
    cursor_column: c_int,
    cursor_row: c_int,
    cursor_visible: u8,
    application_cursor: u8,
    application_keypad: u8,
    bracketed_paste: u8,
    cursor_style: u8,
    keyboard_flags: u16,
    quads: *const Quad,
    quad_count: usize,
}
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Quad {
    pub image_id: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub z: i32,
    pub filter: u8,
}
#[repr(C)]
struct NativeImage {
    pixels: *const u8,
    length: usize,
    stamp: u64,
    width: u32,
    height: u32,
    format: u32,
}
pub struct Image {
    pub pixels: Vec<u8>,
    pub stamp: u64,
    pub width: u32,
    pub height: u32,
    pub format: u32,
}
unsafe extern "C" {
    fn ubq_pixels(
        owner: *mut c_void,
        session: u32,
        columns: c_int,
        rows: c_int,
        width: c_int,
        height: c_int,
    );
    fn ubq_mouse(
        owner: *mut c_void,
        session: u32,
        kind: c_int,
        button: c_int,
        column: c_int,
        row: c_int,
        modifiers: c_int,
        pixel_x: c_int,
        pixel_y: c_int,
    );
    fn ubq_image(owner: *mut c_void, session: u32, image: u32) -> NativeImage;
    fn ubq_open(
        host: *const c_char,
        ssh: *const c_char,
        socket: *const c_char,
        session: u32,
        workspace: u32,
        daemon_instance: u64,
        name: *const c_char,
        columns: c_int,
        rows: c_int,
        graphics: c_int,
    ) -> *mut c_void;
    fn ubq_close(owner: *mut c_void);
    fn ubq_tick(owner: *mut c_void) -> c_int;
    fn ubq_dirty(owner: *const c_void) -> c_int;
    fn ubq_wait_descriptor(owner: *const c_void) -> c_int;
    fn ubq_wait_timeout(owner: *const c_void) -> c_int;
    fn ubq_snapshot(owner: *mut c_void) -> *const c_char;
    fn ubq_resize(owner: *mut c_void, columns: c_int, rows: c_int);
    fn ubq_input(owner: *mut c_void, bytes: *const u8, length: usize);
    fn ubq_intent(owner: *mut c_void, intent: *const NativeIntent) -> c_int;
    fn ubq_watch_directory(owner: *mut c_void);
    fn ubq_set_view(owner: *mut c_void, session: u32, columns: c_int, rows: c_int, focused: c_int);
    fn ubq_attach(owner: *mut c_void, session: u32, workspace: u32);
    fn ubq_reveal(owner: *mut c_void, session: u32);
    fn ubq_frame(owner: *mut c_void, session: u32) -> NativeFrame;
    fn ubq_dump(
        bytes: *const u8,
        length: usize,
        columns: c_int,
        rows: c_int,
        metadata: c_int,
    ) -> *mut c_char;
    fn ubq_directory(
        host: *const c_char,
        ssh: *const c_char,
        socket: *const c_char,
        timeout: c_int,
    ) -> *mut c_char;
    fn ubq_retire_idle_daemon(host: *const c_char, socket: *const c_char) -> c_int;
    fn ubq_text_free(text: *mut c_char);
}
/// Asks a local daemon this client started only to list to stop again; a
/// daemon that holds any session or workspace declines.
pub fn retire_idle_daemon(options: &Options, socket: &str) -> bool {
    if !options.ssh.is_empty() {
        return false;
    }
    let (Ok(host), Ok(socket)) = (CString::new(options.host.as_str()), CString::new(socket)) else {
        return false;
    };
    unsafe { ubq_retire_idle_daemon(host.as_ptr(), socket.as_ptr()) != 0 }
}
/// Lists the host's workspaces once. Starts the host's daemon when no socket
/// is named and none runs, as connecting would: workspaces the host records
/// outlive its daemon, and only a running daemon lists or restores them.
pub fn directory(options: &Options, timeout_ms: i32) -> Result<Directory, String> {
    let text = |value: &str| CString::new(value).map_err(|_| "option contains NUL".to_string());
    let (host, ssh, socket) = (
        text(&options.host)?,
        text(&options.ssh)?,
        text(&options.socket)?,
    );
    let value = unsafe { ubq_directory(host.as_ptr(), ssh.as_ptr(), socket.as_ptr(), timeout_ms) };
    if value.is_null() {
        return Err("Core could not list workspaces".into());
    }
    let bytes = unsafe { CStr::from_ptr(value) }.to_bytes().to_vec();
    unsafe { ubq_text_free(value) };
    let directory: Directory = serde_json::from_str(&String::from_utf8_lossy(&bytes))
        .map_err(|e| format!("invalid Core bridge listing: {e}"))?;
    if !directory.error.is_empty() {
        return Err(directory.error);
    }
    Ok(directory)
}
pub fn dump(bytes: &[u8], columns: u16, rows: u16, metadata: bool) -> Result<String, String> {
    let value = unsafe {
        ubq_dump(
            bytes.as_ptr(),
            bytes.len(),
            columns.into(),
            rows.into(),
            i32::from(metadata),
        )
    };
    if value.is_null() {
        return Err("invalid capture geometry or input limit".into());
    }
    let text = unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned();
    unsafe { ubq_text_free(value) };
    Ok(text)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub foreground: u32,
    pub background: u32,
    pub underline: u32,
    pub attributes: u8,
    pub width: u8,
    pub underline_style: u8,
    pub hyperlink: String,
}
#[derive(Default)]
pub struct Frame {
    pub cells: Vec<Cell>,
    pub columns: u16,
    pub rows: u16,
    pub cursor: Option<(u16, u16)>,
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub bracketed_paste: bool,
    pub cursor_style: u8,
    pub keyboard_flags: u16,
    pub quads: Vec<Quad>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PaneView {
    session: u32,
    columns: u16,
    rows: u16,
    focused: bool,
}
pub struct Client {
    owner: NonNull<c_void>,
    visible: Vec<PaneView>,
    cell_pixels: (u16, u16),
    surface_focused: bool,
    interaction_focused: bool,
    pixels_dirty: bool,
    _thread: PhantomData<Rc<()>>,
}
impl Client {
    pub fn new(options: &Options, columns: u16, rows: u16) -> Result<Self, String> {
        let strings = [
            &options.host,
            &options.ssh,
            &options.socket,
            &options.workspace_name,
        ]
        .map(|s| CString::new(s.as_bytes()).map_err(|_| "option contains NUL".to_string()));
        let [host, ssh, socket, name] = strings;
        let (host, ssh, socket, name) = (host?, ssh?, socket?, name?);
        // The native constructor copies every string; the owner is confined to this thread.
        let owner = unsafe {
            ubq_open(
                host.as_ptr(),
                ssh.as_ptr(),
                socket.as_ptr(),
                options.session,
                options.workspace,
                options.daemon_instance,
                name.as_ptr(),
                columns.into(),
                rows.into(),
                options.profile as i32,
            )
        };
        Ok(Self {
            owner: NonNull::new(owner).ok_or("Core could not create a workspace")?,
            visible: Vec::new(),
            cell_pixels: (8, 16),
            surface_focused: true,
            interaction_focused: true,
            pixels_dirty: true,
            _thread: PhantomData,
        })
    }
    pub fn tick(&mut self) -> bool {
        unsafe { ubq_tick(self.owner.as_ptr()) != 0 }
    }
    pub fn dirty(&self) -> bool {
        unsafe { ubq_dirty(self.owner.as_ptr()) != 0 }
    }
    pub fn wait_descriptor(&self) -> c_int {
        unsafe { ubq_wait_descriptor(self.owner.as_ptr()) }
    }
    pub fn wait_timeout(&self) -> c_int {
        unsafe { ubq_wait_timeout(self.owner.as_ptr()) }
    }
    pub fn snapshot(&mut self) -> Result<Snapshot, String> {
        // Copy the borrowed view before any subsequent native call can invalidate it.
        let bytes = unsafe { CStr::from_ptr(ubq_snapshot(self.owner.as_ptr())) }.to_bytes();
        // Titles, paths and messages originate in hosted programs and may not be
        // valid UTF-8; replace bad bytes rather than ending the session.
        serde_json::from_str(&String::from_utf8_lossy(bytes))
            .map_err(|e| format!("invalid Core bridge snapshot: {e}"))
    }
    pub fn resize(&mut self, columns: u16, rows: u16) {
        unsafe { ubq_resize(self.owner.as_ptr(), columns.into(), rows.into()) }
    }
    pub fn input(&mut self, bytes: &[u8]) {
        unsafe { ubq_input(self.owner.as_ptr(), bytes.as_ptr(), bytes.len()) }
    }
    pub fn set_cell_pixels(&mut self, pixels: (u16, u16)) {
        if pixels != self.cell_pixels && pixels.0 > 0 && pixels.1 > 0 {
            self.cell_pixels = pixels;
            self.pixels_dirty = true;
        }
    }
    pub fn surface_focus(&mut self, focused: bool, panes: &[Pane]) {
        self.surface_focused = focused;
        self.configure_views(panes, self.interaction_focused);
    }
    pub fn mouse(
        &mut self,
        session: u32,
        kind: i32,
        button: i32,
        cell: (i32, i32),
        modifiers: i32,
        pixels: (i32, i32),
    ) {
        unsafe {
            ubq_mouse(
                self.owner.as_ptr(),
                session,
                kind,
                button,
                cell.0,
                cell.1,
                modifiers,
                pixels.0,
                pixels.1,
            )
        }
    }
    pub fn select(&mut self, snapshot: &Snapshot, tab: u32, pane: Option<u32>) {
        let intent = NativeIntent {
            verb: if pane.is_some() {
                c"focus_pane"
            } else {
                c"set_active_tab"
            }
            .as_ptr(),
            surface_id: snapshot.surface_id,
            tab_id: tab,
            pane_id: pane.unwrap_or(0),
            target_tab_id: 0,
            target_pane_id: 0,
            direction: 0,
            orientation: 0,
            side: 0,
            ratio: 0.5,
            title: b"".as_ptr(),
            title_length: 0,
        };
        unsafe {
            ubq_intent(self.owner.as_ptr(), &intent);
        }
    }
    pub fn image(&mut self, session: u32, id: u32, include_pixels: bool) -> Option<Image> {
        let native = unsafe { ubq_image(self.owner.as_ptr(), session, id) };
        if native.width == 0
            || native.height == 0
            || native.pixels.is_null()
            || ![24, 32].contains(&native.format)
            || native.length
                != native.width as usize * native.height as usize * (native.format as usize / 8)
        {
            return None;
        }
        Some(Image {
            pixels: if !include_pixels {
                Vec::new()
            } else {
                unsafe { slice::from_raw_parts(native.pixels, native.length) }.to_vec()
            },
            stamp: native.stamp,
            width: native.width,
            height: native.height,
            format: native.format,
        })
    }
    pub fn action(&mut self, key: u8, text: &str, snapshot: &Snapshot) {
        if key == b'C' {
            self.attach(0, 0);
            return;
        }
        if key == b'w' || key == b's' {
            unsafe { ubq_watch_directory(self.owner.as_ptr()) };
            return;
        }
        let Some(intent) = command(key, text, snapshot) else {
            return;
        };
        unsafe { ubq_intent(self.owner.as_ptr(), &intent) };
    }
    pub fn configure_views(&mut self, panes: &[Pane], interaction_focused: bool) {
        self.interaction_focused = interaction_focused;
        let visible = panes
            .iter()
            .filter(|pane| pane.rows > pane.header && pane.columns > 0)
            .map(|pane| PaneView {
                session: pane.session,
                columns: pane.columns,
                rows: pane.rows - pane.header,
                focused: pane.focused && self.surface_focused && interaction_focused,
            })
            .collect::<Vec<_>>();
        for previous in &self.visible {
            if !visible.iter().any(|view| view.session == previous.session) {
                unsafe { ubq_set_view(self.owner.as_ptr(), previous.session, 0, 0, 0) };
            }
        }
        for view in visible
            .iter()
            .filter(|view| self.pixels_dirty || !self.visible.contains(view))
        {
            unsafe {
                ubq_pixels(
                    self.owner.as_ptr(),
                    view.session,
                    view.columns.into(),
                    view.rows.into(),
                    self.cell_pixels.0.into(),
                    self.cell_pixels.1.into(),
                );
                ubq_set_view(
                    self.owner.as_ptr(),
                    view.session,
                    view.columns.into(),
                    view.rows.into(),
                    i32::from(view.focused),
                )
            };
        }
        self.visible = visible;
        self.pixels_dirty = false;
    }
    /// Focuses a pane of the attached workspace without reconnecting.
    pub fn reveal(&mut self, session: u32) {
        unsafe { ubq_reveal(self.owner.as_ptr(), session) }
    }
    pub fn attach(&mut self, session: u32, workspace: u32) {
        unsafe { ubq_attach(self.owner.as_ptr(), session, workspace) }
        self.visible.clear();
        self.pixels_dirty = true;
    }
    pub fn frame(&mut self, session: u32) -> Frame {
        let native = unsafe { ubq_frame(self.owner.as_ptr(), session) };
        if native.cells.is_null()
            || !(1..=512).contains(&native.columns)
            || !(1..=256).contains(&native.rows)
            || native.count != native.columns as usize * native.rows as usize
        {
            return Frame::default();
        }
        // The native owner retains these buffers until the next native call.
        let cells = unsafe { slice::from_raw_parts(native.cells, native.count) }
            .iter()
            .map(|cell| {
                let text = if cell.length == 0 {
                    " ".to_string()
                } else {
                    let bytes =
                        unsafe { slice::from_raw_parts(cell.text.cast::<u8>(), cell.length) };
                    String::from_utf8_lossy(bytes).into_owned()
                };
                Cell {
                    text,
                    foreground: cell.foreground,
                    background: cell.background,
                    underline: cell.underline,
                    attributes: cell.attributes,
                    width: cell.width,
                    underline_style: cell.underline_style,
                    hyperlink: if cell.hyperlink_length == 0 {
                        String::new()
                    } else {
                        String::from_utf8_lossy(unsafe {
                            slice::from_raw_parts(
                                cell.hyperlink.cast::<u8>(),
                                cell.hyperlink_length,
                            )
                        })
                        .into_owned()
                    },
                }
            })
            .collect();
        Frame {
            cells,
            columns: native.columns as u16,
            rows: native.rows as u16,
            cursor: (native.cursor_visible != 0
                && native.cursor_column >= 0
                && native.cursor_row >= 0
                && native.cursor_column < native.columns
                && native.cursor_row < native.rows)
                .then_some((native.cursor_column as u16, native.cursor_row as u16)),
            application_cursor: native.application_cursor != 0,
            application_keypad: native.application_keypad != 0,
            bracketed_paste: native.bracketed_paste != 0,
            cursor_style: native.cursor_style,
            keyboard_flags: native.keyboard_flags,
            quads: if native.quad_count == 0 {
                Vec::new()
            } else {
                unsafe { slice::from_raw_parts(native.quads, native.quad_count) }.to_vec()
            },
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        unsafe { ubq_close(self.owner.as_ptr()) }
    }
}
fn command(key: u8, title: &str, snapshot: &Snapshot) -> Option<NativeIntent> {
    let active = snapshot
        .tabs
        .iter()
        .position(|tab| tab.id == snapshot.tab_id)?;
    let tab = snapshot.tabs.get(active)?;
    let focused = tab.pane_ids.iter().position(|id| *id == snapshot.pane_id)?;
    let mut intent = NativeIntent {
        verb: c"add_tab".as_ptr(),
        surface_id: snapshot.surface_id,
        tab_id: snapshot.tab_id,
        pane_id: snapshot.pane_id,
        target_tab_id: 0,
        target_pane_id: 0,
        direction: 0,
        orientation: 0,
        side: 0,
        ratio: 0.5,
        title: title.as_ptr(),
        title_length: title.len(),
    };
    let next_tab = |forward: bool| {
        &snapshot.tabs
            [(active + if forward { 1 } else { snapshot.tabs.len() - 1 }) % snapshot.tabs.len()]
    };
    let verb = match key {
        b'c' => c"add_tab",
        b'n' | b'p' => {
            intent.tab_id = next_tab(key == b'n').id;
            c"set_active_tab"
        }
        b'W' => {
            let active = snapshot
                .surfaces
                .iter()
                .position(|id| *id == snapshot.surface_id)?;
            intent.surface_id = snapshot.surfaces[(active + 1) % snapshot.surfaces.len()];
            c"set_active_window"
        }
        b'N' => c"add_window",
        b'o' => {
            intent.pane_id = tab.pane_ids[(focused + 1) % tab.pane_ids.len()];
            c"focus_pane"
        }
        b'H' | b'L' | b'K' | b'J' => {
            intent.direction = match key {
                b'H' => 0,
                b'L' => 1,
                b'K' => 2,
                _ => 3,
            };
            c"move_focus"
        }
        b'|' | b'-' => {
            intent.orientation = u8::from(key == b'-');
            c"split"
        }
        b'z' => c"zoom",
        b'=' => c"balance",
        b'+' | b'_' => {
            intent.direction = u8::from(key == b'+');
            intent.ratio = 0.05;
            c"adjust_ratio"
        }
        b't' => c"extract",
        b'S' => {
            intent.target_pane_id = tab.pane_ids[(focused + 1) % tab.pane_ids.len()];
            c"swap"
        }
        b'm' => {
            let destination = next_tab(true);
            intent.target_tab_id = destination.id;
            intent.target_pane_id = destination.focused_pane;
            intent.side = 1;
            c"move"
        }
        b',' => c"rename_tab",
        b'R' => c"rename_workspace",
        b'x' => c"close_pane",
        b'X' => c"close_tab",
        b'!' => c"close_window",
        b'1'..=b'9' => {
            intent.tab_id = snapshot.tabs.get(usize::from(key - b'1'))?.id;
            c"set_active_tab"
        }
        _ => return None,
    };
    intent.verb = verb.as_ptr();
    Some(intent)
}
