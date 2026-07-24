use std::path::PathBuf;

use crate::{
    controller::{Controller, Deadline, START_DEADLINE},
    domain::JobName,
    error::Error,
    output::StartData,
    state::{ActiveJob, JobRecord, PendingStart},
    zellij::{PaneSpec, Zellij},
};

impl<Z: Zellij> Controller<Z> {
    pub fn start(
        &self,
        job: JobName,
        cwd: Option<PathBuf>,
        command: Vec<String>,
    ) -> Result<StartData, Error> {
        if command.is_empty() {
            return Err(Error::InvalidInput {
                message: "start requires a command after --".to_owned(),
            });
        }
        let cwd = canonical_cwd(self.store.paths().project_root(), cwd)?;
        let (mut locked, mut registry) = self.open()?;
        if registry.jobs.contains_key(&job) {
            self.reconcile_job(&mut locked, &mut registry, &job, Deadline::per_call())?;
        }
        if registry.jobs.contains_key(&job) {
            return Err(Error::JobExists {
                job: job.to_string(),
            });
        }
        let pending = PendingStart::for_job(&job, cwd.clone(), command.clone());
        registry
            .jobs
            .insert(job.clone(), JobRecord::PendingStart(pending.clone()));
        locked.save(&registry)?;
        self.ensure_session(&registry)?;
        let pane_id = self.zellij.create_pane(
            &PaneSpec {
                session: registry.session.clone(),
                cwd,
                title: pending.title.clone(),
                command,
            },
            self.zellij.command_timeout(),
        )?;
        let mut pending_with_id = pending;
        pending_with_id.pane_id = Some(pane_id);
        registry.jobs.insert(
            job.clone(),
            JobRecord::PendingStart(pending_with_id.clone()),
        );
        locked.save(&registry)?;
        let active = ActiveJob::from_pending(pending_with_id, pane_id);
        let pane = self.wait_for_target(&registry, &active, START_DEADLINE)?;
        registry.jobs.insert(job.clone(), JobRecord::Active(active));
        locked.save(&registry)?;
        let (state, exit_code) = Self::pane_state(pane.as_ref());
        Ok(StartData {
            job,
            state,
            exit_code,
        })
    }
}

fn canonical_cwd(project_root: &std::path::Path, cwd: Option<PathBuf>) -> Result<PathBuf, Error> {
    let candidate = match cwd {
        Some(path) if path.is_absolute() => path,
        Some(path) => project_root.join(path),
        None => project_root.to_path_buf(),
    };
    let canonical = candidate.canonicalize().map_err(|source| Error::StateIo {
        action: "canonicalize job working directory",
        path: candidate,
        source,
    })?;
    if !canonical.is_dir() {
        return Err(Error::InvalidInput {
            message: format!(
                "job working directory is not a directory: {}",
                canonical.display()
            ),
        });
    }
    Ok(canonical)
}
