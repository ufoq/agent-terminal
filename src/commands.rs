use std::{
    fs::{self, File},
    io::Write as _,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use fs2::FileExt as _;

use crate::{
    config::write_session_files,
    domain::{JobName, Key, bound_screen, normalize_screen},
    error::Error,
    output::{
        CommandData, JobSummary, ListData, PressData, ReadData, SendData, StartData, StopData,
    },
    owner::{self, OwnerChain},
    paths::{self, ProjectPaths, Scope, ensure_private_dir},
    sweep,
    zellij::{PaneSnapshot, PaneSpec, ZellijCli, find_job, job_panes, unknown_panes},
};

const LOCK_RETRY: Duration = Duration::from_millis(50);
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const START_WAIT: Duration = Duration::from_secs(5);
const SCREEN_LINES: usize = 200;
const SCREEN_BYTES: usize = 32 * 1024;

pub struct Context {
    pub paths: ProjectPaths,
    pub zellij: ZellijCli,
    pub owner: Option<OwnerChain>,
}

pub fn build_context(
    project: Option<&Path>,
    state_dir: Option<&Path>,
    global: bool,
) -> Result<Context, Error> {
    let cwd = std::env::current_dir().map_err(|source| Error::StateIo {
        action: "read current directory",
        path: PathBuf::from("."),
        source,
        cause: "current directory is unavailable".to_owned(),
    })?;
    let start = project.map_or(cwd, std::path::Path::to_path_buf);
    let project_root = paths::find_project_root(&start)?;
    let runtime = paths::runtime_root(state_dir)?;
    let (scope, owner) = if global {
        (Scope::Global, None)
    } else {
        let chain = owner::discover_current()?;
        (Scope::Owner(chain.nearest.key()), Some(chain))
    };
    let project_paths = ProjectPaths::new(&project_root, &runtime, scope)?;
    let zellij = ZellijCli::new(
        find_zellij()?,
        project_paths.clone(),
        Duration::from_secs(30),
    );
    Ok(Context {
        paths: project_paths,
        zellij,
        owner,
    })
}

pub fn acquire_lock(paths: &ProjectPaths) -> Result<File, Error> {
    let lock = paths.lock_file();
    if let Some(parent) = lock.parent() {
        ensure_private_dir(parent)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock)
        .map_err(|source| Error::state_io("open project lock", &lock, source))?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        if file.try_lock_exclusive().is_ok() {
            return Ok(file);
        }
        if Instant::now() >= deadline {
            return Err(Error::LockBusy);
        }
        thread::sleep(LOCK_RETRY);
    }
}

fn find_zellij() -> Result<PathBuf, Error> {
    if let Some(value) = std::env::var_os("AGENT_TERMINAL_ZELLIJ") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return path
                .canonicalize()
                .map_err(|source| Error::state_io("resolve zellij executable", &path, source));
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("zellij");
            if candidate.is_file() {
                return candidate.canonicalize().map_err(|source| {
                    Error::state_io("resolve zellij executable", &candidate, source)
                });
            }
        }
    }
    Err(Error::ZellijNotFound {
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "zellij not found"),
    })
}

pub fn list(context: &Context, session_lost: bool) -> Result<CommandData, Error> {
    let jobs: Vec<JobSummary> = current_panes(context)?
        .iter()
        .filter_map(PaneSnapshot::job_name)
        .map(|name| JobSummary { name })
        .collect();
    let session = (session_lost && jobs.is_empty()).then_some("lost");
    Ok(CommandData::List(ListData { jobs, session }))
}

fn current_panes(context: &Context) -> Result<Vec<PaneSnapshot>, Error> {
    if !context.paths.project_dir().is_dir() {
        return Ok(Vec::new());
    }
    if !context
        .zellij
        .session_exists(context.zellij.command_timeout())?
    {
        return Ok(Vec::new());
    }
    context.zellij.list_panes(context.zellij.command_timeout())
}

