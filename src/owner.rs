use std::{fs, io, path::Path, time::Duration};

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub starttime: u64,
}

impl ProcessIdentity {
    #[must_use]
    pub fn key(self) -> String {
        format!("{}:{}", self.pid, self.starttime)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerChain {
    pub nearest: ProcessIdentity,
    pub all: Vec<ProcessIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stat {
    state: char,
    parent_pid: u32,
    starttime: u64,
}

pub fn discover_current() -> Result<OwnerChain, Error> {
    let me = std::process::id();
    let parent = read_stat(Path::new("/proc"), me)?.map_or(me, |stat| stat.parent_pid);
    discover_from(Path::new("/proc"), parent)
}

pub fn discover_from(proc_root: &Path, start_pid: u32) -> Result<OwnerChain, Error> {
    let mut pid = start_pid;
    let mut ancestors = Vec::new();
    while let Some(stat) = read_stat(proc_root, pid)? {
        if is_pi(proc_root, pid) {
            ancestors.push(ProcessIdentity {
                pid,
                starttime: stat.starttime,
            });
        }
        if stat.parent_pid == 0 || stat.parent_pid == pid {
            break;
        }
        pid = stat.parent_pid;
    }
    let nearest = ancestors.first().copied().ok_or(Error::NoPiAncestor)?;
    Ok(OwnerChain {
        nearest,
        all: ancestors,
    })
}

pub fn is_alive(proc_root: &Path, identity: ProcessIdentity) -> Result<bool, Error> {
    validate_root(proc_root)?;
    let Some(stat) = read_stat(proc_root, identity.pid)? else {
        return Ok(false);
    };
    Ok(stat.state != 'Z' && stat.starttime == identity.starttime)
}

pub fn write_owners(path: &Path, chain: &OwnerChain) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        crate::paths::ensure_private_dir(parent)?;
    }
    let body = chain
        .all
        .iter()
        .map(|identity| identity.key())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, format!("{body}\n"))
        .map_err(|source| Error::state_io("write owners file", path, source))
}

pub fn read_owners(path: &Path) -> Result<Vec<ProcessIdentity>, Error> {
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(source) => {
            return Err(Error::state_io("read owners file", path, source));
        }
    };
    let mut owners = Vec::new();
    for line in body.lines() {
        let Some((pid, starttime)) = line.split_once(':') else {
            return Err(Error::InvalidInput {
                message: "owners file is malformed".to_owned(),
            });
        };
        let pid = pid.parse::<u32>().map_err(|_| Error::InvalidInput {
            message: "owners file is malformed".to_owned(),
        })?;
        let starttime = starttime.parse::<u64>().map_err(|_| Error::InvalidInput {
            message: "owners file is malformed".to_owned(),
        })?;
        owners.push(ProcessIdentity { pid, starttime });
    }
    Ok(owners)
}

