// Copyright (C) 2026 Stanislav Yaglo
// SPDX-License-Identifier: Apache-2.0
use std::{
    env,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const HOST: &str = "ubiquitty-host";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum Profile {
    #[default]
    Modern,
    Dec,
    Ascii,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Command {
    /// Create a workspace, or open the one a target option selects.
    #[default]
    Run,
    /// Print the host's workspaces and exit.
    List,
    /// Open an existing workspace: the most recently used one, or the one
    /// named or numbered.
    Attach(Option<String>),
}

#[derive(Default, Debug)]
pub struct Options {
    pub command: Command,
    pub host: String,
    pub host_explicit: bool,
    pub ssh: String,
    pub socket: String,
    pub workspace_name: String,
    pub session: u32,
    pub workspace: u32,
    /// The daemon that listed `workspace`, set by `attach`; zero for any.
    pub daemon_instance: u64,
    pub geometry: Option<(u16, u16)>,
    pub profile: Profile,
    pub plain: bool,
    pub help: bool,
}
impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut result = Self::default();
        let mut command = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--help" | "-h" => {
                    result.help = true;
                    continue;
                }
                "--no-powerline" => {
                    result.plain = true;
                    continue;
                }
                "--ascii" => {
                    result.profile = Profile::Ascii;
                    continue;
                }
                "--legacy" => {
                    result.profile = Profile::Dec;
                    continue;
                }
                "--host-binary" | "--ssh" | "--socket" | "--session" | "--workspace" | "-w"
                | "--workspace-id" | "--geometry" => {}
                _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
                _ => {
                    command = Some(match (command, arg.as_str()) {
                        (None, "ls" | "list") => Command::List,
                        (None, "attach" | "a") => Command::Attach(None),
                        (Some(Command::Attach(None)), _) => Command::Attach(Some(arg)),
                        (None, _) => return Err(format!("unknown command: {arg}")),
                        _ => return Err(format!("unexpected argument: {arg}")),
                    });
                    continue;
                }
            }
            let value = args
                .next()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("missing value for {arg}"))?;
            if value.contains('\0') {
                return Err("options cannot contain NUL bytes".into());
            }
            match arg.as_str() {
                "--host-binary" => {
                    result.host = value;
                    result.host_explicit = true;
                }
                "--ssh" => {
                    if value.starts_with('-') {
                        return Err("SSH destination cannot start with '-'".into());
                    }
                    result.ssh = value;
                }
                "--socket" => result.socket = value,
                "--workspace" | "-w" => result.workspace_name = value,
                "--session" | "--workspace-id" => {
                    let number = value
                        .parse::<u32>()
                        .ok()
                        .filter(|n| *n != 0)
                        .ok_or("session and workspace IDs must be positive integers")?;
                    if arg == "--session" {
                        result.session = number;
                    } else {
                        result.workspace = number;
                    }
                }
                "--geometry" => {
                    let (columns, rows) =
                        value.split_once('x').ok_or("geometry must be COLSxROWS")?;
                    let columns = columns
                        .parse::<u16>()
                        .ok()
                        .filter(|n| (20..=512).contains(n))
                        .ok_or("columns must be between 20 and 512")?;
                    let rows = rows
                        .parse::<u16>()
                        .ok()
                        .filter(|n| (4..=256).contains(n))
                        .ok_or("rows must be between 4 and 256")?;
                    result.geometry = Some((columns, rows));
                }
                _ => unreachable!(),
            }
        }
        result.command = command.unwrap_or_default();
        let targets = usize::from(result.session != 0)
            + usize::from(result.workspace != 0)
            + usize::from(!result.workspace_name.is_empty());
        if targets > 1 {
            return Err("choose only one of --session, --workspace-id, or --workspace".into());
        }
        if targets > 0 && result.command != Command::Run {
            return Err(
                "ls and attach take no --session, --workspace-id, or --workspace; \
                 name the workspace after attach"
                    .into(),
            );
        }
        Ok(result)
    }
    /// Resolves the host executable before Core spawns it through a shell, so
    /// a missing host is reported instead of being retried as a lost connection.
    ///
    /// Locally, an explicit path is used as given and an explicit bare name is
    /// searched for on PATH. Without one, the host the app installs in the
    /// control directory (`$UBIQUITTY_HOME`, else `~/.ubiquitty`) comes first,
    /// so the client reaches the daemon the app's sessions run in; then the
    /// host beside this executable, then PATH. Over SSH, the host Ubiquitty
    /// installs on a server, `~/.ubiquitty/ubiquitty-host`, unless a path or
    /// name is given.
    pub fn resolve_host(&mut self) -> Result<(), String> {
        let name = if self.host_explicit {
            self.host.clone()
        } else {
            HOST.into()
        };
        if !self.ssh.is_empty() {
            // A remote command starts in the remote home, where Ubiquitty
            // installs a server's host, as the app's bootstrap expects.
            self.host = if self.host_explicit {
                name
            } else {
                format!(".ubiquitty/{HOST}")
            };
            return Ok(());
        }
        let preferred = if self.host_explicit {
            Vec::new()
        } else {
            // The control directory, where the app installs this machine's
            // host: $UBIQUITTY_HOME when set, as Core resolves it.
            let installed = env::var_os("UBIQUITTY_HOME")
                .filter(|home| !home.is_empty())
                .map(PathBuf::from)
                .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".ubiquitty")))
                .filter(|directory| directory.is_absolute())
                .map(|directory| directory.join(HOST));
            let beside = env::current_exe()
                .and_then(|path| path.canonicalize())
                .ok()
                .and_then(|path| path.parent().map(|dir| dir.join(HOST)));
            installed.into_iter().chain(beside).collect()
        };
        self.host = find_host(&name, preferred, env::var_os("PATH"))
            .ok_or_else(|| {
                if self.host_explicit {
                    format!("host executable not found: {name}")
                } else {
                    format!(
                        "{HOST} not found in the Ubiquitty control directory, beside utty, or on PATH; \
                         pass --host-binary PATH"
                    )
                }
            })?
            .to_string_lossy()
            .into_owned();
        Ok(())
    }
}

