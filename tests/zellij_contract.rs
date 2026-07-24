use std::{
    fs,
    path::Path,
    process::Command,
    str::FromStr,
    time::{Duration, Instant},
};

use agent_terminal::{
    config::write_private_files,
    domain::{SessionName, TerminalPaneId},
    error::Error,
    paths::ProjectPaths,
    zellij::{PaneTarget, Zellij, ZellijCli, find_owned_pane, parse_panes},
};
use tempfile::TempDir;

#[test]
fn pane_identity_ignores_plugin_with_same_numeric_id() -> Result<(), Box<dyn std::error::Error>> {
    let panes = parse_panes(
        r#"[
          {"id":0,"is_plugin":true,"title":"zellij:link","exited":false,"exit_status":null,"is_held":false},
          {"id":0,"is_plugin":false,"title":"agent-terminal:api:abc","exited":true,"exit_status":7,"is_held":true}
        ]"#,
    )?;
    let target = PaneTarget {
        session: SessionName::new("agent-terminal-project-owner".to_owned())?,
        pane_id: TerminalPaneId::new(0),
        title: "agent-terminal:api:abc".to_owned(),
    };

    let owned = find_owned_pane(&panes, &target).ok_or("owned terminal pane missing")?;
    assert!(!owned.is_plugin);
    assert!(owned.exited);
    assert_eq!(owned.exit_status, Some(7));
    Ok(())
}

#[test]
fn terminal_id_parser_rejects_noncanonical_output() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(TerminalPaneId::from_str("terminal_42")?.get(), 42);
    for invalid in ["0", "plugin_0", "terminal_-1", "terminal_1 extra"] {
        assert!(TerminalPaneId::from_str(invalid).is_err());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn dump_screen_requires_the_output_file() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let executable = write_script(temp.path(), "exit 0")?;
    let cli = ZellijCli::new(
        executable,
        temp.path().join("config.kdl"),
        Duration::from_millis(200),
    );
    let target = PaneTarget {
        session: SessionName::new("agent-terminal-project-owner".to_owned())?,
        pane_id: TerminalPaneId::new(1),
        title: "agent-terminal:api:abc".to_owned(),
    };

    assert!(matches!(
        cli.dump_screen(
            &target,
            &temp.path().join("screen.txt"),
            Duration::from_millis(200)
        ),
        Err(Error::ZellijFailed { .. })
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn subprocess_deadline_is_enforced() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let executable = write_script(temp.path(), "while :; do :; done")?;
    let cli = ZellijCli::new(
        executable,
        temp.path().join("config.kdl"),
        Duration::from_millis(20),
    );
    let session = SessionName::new("agent-terminal-project-owner".to_owned())?;

    assert!(matches!(
        cli.list_panes(&session, Duration::from_millis(20)),
        Err(Error::ZellijFailed { .. })
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn exited_parent_does_not_wait_for_descendant_pipe_holders()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let pid_file = temp.path().join("child.pid");
    let executable = write_script(
        temp.path(),
        &format!(
            "sleep 30 & printf '%s' \"$!\" > '{}'; exit 0",
            pid_file.display()
        ),
    )?;
    let cli = ZellijCli::new(
        executable,
        temp.path().join("config.kdl"),
        Duration::from_millis(100),
    );
    let session = SessionName::new("agent-terminal-project-owner".to_owned())?;
    let started = Instant::now();

    let result = cli.list_panes(&session, Duration::from_millis(100));
    if let Ok(pid) = fs::read_to_string(&pid_file) {
        let _status = Command::new("kill").arg(pid.trim()).status();
    }

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "reader blocked for {:?}",
        started.elapsed()
    );
    Ok(())
}

#[test]
fn bootstrap_files_are_private_and_owner_marked() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let project = temp.path().join("project");
    fs::create_dir_all(&project)?;
    let paths = ProjectPaths::new(&project, Some(&temp.path().join("state")))?;
    write_private_files(&paths, "0123456789abcdef")?;

    let config = fs::read_to_string(paths.config_file())?;
    let layout = fs::read_to_string(paths.layout_file())?;
    assert!(config.contains("show_release_notes false"));
    assert!(config.contains("session_serialization false"));
    assert!(layout.contains("agent-terminal:keeper:0123456789ab"));
    Ok(())
}

#[cfg(unix)]
fn write_script(parent: &Path, body: &str) -> Result<std::path::PathBuf, std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;

    let path = parent.join("fake-zellij");
    fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    Ok(path)
}