pub fn start(
    context: &Context,
    job: JobName,
    cwd: &Path,
    command: &[String],
) -> Result<CommandData, Error> {
    if command.is_empty() {
        return Err(Error::InvalidInput {
            message: "start requires a command after --".to_owned(),
        });
    }
    let cwd = cwd
        .canonicalize()
        .map_err(|source| Error::state_io("canonicalize job cwd", cwd, source))?;
    ensure_session(context)?;
    let panes = context
        .zellij
        .list_panes(context.zellij.command_timeout())?;
    if find_job(&panes, &job).is_some() {
        return Err(Error::JobExists {
            job: job.as_str().to_owned(),
        });
    }
    let spec = PaneSpec {
        session: context.zellij.session(),
        cwd,
        title: job.pane_title(),
        command: command.to_vec(),
        close_on_exit: true,
    };
    context
        .zellij
        .create_pane(&spec, context.zellij.command_timeout())?;
    Ok(CommandData::Start(StartData { job }))
}

pub fn read(
    context: &Context,
    job: &JobName,
    session_lost: bool,
    save: Option<&Path>,
) -> Result<CommandData, Error> {
    let panes = current_panes(context)?;
    let pane = find_job(&panes, job).ok_or_else(|| missing_job(job, session_lost))?;
    let scratch = context.paths.scratch_dir();
    ensure_private_dir(&scratch)?;
    let path = scratch.join(format!("screen.{}", std::process::id()));
    context
        .zellij
        .dump_screen(pane.id, &path, context.zellij.command_timeout())?;
    let raw = fs::read_to_string(&path)
        .map_err(|source| Error::state_io("read captured screen", &path, source))?;
    let _ = fs::remove_file(&path);
    let screen = normalize_screen(&raw);
    let saved = save
        .map(|path| save_screen(path, &screen, context.paths.runtime_root()))
        .transpose()?;
    let (saved, saved_bytes, saved_lines) = saved.map_or((None, None, None), |saved| {
        (Some(saved.path), Some(saved.bytes), Some(saved.lines))
    });
    let bounded = bound_screen(&screen, SCREEN_LINES, SCREEN_BYTES);
    Ok(CommandData::Read(ReadData {
        job: job.clone(),
        screen: bounded.screen,
        truncated: bounded.truncated,
        saved,
        saved_bytes,
        saved_lines,
    }))
}

#[derive(Debug)]
struct SavedScreen {
    path: String,
    bytes: u64,
    lines: usize,
}

#[cfg(test)]
thread_local! {
    // Per-thread so parallel unit tests cannot trip each other's injection.
    static INJECT_REPLACE_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn path_ends_with_dot_component(path: &Path) -> bool {
    let raw = path.as_os_str().as_encoded_bytes();
    raw == b"." || raw == b".." || raw.ends_with(b"/.") || raw.ends_with(b"/..")
}

fn save_screen(path: &Path, screen: &str, runtime_root: &Path) -> Result<SavedScreen, Error> {
    if path.as_os_str().is_empty() {
        return Err(invalid_path("saved screen path must not be empty"));
    }
    if path.as_os_str().as_encoded_bytes().last() == Some(&b'/') {
        return Err(invalid_path("saved screen path must not end with '/'"));
    }
    if path_ends_with_dot_component(path) {
        return Err(invalid_path("saved screen path must name a file"));
    }
    if path.to_str().is_none() {
        return Err(invalid_path("saved screen path must be valid UTF-8"));
    }

    let absolute = std::path::absolute(path).map_err(|source| Error::save_failed(path, source))?;
    if absolute.starts_with(runtime_root) {
        return Err(invalid_path(
            "saved screen path must not be inside the runtime state directory",
        ));
    }
    let Some(file_name) = absolute.file_name() else {
        return Err(invalid_path("saved screen path must name a file"));
    };
    let Some(parent) = absolute.parent() else {
        return Err(invalid_path("saved screen path must name a file"));
    };
    let parent = fs::canonicalize(parent).map_err(|source| Error::save_failed(path, source))?;
    let destination = parent.join(file_name);

    if fs::symlink_metadata(&destination).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return Err(invalid_path(
            "saved screen path must name a file, not a directory",
        ));
    }
    if destination.starts_with(runtime_root) {
        return Err(invalid_path(
            "saved screen path must not be inside the runtime state directory",
        ));
    }
    let Some(destination_text) = destination.to_str() else {
        return Err(invalid_path("saved screen path must be valid UTF-8"));
    };

    let mut file = tempfile::NamedTempFile::new_in(&parent)
        .map_err(|source| Error::save_failed(path, source))?;
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|source| Error::save_failed(path, source))?;
    file.write_all(screen.as_bytes())
        .map_err(|source| Error::save_failed(path, source))?;
    file.flush()
        .map_err(|source| Error::save_failed(path, source))?;

    #[cfg(test)]
    if INJECT_REPLACE_FAILURE.with(|flag| flag.replace(false)) {
        return Err(Error::save_failed(
            path,
            std::io::Error::other("injected replacement failure"),
        ));
    }

    file.persist(&destination)
        .map_err(|error| Error::save_failed(path, error.error))?;

    Ok(SavedScreen {
        path: destination_text.to_owned(),
        bytes: u64::try_from(screen.len()).unwrap_or(u64::MAX),
        lines: screen.lines().count(),
    })
}