fn find_host(
    name: &str,
    preferred: Vec<PathBuf>,
    path: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let executable = |path: &Path| {
        path.metadata()
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        let path = PathBuf::from(name);
        return executable(&path).then(|| path.canonicalize().unwrap_or(path));
    }
    preferred
        .into_iter()
        .chain(env::split_paths(&path.unwrap_or_default()).map(|dir| dir.join(name)))
        .find(|candidate| candidate.is_absolute() && executable(candidate))
}

pub const HELP: &str = "Usage: utty [options]                    Create a workspace
       utty [options] -w NAME            Open workspace NAME, creating it if needed
       utty [options] attach [NAME|ID]   Open the most recently used workspace,
                                         or the one named or numbered
       utty [options] ls                 List the host's workspaces
Options:
  --host-binary PATH  Matching ubiquitty-host (default: ~/.ubiquitty,
                      then beside utty, then PATH; over --ssh,
                      ~/.ubiquitty/ubiquitty-host on that machine)
  --ssh USER@HOST     Noninteractive SSH transport
  --socket NAME       Existing daemon socket leaf
  --session ID        Existing session
  -w, --workspace NAME
                      Open or create a named workspace
  --workspace-id ID   Open a workspace by ID
  --geometry CxR      Physical size, including the workspace bar
  --no-powerline      Plain separators
  --legacy            Seven-bit DEC terminal profile
  --ascii             Seven-bit ASCII terminal profile
Ctrl-] then d: detach, w: workspaces, c: new tab, |: split, ?: help.
Ctrl-] twice sends a literal Ctrl-].";

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(values: &[&str]) -> Result<Options, String> {
        Options::parse(values.iter().map(|v| v.to_string()))
    }
    #[test]
    fn rejects_ambiguous_targets_and_invalid_ssh() {
        assert!(parse(&["--session", "1", "--workspace", "one"]).is_err());
        assert!(parse(&["--ssh", "-oProxyCommand=anything"]).is_err());
        assert!(parse(&["--session", "4294967296"]).is_err());
        assert!(parse(&["--geometry", "80x0"]).is_err());
    }
    #[test]
    fn commands_take_their_target_as_an_argument() {
        assert_eq!(parse(&[]).unwrap().command, Command::Run);
        assert_eq!(parse(&["ls"]).unwrap().command, Command::List);
        assert_eq!(parse(&["attach"]).unwrap().command, Command::Attach(None));
        let named = parse(&["--socket", "s.sock", "a", "work", "--legacy"]).unwrap();
        assert_eq!(named.command, Command::Attach(Some("work".into())));
        assert_eq!(
            (named.socket.as_str(), named.profile),
            ("s.sock", Profile::Dec)
        );
        assert_eq!(parse(&["-w", "work"]).unwrap().workspace_name, "work");
        assert!(parse(&["attach", "one", "two"]).is_err());
        assert!(parse(&["ls", "extra"]).is_err());
        assert!(parse(&["attach", "--session", "3"]).is_err());
        assert!(parse(&["start"]).is_err());
    }
    #[test]
    fn remote_hosts_default_to_the_servers_install() {
        let mut options = parse(&["--ssh", "me@server"]).unwrap();
        options.resolve_host().unwrap();
        assert_eq!(options.host, ".ubiquitty/ubiquitty-host");
        let mut options =
            parse(&["--ssh", "me@server", "--host-binary", "ubiquitty-host"]).unwrap();
        options.resolve_host().unwrap();
        assert_eq!(options.host, "ubiquitty-host");
    }
    #[test]
    fn host_prefers_the_installed_then_the_beside_copy_and_is_never_relative() {
        let root = env::temp_dir().join(format!("ubq-host-{}", std::process::id()));
        let (installed, beside, listed) = (
            root.join("installed"),
            root.join("beside"),
            root.join("listed"),
        );
        for dir in [&installed, &beside, &listed] {
            std::fs::create_dir_all(dir).unwrap();
            let host = dir.join(HOST);
            std::fs::write(&host, "").unwrap();
            std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = Some(env::join_paths([Path::new("."), &listed]).unwrap());
        let preferred = vec![installed.join(HOST), beside.join(HOST)];
        assert_eq!(
            find_host(HOST, preferred.clone(), path.clone()),
            Some(installed.join(HOST))
        );
        std::fs::remove_file(installed.join(HOST)).unwrap();
        assert_eq!(
            find_host(HOST, preferred, path.clone()),
            Some(beside.join(HOST))
        );
        assert_eq!(
            find_host(HOST, Vec::new(), path.clone()),
            Some(listed.join(HOST))
        );
        assert_eq!(find_host(HOST, vec![root.join("missing")], None), None);
        std::fs::set_permissions(listed.join(HOST), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        assert_eq!(find_host(HOST, Vec::new(), path), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