pub fn any_dead(proc_root: &Path, owners: &[ProcessIdentity]) -> Result<bool, Error> {
    for owner in owners {
        if !is_alive(proc_root, *owner)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn wait_until_dead(proc_root: &Path, owners: &[ProcessIdentity]) -> Result<(), Error> {
    loop {
        if any_dead(proc_root, owners)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn is_pi(proc_root: &Path, pid: u32) -> bool {
    let base = proc_root.join(pid.to_string());
    let comm = fs::read_to_string(base.join("comm")).unwrap_or_default();
    if comm.trim() == "pi" {
        return true;
    }
    if let Ok(exe) = fs::read_link(base.join("exe"))
        && basename_is_pi(&exe)
    {
        return true;
    }
    if !matches!(comm.trim(), "node" | "bun") {
        return false;
    }
    let Ok(cmdline) = fs::read(base.join("cmdline")) else {
        return false;
    };
    cmdline_is_pi(&cmdline)
}

fn cmdline_is_pi(cmdline: &[u8]) -> bool {
    let mut args = cmdline
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned());
    let first = args.next().unwrap_or_default();
    if basename_is_pi(Path::new(&first)) {
        return true;
    }
    let mut expect_value = false;
    let mut eval_mode = false;
    for arg in args {
        if expect_value {
            expect_value = false;
            continue;
        }
        if arg == "--" {
            continue;
        }
        if arg == "--eval"
            || arg == "--print"
            || arg.starts_with("--eval=")
            || arg.starts_with("--print=")
            || (arg.len() > 1
                && !arg.starts_with("--")
                && (arg.starts_with("-p") || arg.starts_with("-e")))
        {
            eval_mode = true;
            continue;
        }
        if arg == "-" {
            return false;
        }
        if arg.starts_with("--") && !arg.contains('=') && option_takes_value(&arg) {
            expect_value = true;
            continue;
        }
        if matches!(arg.as_str(), "-r" | "-C") {
            expect_value = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        return !eval_mode && basename_is_pi(Path::new(&arg));
    }
    false
}

fn option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "--require"
            | "--import"
            | "--loader"
            | "--experimental-loader"
            | "--inspect-port"
            | "--env-file"
            | "--conditions"
            | "--title"
            | "--preload"
            | "--diagnostic-dir"
            | "--unhandled-rejections"
            | "--dns-result-order"
            | "--stack-trace-limit"
            | "--max-old-space-size"
            | "--max-semi-space-size"
            | "--trace-event-categories"
            | "--trace-event-file-pattern"
            | "--test-reporter"
            | "--test-name-pattern"
            | "--test-concurrency"
            | "--watch-path"
    )
}

fn basename_is_pi(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "pi")
}

fn read_stat(proc_root: &Path, pid: u32) -> Result<Option<Stat>, Error> {
    let path = proc_root.join(pid.to_string()).join("stat");
    let body = match fs::read_to_string(&path) {
        Ok(body) => body,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(Error::state_io("read process stat", &path, source));
        }
    };
    let start = body.find('(').ok_or_else(|| Error::InvalidInput {
        message: "process stat is malformed".to_owned(),
    })?;
    let end = body.rfind(')').ok_or_else(|| Error::InvalidInput {
        message: "process stat is malformed".to_owned(),
    })?;
    let fields: Vec<&str> = body[end + 1..].split_whitespace().collect();
    let state = fields
        .first()
        .and_then(|field| field.chars().next())
        .ok_or_else(|| Error::InvalidInput {
            message: "process stat is malformed".to_owned(),
        })?;
    let parent_pid = fields
        .get(1)
        .and_then(|field| field.parse::<u32>().ok())
        .ok_or_else(|| Error::InvalidInput {
            message: "process stat is malformed".to_owned(),
        })?;
    let starttime = fields
        .get(19)
        .and_then(|field| field.parse::<u64>().ok())
        .ok_or_else(|| Error::InvalidInput {
            message: "process stat is malformed".to_owned(),
        })?;
    let _ = start;
    Ok(Some(Stat {
        state,
        parent_pid,
        starttime,
    }))
}

fn validate_root(proc_root: &Path) -> Result<(), Error> {
    if proc_root.is_dir() {
        Ok(())
    } else {
        Err(Error::InvalidInput {
            message: format!("process root is not a directory: {}", proc_root.display()),
        })
    }
}

