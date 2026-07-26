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
    zellij::{PaneSnapshot, PaneSpec, PaneTarget, Zellij, ZellijCli, find_owned_pane, parse_panes},
};
use std::sync::Mutex;

use tempfile::TempDir;

static ZELLIJ_CONTRACT_LOCK: Mutex<()> = Mutex::new(());

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

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
fn session_exists_matches_complete_lines_only() -> TestResult {
    let _guard = ZELLIJ_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = TempDir::new()?;
    let (cli, _executable) = fake_cli(
        temp.path(),
        "printf '%s\\n' agent-terminal-project-owner-copy agent-terminal-project-owner",
    )?;
    let exact = SessionName::new("agent-terminal-project-owner".to_owned())?;
    let partial = SessionName::new("agent-terminal-project".to_owned())?;

    assert!(cli.session_exists(&exact, Duration::from_millis(200))?);
    assert!(!cli.session_exists(&partial, Duration::from_millis(200))?);
    Ok(())
}

#[cfg(unix)]
#[test]
fn no_active_sessions_message_maps_to_false() -> TestResult {
    let _guard = ZELLIJ_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = TempDir::new()?;
    let (cli, _executable) = fake_cli(
        temp.path(),
        "printf '%s\\n' 'No active zellij sessions found.' >&2; exit 1",
    )?;
    let session = SessionName::new("agent-terminal-project-owner".to_owned())?;

    assert!(!cli.session_exists(&session, Duration::from_millis(200))?);
    Ok(())
}

#[cfg(unix)]
#[test]
fn unexpected_list_sessions_failure_is_typed() -> TestResult {
    let _guard = ZELLIJ_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = TempDir::new()?;
    let (cli, _executable) = fake_cli(
        temp.path(),
        "printf '%s\\n' 'adapter unavailable' >&2; exit 23",
    )?;
    let session = SessionName::new("agent-terminal-project-owner".to_owned())?;

    let result = cli.session_exists(&session, Duration::from_millis(200));

    assert!(matches!(
        result,
        Err(Error::ZellijFailed { message }) if message == "adapter unavailable"
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn create_background_passes_exact_bootstrap_arguments() -> TestResult {
    let _guard = ZELLIJ_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = TempDir::new()?;
    let (cli, executable) = fake_cli(temp.path(), "exit 0")?;
    let config = temp.path().join("config.kdl");
    let layout = temp.path().join("layout with spaces.kdl");
    let session = SessionName::new("agent-terminal-project-owner".to_owned())?;

    cli.create_background(&session, &layout, Duration::from_millis(200))?;

    assert_eq!(
        read_arguments(&executable)?,
        vec![
            "--config".to_owned(),
            config.to_string_lossy().into_owned(),
            "attach".to_owned(),
            "--create-background".to_owned(),
            session.as_str().to_owned(),
            "options".to_owned(),
            "--default-layout".to_owned(),
            layout.to_string_lossy().into_owned(),
            "--session-serialization".to_owned(),
            "false".to_owned(),
            "--show-release-notes".to_owned(),
            "false".to_owned(),
            "--show-startup-tips".to_owned(),
            "false".to_owned(),
        ]
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn list_panes_parses_every_snapshot_field() -> TestResult {
    let _guard = ZELLIJ_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = TempDir::new()?;
    let (cli, _executable) = fake_cli(
        temp.path(),
        r#"printf '%s\n' '[{"id":17,"is_plugin":true,"title":"status pane","exited":true,"exit_status":137,"is_held":true}]'"#,
    )?;
    let session = SessionName::new("agent-terminal-project-owner".to_owned())?;

    let panes = cli.list_panes(&session, Duration::from_millis(200))?;

    assert_eq!(
        panes,
        vec![PaneSnapshot {
            id: 17,
            is_plugin: true,
            title: "status pane".to_owned(),
            exited: true,
            exit_status: Some(137),
            is_held: true,
        }]
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn list_panes_rejects_malformed_or_incomplete_json() -> TestResult {
    let _guard = ZELLIJ_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for output in ["not-json", r#"[{"id":1}]"#] {
        let temp = TempDir::new()?;
        let body = format!("printf '%s\\n' '{output}'");
        let (cli, _executable) = fake_cli(temp.path(), &body)?;
        let session = SessionName::new("agent-terminal-project-owner".to_owned())?;

        assert!(matches!(
            cli.list_panes(&session, Duration::from_millis(200)),
            Err(Error::ZellijFailed { message }) if message.starts_with("invalid list-panes JSON:")
        ));
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn create_pane_rejects_invalid_id_output() -> TestResult {
    let _guard = ZELLIJ_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = TempDir::new()?;
    let (cli, _executable) = fake_cli(temp.path(), "printf '%s\\n' 'terminal_7 extra'")?;
    let spec = PaneSpec {
        session: SessionName::new("agent-terminal-project-owner".to_owned())?,
        cwd: temp.path().to_path_buf(),
        title: "agent-terminal:api:owner".to_owned(),
        command: vec!["true".to_owned()],
    };

    assert!(matches!(
        cli.create_pane(&spec, Duration::from_millis(200)),
        Err(Error::InvalidInput { message }) if message.contains("terminal_7 extra")
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn dump_screen_requires_the_output_file() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = ZELLIJ_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    let _guard = ZELLIJ_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    let _guard = ZELLIJ_CONTRACT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    fs::write(path.with_extension("args"), [])?;
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nfor argument in \"$@\"; do\n  printf '%s\\000' \"$argument\" >> \"$0.args\"\ndone\n{body}\n"
        ),
    )?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

#[cfg(unix)]
fn fake_cli(parent: &Path, body: &str) -> Result<(ZellijCli, std::path::PathBuf), std::io::Error> {
    let executable = write_script(parent, body)?;
    let cli = ZellijCli::new(
        executable.clone(),
        parent.join("config.kdl"),
        Duration::from_secs(2),
    );
    Ok((cli, executable))
}

#[cfg(unix)]
fn read_arguments(executable: &Path) -> TestResult<Vec<String>> {
    let bytes = fs::read(executable.with_extension("args"))?;
    let mut arguments = bytes
        .split(|byte| *byte == 0)
        .map(|argument| String::from_utf8(argument.to_vec()))
        .collect::<Result<Vec<_>, _>>()?;
    if arguments.last().is_some_and(String::is_empty) {
        arguments.pop();
    }
    Ok(arguments)
}