fn invalid_path(message: &str) -> Error {
    Error::InvalidInput {
        message: message.to_owned(),
    }
}

pub fn send(
    context: &Context,
    job: &JobName,
    text: &str,
    session_lost: bool,
) -> Result<CommandData, Error> {
    let pane = require_job(context, job, session_lost)?;
    context
        .zellij
        .paste(pane.id, text, context.zellij.command_timeout())?;
    context.zellij.send_keys(
        pane.id,
        &["Enter".to_owned()],
        context.zellij.command_timeout(),
    )?;
    Ok(CommandData::Send(SendData { job: job.clone() }))
}

pub fn press(
    context: &Context,
    job: &JobName,
    key: &Key,
    session_lost: bool,
) -> Result<CommandData, Error> {
    let pane = require_job(context, job, session_lost)?;
    context.zellij.send_keys(
        pane.id,
        &[key.zellij_token().to_owned()],
        context.zellij.command_timeout(),
    )?;
    Ok(CommandData::Press(PressData { job: job.clone() }))
}

pub fn stop(context: &Context, job: &JobName) -> Result<CommandData, Error> {
    let panes = current_panes(context)?;
    if let Some(pane) = find_job(&panes, job) {
        context
            .zellij
            .close_pane(pane.id, context.zellij.command_timeout())?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut panes = current_panes(context)?;
        while find_job(&panes, job).is_some() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
            panes = current_panes(context)?;
        }
        if find_job(&panes, job).is_some() {
            return Err(Error::ZellijFailed {
                message: "job pane did not close".to_owned(),
            });
        }
    }
    finish_idle(context)?;
    Ok(CommandData::Stop(StopData { job: job.clone() }))
}

fn finish_idle(context: &Context) -> Result<(), Error> {
    let panes = current_panes(context)?;
    if !job_panes(&panes).is_empty() || !unknown_panes(&panes).is_empty() {
        return Ok(());
    }
    if context
        .zellij
        .session_exists(context.zellij.command_timeout())?
        && !crate::sweep::shutdown(&context.zellij.session(), &context.zellij)?
    {
        return Err(Error::ZellijFailed {
            message: "terminal session did not shut down".to_owned(),
        });
    }
    crate::sweep::remove_project(&context.paths)
}

fn require_job(
    context: &Context,
    job: &JobName,
    session_lost: bool,
) -> Result<PaneSnapshot, Error> {
    let panes = current_panes(context)?;
    find_job(&panes, job)
        .cloned()
        .ok_or_else(|| missing_job(job, session_lost))
}

fn missing_job(job: &JobName, session_lost: bool) -> Error {
    if session_lost {
        Error::SessionLost
    } else {
        Error::JobNotFound {
            job: job.as_str().to_owned(),
        }
    }
}

fn session_is_absent(context: &Context) -> Result<bool, Error> {
    if !context.paths.project_dir().is_dir() {
        return Ok(false);
    }
    Ok(!context
        .zellij
        .session_exists(context.zellij.command_timeout())?)
}

