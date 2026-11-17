// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
//! Finding a workspace before the terminal is taken over: `ls`, `attach`,
//! and the command a detach prints to return.
use crate::{
    core::{Directory, ListedWorkspace},
    options::Options,
    text::clean,
};
use std::process::{Command, Stdio};
use unicode_width::UnicodeWidthStr;

/// The status `ubiquitty-host --probe-daemon` exits with when nothing listens.
const DAEMON_GONE: i32 = 3;

/// Asks a local host binary whether its daemon runs, without starting one.
///
/// - Returns: The answer, or nothing when it cannot be had locally (SSH) or
///   the probe says nothing either way.
pub fn daemon_running(options: &Options) -> Option<bool> {
    if !options.ssh.is_empty() {
        return None;
    }
    let mut probe = Command::new(&options.host);
    probe.arg("--probe-daemon");
    if !options.socket.is_empty() {
        probe.args(["--socket", &options.socket]);
    }
    let status = probe
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    match status.code() {
        Some(0) => Some(true),
        Some(DAEMON_GONE) => Some(false),
        _ => None,
    }
}

/// Picks the workspace `attach` opens, by id.
///
/// Without a request, the live workspace someone typed into last, then the
/// one with the newest output, then the newest; with nothing live, the newest
/// restorable one, which attaching recreates. A request matches a workspace's
/// name, then its id.
pub fn choose(directory: &Directory, requested: Option<&str>) -> Result<u32, String> {
    let Some(requested) = requested else {
        let live = directory
            .workspaces
            .iter()
            .filter(|workspace| !workspace.restorable)
            .max_by_key(|workspace| {
                (
                    workspace.last_input_ms,
                    workspace.last_output_ms,
                    workspace.id,
                )
            });
        let restorable = || {
            directory
                .workspaces
                .iter()
                .max_by_key(|workspace| workspace.id)
        };
        return live
            .or_else(restorable)
            .map(|workspace| workspace.id)
            .ok_or_else(|| "no workspaces to attach to; run utty to create one".into());
    };
    directory
        .workspaces
        .iter()
        .find(|workspace| workspace.name == requested)
        .or_else(|| {
            let id = requested.parse::<u32>().ok()?;
            directory
                .workspaces
                .iter()
                .find(|workspace| workspace.id == id)
        })
        .map(|workspace| workspace.id)
        .ok_or_else(|| {
            format!(
                "no workspace named or numbered {}; `utty -w NAME` creates a named one",
                clean(requested, false)
            )
        })
}

/// How long before the host's `now_ms` an instant was, briefly; "-" for
/// never.
pub fn ago(now_ms: u64, then_ms: u64) -> String {
    if then_ms == 0 || now_ms == 0 {
        return "-".into();
    }
    let seconds = now_ms.saturating_sub(then_ms) / 1000;
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

fn age(now_ms: u64, workspace: &ListedWorkspace) -> String {
    ago(
        now_ms,
        workspace.last_input_ms.max(workspace.last_output_ms),
    )
}

/// Lays out `ls`: one row per workspace, then the host's other daemons.
pub fn table(directory: &Directory) -> String {
    let mut rows = vec![
        [
            "ID", "NAME", "TITLE", "WINDOWS", "TABS", "PANES", "ATTACHED", "ACTIVE",
        ]
        .map(String::from),
    ];
    for workspace in &directory.workspaces {
        let attached = match (workspace.people, workspace.agents) {
            (0, 0) => "-".to_string(),
            (people, 0) => people.to_string(),
            (people, agents) => format!("{people}+{agents} agents"),
        };
        rows.push([
            workspace.id.to_string(),
            clean(&workspace.name, false),
            clean(&workspace.title, false),
            workspace.windows.to_string(),
            workspace.tabs.to_string(),
            workspace.panes.to_string(),
            attached,
            if workspace.restorable {
                "restorable".into()
            } else {
                age(directory.now_ms, workspace)
            },
        ]);
    }
    let mut widths = [0; 8];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.width());
        }
    }
    let mut output = String::new();
    for row in &rows {
        let mut line = String::new();
        for (index, (cell, width)) in row.iter().zip(widths).enumerate() {
            line.push_str(cell);
            if index + 1 < row.len() {
                line.push_str(&" ".repeat(width - cell.width() + 2));
            }
        }
        output.push_str(line.trim_end());
        output.push('\n');
    }
    for instance in &directory.instances {
        output.push_str(&format!(
            "Another host daemon{} runs at --socket {}\n",
            if instance.version.is_empty() {
                String::new()
            } else {
                format!(" ({})", clean(&instance.version, false))
            },
            clean(&instance.socket, false)
        ));
    }
    output
}