#[must_use]
pub fn owners_path_display(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::panic,
        clippy::expect_used,
        clippy::disallowed_methods
    )]
    use super::{ProcessIdentity, discover_from, is_alive};
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        path::{Path, PathBuf},
    };
    use tempfile::TempDir;

    fn proc_entry(root: &Path, pid: u32, parent_pid: u32, state: char, starttime: u64, comm: &str) {
        let dir = root.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("stat"),
            format!(
                "{pid} ({comm}) {state} {parent_pid} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 {starttime} 0 0\n"
            ),
        )
        .unwrap();
        fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn argv(parts: &[&str]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for part in parts {
            bytes.extend_from_slice(part.as_bytes());
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn finds_pi_after_node_options_with_values() {
        assert!(super::cmdline_is_pi(&argv(&[
            "node",
            "--require",
            "/tmp/bootstrap.cjs",
            "/usr/local/bin/pi",
        ])));
        assert!(!super::cmdline_is_pi(&argv(&[
            "node",
            "--require",
            "/usr/local/bin/pi",
            "/tmp/other.js",
        ])));
    }

    #[test]
    fn finds_pi_with_bun_and_direct_argv() {
        assert!(super::cmdline_is_pi(&argv(&[
            "bun",
            "--preload",
            "/tmp/x.ts",
            "/opt/pi"
        ])));
        assert!(super::cmdline_is_pi(&argv(&[
            "/usr/local/bin/pi",
            "--model",
            "x"
        ])));
        assert!(!super::cmdline_is_pi(&argv(&["node", "/tmp/other.js"])));
        assert!(!super::cmdline_is_pi(&argv(&[
            "node",
            "-e",
            "console.log(1)",
            "/opt/bin/pi"
        ])));
        assert!(!super::cmdline_is_pi(&argv(&[
            "node",
            "--eval=console.log(1)",
            "/opt/bin/pi"
        ])));
        assert!(super::cmdline_is_pi(&argv(&[
            "node",
            "--diagnostic-dir",
            "/tmp",
            "/opt/bin/pi"
        ])));
        assert!(!super::cmdline_is_pi(&argv(&["node", "-pe", "0//pi"])));
        assert!(!super::cmdline_is_pi(&argv(&["node", "-", "/opt/bin/pi"])));
        assert!(super::cmdline_is_pi(&argv(&[
            "node",
            "--unhandled-rejections",
            "strict",
            "/opt/bin/pi"
        ])));
        assert!(super::cmdline_is_pi(&argv(&[
            "node",
            "--dns-result-order",
            "ipv4first",
            "/opt/bin/pi"
        ])));
    }

    #[test]
    fn nearest_pi_ancestor_is_found() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        proc_entry(root, 10, 20, 'S', 500, "pi");
        proc_entry(root, 20, 30, 'S', 400, "bash");
        proc_entry(root, 30, 0, 'S', 300, "pi");
        let chain = discover_from(root, 10).unwrap();
        assert_eq!(chain.nearest.pid, 10);
        assert_eq!(chain.all.len(), 2);
    }

    #[test]
    fn root_pi_is_included() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        proc_entry(root, 10, 20, 'S', 500, "bash");
        proc_entry(root, 20, 0, 'S', 400, "pi");
        let chain = discover_from(root, 10).unwrap();
        assert_eq!(chain.nearest.pid, 20);
        assert_eq!(chain.all.len(), 1);
    }

    #[test]
    fn live_process_is_alive() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        proc_entry(root, 10, 1, 'S', 500, "pi");
        assert!(
            is_alive(
                root,
                ProcessIdentity {
                    pid: 10,
                    starttime: 500,
                }
            )
            .unwrap()
        );
    }

    #[test]
    fn zombie_is_dead() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        proc_entry(root, 10, 1, 'Z', 500, "pi");
        assert!(
            !is_alive(
                root,
                ProcessIdentity {
                    pid: 10,
                    starttime: 500
                }
            )
            .unwrap()
        );
    }

    #[test]
    fn changed_starttime_is_dead() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        proc_entry(root, 10, 1, 'S', 501, "pi");
        assert!(
            !is_alive(
                root,
                ProcessIdentity {
                    pid: 10,
                    starttime: 500
                }
            )
            .unwrap()
        );
    }

    #[test]
    fn missing_process_is_dead() {
        let temp = TempDir::new().unwrap();
        assert!(
            !is_alive(
                temp.path(),
                ProcessIdentity {
                    pid: 999,
                    starttime: 1
                }
            )
            .unwrap()
        );
    }

    #[test]
    fn fake_executable_named_pi_is_detected() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        proc_entry(root, 10, 1, 'S', 500, "node");
        let exe = root.join("bin").join("pi");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"").unwrap();
        std::os::unix::fs::symlink(exe, root.join("10").join("exe")).unwrap();
        assert!(super::is_pi(root, 10));
    }

    #[test]
    fn owners_roundtrip() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("owners");
        let chain = super::OwnerChain {
            nearest: ProcessIdentity {
                pid: 10,
                starttime: 500,
            },
            all: vec![ProcessIdentity {
                pid: 10,
                starttime: 500,
            }],
        };
        super::write_owners(&path, &chain).unwrap();
        assert_eq!(super::read_owners(&path).unwrap(), chain.all);
    }

    #[test]
    fn proc_entry_is_synthetic() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        proc_entry(root, 10, 1, 'S', 500, "pi");
        assert!(PathBuf::from(root).join("10").join("stat").is_file());
    }
}