fn ensure_session(context: &Context) -> Result<(), Error> {
    if !context.paths.project_dir().is_dir() {
        prepare_session_files(context)?;
    }
    let exists = context
        .zellij
        .session_exists(context.zellij.command_timeout())?;
    if exists && keeper_is_healthy(context, false)? {
        return Ok(());
    }
    if exists {
        let panes = context
            .zellij
            .list_panes(context.zellij.command_timeout())?;
        if !unknown_panes(&panes).is_empty() {
            return Err(Error::ZellijFailed {
                message: "terminal session contains panes owned by another process".to_owned(),
            });
        }
        if !crate::sweep::shutdown(&context.zellij.session(), &context.zellij)? {
            return Err(Error::ZellijFailed {
                message: "terminal session did not shut down".to_owned(),
            });
        }
        crate::sweep::remove_project(&context.paths)?;
    }
    prepare_session_files(context)?;
    context
        .zellij
        .create_background(context.zellij.command_timeout())?;
    let deadline = Instant::now() + START_WAIT;
    while Instant::now() < deadline {
        if keeper_is_healthy(context, true)? {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    if !crate::sweep::shutdown(&context.zellij.session(), &context.zellij)? {
        return Err(Error::ZellijFailed {
            message: "terminal session did not shut down".to_owned(),
        });
    }
    crate::sweep::remove_project(&context.paths)?;
    Err(Error::ZellijFailed {
        message: "terminal session did not become ready".to_owned(),
    })
}

fn keeper_is_healthy(context: &Context, require_ready: bool) -> Result<bool, Error> {
    let panes = context
        .zellij
        .list_panes(context.zellij.command_timeout())?;
    let Some(keeper) = panes.iter().find(|pane| pane.is_keeper()) else {
        return Ok(false);
    };
    if context.paths.scope().is_global() || !require_ready {
        return Ok(true);
    }
    let Ok(ready) = fs::read_to_string(context.paths.ready_file()) else {
        return Ok(false);
    };
    Ok(ready.trim().parse::<u32>() == Ok(keeper.id))
}

fn prepare_session_files(context: &Context) -> Result<(), Error> {
    let _metadata = context.paths.metadata_lock()?;
    let zellij = find_zellij()?;
    write_session_files(
        &context.paths,
        &std::env::current_exe().map_err(|source| {
            Error::state_io(
                "resolve agent-terminal executable",
                Path::new("agent-terminal"),
                source,
            )
        })?,
        &zellij,
    )?;
    if let Some(owner) = &context.owner {
        owner::write_owners(&context.paths.owners_file(), owner)?;
    }
    let _ = fs::remove_file(context.paths.ready_file());
    Ok(())
}

pub fn run_command(
    context: &Context,
    command: crate::cli::CliCommand,
) -> Result<CommandData, Error> {
    let _lock = acquire_lock(&context.paths)?;
    let observe_absence = matches!(
        &command,
        crate::cli::CliCommand::List
            | crate::cli::CliCommand::Read { .. }
            | crate::cli::CliCommand::Send { .. }
            | crate::cli::CliCommand::Press { .. }
    );
    let session_lost = observe_absence && session_is_absent(context)?;
    if !(session_lost && matches!(command, crate::cli::CliCommand::List)) {
        sweep::sweep(&context.paths, &context.zellij)?;
    }
    match command {
        crate::cli::CliCommand::List => list(context, session_lost),
        crate::cli::CliCommand::Start { job, cwd, command } => start(
            context,
            job,
            cwd.as_deref().unwrap_or_else(|| Path::new(".")),
            &command,
        ),
        crate::cli::CliCommand::Read { job, save } => {
            read(context, &job, session_lost, save.as_deref())
        }
        crate::cli::CliCommand::Send { job, text } => send(context, &job, &text, session_lost),
        crate::cli::CliCommand::Press { job, key } => press(context, &job, &key, session_lost),
        crate::cli::CliCommand::Stop { job } => stop(context, &job),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods)]

    use std::{
        ffi::OsString,
        os::unix::{
            ffi::OsStringExt as _,
            fs::{MetadataExt as _, PermissionsExt as _, symlink},
        },
    };

    use tempfile::TempDir;

    use super::{INJECT_REPLACE_FAILURE, save_screen};

    fn canonical(path: &std::path::Path) -> std::path::PathBuf {
        std::fs::canonicalize(path).unwrap()
    }

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn saves_multibyte_text_byte_for_byte_with_metadata() {
        let dir = TempDir::new().unwrap();
        let runtime = TempDir::new().unwrap();
        let path = canonical(dir.path()).join("snap.log");
        let screen = "alpha\nβeta\n界";

        let saved = save_screen(&path, screen, &canonical(runtime.path())).unwrap();

        assert_eq!(saved.path, path.to_str().unwrap());
        assert_eq!(saved.bytes, u64::try_from(screen.len()).unwrap());
        assert_eq!(saved.lines, 3);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), screen);
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn empty_snapshot_has_zero_metadata() {
        let dir = TempDir::new().unwrap();
        let runtime = TempDir::new().unwrap();
        let path = canonical(dir.path()).join("empty.log");

        let saved = save_screen(&path, "", &canonical(runtime.path())).unwrap();

        assert_eq!(saved.bytes, 0);
        assert_eq!(saved.lines, 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"");
    }

    #[test]
    fn replaces_existing_file_and_tightens_mode() {
        let dir = TempDir::new().unwrap();
        let runtime = TempDir::new().unwrap();
        let path = canonical(dir.path()).join("snap.log");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        save_screen(&path, "new\n", &canonical(runtime.path())).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn replaces_symlink_entry_without_touching_target() {
        let dir = TempDir::new().unwrap();
        let runtime = TempDir::new().unwrap();
        let target = canonical(dir.path()).join("target.log");
        let link = canonical(dir.path()).join("link.log");
        std::fs::write(&target, "target").unwrap();
        symlink(&target, &link).unwrap();

        save_screen(&link, "link content", &canonical(runtime.path())).unwrap();

        assert!(
            !std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&link).unwrap(), "link content");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "target");
    }

    #[test]
    fn missing_parent_is_actionable_and_creates_nothing() {
        let dir = TempDir::new().unwrap();
        let runtime = TempDir::new().unwrap();
        let path = canonical(dir.path()).join("missing").join("snap.log");

        let error = save_screen(&path, "x", &canonical(runtime.path())).unwrap_err();

        assert_eq!(error.kind(), "state_io");
        assert!(error.public_message().contains("snap.log"));
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn pre_rename_failure_preserves_existing_file_and_cleans_temp() {
        let dir = TempDir::new().unwrap();
        let runtime = TempDir::new().unwrap();
        let path = canonical(dir.path()).join("snap.log");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        INJECT_REPLACE_FAILURE.with(|flag| flag.set(true));

        let error = save_screen(&path, "new", &canonical(runtime.path())).unwrap_err();

        assert_eq!(error.kind(), "state_io");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
        assert_eq!(mode(&path), 0o640);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "temporary file was not removed: {names:?}");
    }

    #[test]
    fn read_only_parent_preserves_existing_file() {
        if std::fs::metadata("/proc/self").unwrap().uid() == 0 {
            eprintln!("skipping: running as root");
            return;
        }
        let dir = TempDir::new().unwrap();
        let runtime = TempDir::new().unwrap();
        let path = canonical(dir.path()).join("snap.log");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();

        let result = save_screen(&path, "new", &canonical(runtime.path()));

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let error = result.unwrap_err();
        assert_eq!(error.kind(), "state_io");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
    }

    #[test]
    fn rejects_invalid_destination_paths_before_writing() {
        let dir = TempDir::new().unwrap();
        let runtime = TempDir::new().unwrap();
        let root = canonical(dir.path());
        let runtime_root = canonical(runtime.path());
        let subdir = root.join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        let locks = runtime_root.join("locks");
        std::fs::create_dir(&locks).unwrap();
        let victim = root.join("victim.log");
        std::fs::write(&victim, "keep").unwrap();

        for path in [
            root.join(""),
            root.join("file/"),
            root.join(".."),
            victim.join("."),
            victim.join(".."),
            subdir,
            locks.join("project.lock"),
            runtime_root.join("missing").join("snap.log"),
        ] {
            let error = save_screen(&path, "x", &runtime_root).unwrap_err();
            assert_eq!(error.kind(), "invalid_input", "path {path:?}");
        }
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");

        let bad_name = OsString::from_vec(vec![b'b', 0xFF]);
        let error = save_screen(&root.join(&bad_name), "x", &runtime_root).unwrap_err();
        assert_eq!(error.kind(), "invalid_input");
        let error =
            save_screen(&root.join(bad_name).join("missing"), "x", &runtime_root).unwrap_err();
        assert_eq!(error.kind(), "invalid_input");

        let bad_parent = root.join(OsString::from_vec(vec![b'd', 0xFF]));
        std::fs::create_dir(&bad_parent).unwrap();
        let link = root.join("bad-link");
        symlink(&bad_parent, &link).unwrap();
        let error = save_screen(&link.join("snap.log"), "x", &runtime_root).unwrap_err();
        assert_eq!(error.kind(), "invalid_input");
        assert!(!link.join("snap.log").exists());
    }
}
