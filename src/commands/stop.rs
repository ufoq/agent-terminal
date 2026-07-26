use std::thread;

use crate::{
    controller::{Controller, Deadline, POLL_INTERVAL, STOP_DEADLINE, STOP_GRACE},
    domain::JobName,
    error::Error,
    output::StopData,
    state::{JobRecord, PendingRemove},
    zellij::Zellij,
};

impl<Z: Zellij> Controller<Z> {
    pub fn stop(&self, job: JobName, force: bool) -> Result<StopData, Error> {
        let deadline = Deadline::after(STOP_DEADLINE);
        let (mut locked, mut registry) = self.open()?;
        let original = registry
            .jobs
            .get(&job)
            .cloned()
            .ok_or_else(|| Error::JobNotFound {
                job: job.to_string(),
            })?;
        if let JobRecord::PendingRemove(pending) = original {
            let screen = self
                .live_pane(&registry, &pending.job, deadline)?
                .and_then(|_| self.capture_screen(&registry, &pending.job, deadline).ok());
            let forced = self.finish_remove(
                &mut locked,
                &mut registry,
                &job,
                &pending.job,
                pending.force_authorized,
                deadline,
            )?;
            return Ok(cleaned(job, forced, screen));
        }
        if matches!(original, JobRecord::PendingStart(_))
            && let Err(Error::PendingStartAbsent { .. }) =
                self.reconcile_job(&mut locked, &mut registry, &job, deadline)
        {
            if registry.jobs.len() == 1 {
                self.delete_owned_empty_session(&registry, deadline)?;
            }
            registry.jobs.remove(&job);
            locked.save(&registry)?;
            return Ok(cleaned(job, false, None));
        }
        self.reconcile_job(&mut locked, &mut registry, &job, deadline)?;
        let active = Self::active_from(&registry, &job)?;
        let mut pane = self.live_pane(&registry, &active, deadline)?;
        let forced = force && pane.as_ref().is_some_and(|snapshot| !snapshot.exited);
        if !force && pane.as_ref().is_some_and(|snapshot| !snapshot.exited) {
            let target = Self::target(&registry, &active);
            self.zellij.send_keys(
                &target,
                &["Ctrl c".to_owned()],
                deadline.timeout(self.zellij.command_timeout())?,
            )?;
            pane = self.wait_for_exit(&registry, &active, deadline)?;
            if pane.as_ref().is_some_and(|snapshot| !snapshot.exited) {
                return Err(Error::JobStillRunning {
                    job: job.to_string(),
                });
            }
        }
        let screen =
            pane.as_ref().and_then(
                |_| match self.capture_screen(&registry, &active, deadline) {
                    Ok(screen) => Some(screen),
                    Err(error) => {
                        tracing::warn!(%error, %job, "final screen was unavailable during stop");
                        None
                    }
                },
            );
        registry.jobs.insert(
            job.clone(),
            JobRecord::PendingRemove(PendingRemove {
                job: active.clone(),
                force_authorized: force,
            }),
        );
        locked.save(&registry)?;
        let actually_forced =
            self.finish_remove(&mut locked, &mut registry, &job, &active, force, deadline)?;
        Ok(cleaned(job, forced || actually_forced, screen))
    }

    fn wait_for_exit(
        &self,
        registry: &crate::state::Registry,
        active: &crate::state::ActiveJob,
        operation_deadline: Deadline,
    ) -> Result<Option<crate::zellij::PaneSnapshot>, Error> {
        let deadline = operation_deadline.cap_after(STOP_GRACE);
        let mut last_running: Option<crate::zellij::PaneSnapshot> = None;
        loop {
            match self.live_pane(registry, active, deadline) {
                Ok(pane) => {
                    if pane.as_ref().is_none_or(|snapshot| snapshot.exited) {
                        return Ok(pane);
                    }
                    last_running = pane;
                }
                Err(Error::ZellijFailed { .. }) => {
                    return Ok(last_running);
                }
                Err(error) => return Err(error),
            }
            if deadline.timeout(self.zellij.command_timeout()).is_err() {
                return Ok(last_running);
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

fn cleaned(job: JobName, forced: bool, screen: Option<crate::domain::BoundedScreen>) -> StopData {
    StopData {
        job,
        cleaned_up: true,
        forced,
        screen_available: screen.is_some(),
        last_screen: screen.as_ref().map(|bounded| bounded.screen.clone()),
        truncated: screen.map(|bounded| bounded.truncated),
    }
}
