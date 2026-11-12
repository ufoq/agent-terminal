use std::{
    fs::{self, File},
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

pub fn read(context: &Context, job: &JobName, session_lost: bool) -> Result<CommandData, Error> {
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
    let bounded = bound_screen(&screen, SCREEN_LINES, SCREEN_BYTES);
    Ok(CommandData::Read(ReadData {
        job: job.clone(),
        screen: bounded.screen,
        truncated: bounded.truncated,
    }))
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
        crate::cli::CliCommand::Read { job } => read(context, &job, session_lost),
        crate::cli::CliCommand::Send { job, text } => send(context, &job, &text, session_lost),
        crate::cli::CliCommand::Press { job, key } => press(context, &job, &key, session_lost),
        crate::cli::CliCommand::Stop { job } => stop(context, &job),
    }
}
