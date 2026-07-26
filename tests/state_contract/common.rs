use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use agent_terminal::{
    domain::{JobName, SessionName, TerminalPaneId},
    error::Error,
    paths::project_digest,
    state::{ActiveJob, JobRecord, PendingRemove, PendingStart, Registry},
};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub fn create_project(parent: &Path, name: &str) -> TestResult<PathBuf> {
    let project = parent.join(name);
    fs::create_dir_all(&project)?;
    Ok(project.canonicalize()?)
}

pub fn job(name: &str) -> Result<JobName, Error> {
    JobName::from_str(name)
}

pub fn session_for(project: &Path, nonce: &str) -> Result<SessionName, Error> {
    SessionName::new(format!(
        "agent-terminal-{}-{}",
        &project_digest(project)[..12],
        &nonce[..8]
    ))
}

pub fn registries_for_all_phases(project: &Path) -> TestResult<Vec<Registry>> {
    let job = job("worker")?;
    let pending = PendingStart::for_job(
        &job,
        project.to_path_buf(),
        vec!["sh".to_owned(), "-c".to_owned(), "run worker".to_owned()],
    );
    let active = ActiveJob::from_pending(pending.clone(), TerminalPaneId::new(41));
    let records = [
        JobRecord::PendingStart(pending),
        JobRecord::Active(active.clone()),
        JobRecord::PendingRemove(PendingRemove {
            job: active,
            force_authorized: true,
        }),
    ];

    records
        .into_iter()
        .map(|record| {
            let mut registry = Registry::new(project.to_path_buf())?;
            registry.jobs.insert(job.clone(), record);
            Ok(registry)
        })
        .collect::<Result<Vec<_>, Error>>()
        .map_err(Into::into)
}

pub fn only_record_mut(registry: &mut Registry) -> TestResult<&mut JobRecord> {
    registry
        .jobs
        .values_mut()
        .next()
        .ok_or_else(|| std::io::Error::other("phase fixture has no job").into())
}

pub const fn active_metadata_mut(
    record: &mut JobRecord,
) -> (&mut String, &mut PathBuf, &mut Vec<String>) {
    match record {
        JobRecord::PendingStart(pending) => {
            (&mut pending.title, &mut pending.cwd, &mut pending.command)
        }
        JobRecord::Active(active) => (&mut active.title, &mut active.cwd, &mut active.command),
        JobRecord::PendingRemove(pending) => (
            &mut pending.job.title,
            &mut pending.job.cwd,
            &mut pending.job.command,
        ),
    }
}

pub fn set_operation_nonce(record: &mut JobRecord, nonce: &str) {
    let title = format!("agent-terminal:worker:{}", &nonce[..12]);
    match record {
        JobRecord::PendingStart(pending) => {
            nonce.clone_into(&mut pending.operation_nonce);
            pending.title = title;
        }
        JobRecord::Active(active) => {
            nonce.clone_into(&mut active.operation_nonce);
            active.title = title;
        }
        JobRecord::PendingRemove(pending) => {
            nonce.clone_into(&mut pending.job.operation_nonce);
            pending.job.title = title;
        }
    }
}