fn quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:@+=,".contains(&byte))
    {
        return text.into();
    }
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The shortest command that returns to a detached workspace. Options the
/// person chose are kept; the socket is left out when the host chose it,
/// since the same host binary reaches the same daemon again.
pub fn reattach_command(program: &str, options: &Options, name: &str, session: u32) -> String {
    let mut command = vec![quote(program)];
    if options.host_explicit {
        command.extend(["--host-binary".into(), quote(&options.host)]);
    }
    if !options.ssh.is_empty() {
        command.extend(["--ssh".into(), quote(&options.ssh)]);
    }
    if !options.socket.is_empty() {
        command.extend(["--socket".into(), quote(&options.socket)]);
    }
    if name.is_empty() {
        command.extend(["--session".into(), session.to_string()]);
    } else {
        command.extend(["attach".into(), quote(name)]);
    }
    clean(&command.join(" "), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn workspace(id: u32, name: &str, input: u64, output: u64) -> ListedWorkspace {
        ListedWorkspace {
            id,
            name: name.into(),
            title: format!("t{id}"),
            windows: 1,
            tabs: 1,
            panes: 1,
            last_input_ms: input,
            last_output_ms: output,
            ..ListedWorkspace::default()
        }
    }
    #[test]
    fn attach_prefers_recent_input_then_names_then_ids() {
        let mut directory = Directory {
            now_ms: 10_000_000,
            workspaces: vec![
                workspace(1, "work", 9_000_000, 0),
                workspace(2, "", 1_000, 9_999_000),
                workspace(3, "7", 0, 0),
                ListedWorkspace {
                    restorable: true,
                    ..workspace(7, "old", 0, 0)
                },
                ListedWorkspace {
                    restorable: true,
                    ..workspace(9, "", 0, 0)
                },
            ],
            ..Directory::default()
        };
        assert_eq!(choose(&directory, None), Ok(1));
        assert_eq!(choose(&directory, Some("work")), Ok(1));
        assert_eq!(choose(&directory, Some("2")), Ok(2));
        // A name wins over an id that spells it.
        assert_eq!(choose(&directory, Some("7")), Ok(3));
        assert_eq!(choose(&directory, Some("old")), Ok(7));
        assert_eq!(choose(&directory, Some("9")), Ok(9));
        assert!(choose(&directory, Some("missing")).is_err());
        directory
            .workspaces
            .retain(|workspace| workspace.restorable);
        assert_eq!(choose(&directory, None), Ok(9));
        directory.workspaces.clear();
        assert!(choose(&directory, None).is_err());
    }
    #[test]
    fn listing_aligns_columns_and_sanitizes_host_text() {
        let directory = Directory {
            now_ms: 10_000_000,
            workspaces: vec![
                ListedWorkspace {
                    people: 2,
                    ..workspace(12, "w\x1b]0;x", 9_880_000, 0)
                },
                ListedWorkspace {
                    restorable: true,
                    ..workspace(3, "", 0, 0)
                },
            ],
            instances: vec![crate::core::HostInstance {
                socket: "host-1.sock".into(),
                ..Default::default()
            }],
            ..Directory::default()
        };
        let table = table(&directory);
        let lines = table.lines().collect::<Vec<_>>();
        assert_eq!(lines[0].find("NAME"), lines[1].find("w?]0;x"));
        assert_eq!(
            lines[1]
                .split_whitespace()
                .rev()
                .take(3)
                .collect::<Vec<_>>(),
            ["ago", "2m", "2"]
        );
        assert!(lines[2].ends_with("restorable"));
        assert_eq!(lines[3], "Another host daemon runs at --socket host-1.sock");
        assert!(!table.contains('\x1b'));
    }
    #[test]
    fn reattach_keeps_chosen_options_and_omits_the_host_chosen_socket() {
        let mut options = Options::default();
        assert_eq!(
            reattach_command("utty", &options, "", 42),
            "utty --session 42"
        );
        assert_eq!(
            reattach_command("./utty", &options, "my work", 42),
            "./utty attach 'my work'"
        );
        options.host = "/opt/u/ubiquitty-host".into();
        options.host_explicit = true;
        options.socket = "host-1.sock".into();
        assert_eq!(
            reattach_command("utty", &options, "", 5),
            "utty --host-binary /opt/u/ubiquitty-host --socket host-1.sock --session 5"
        );
    }
}
