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
    pub fn stop(&self, job: &JobName) -> Result<StopData, Error> {
        let deadline = Deadline::after(STOP_DEADLINE);
        let (mut locked, mut registry) = self.open()?;
        let original = registry
            .jobs
            .get(job)
            .cloned()
            .ok_or_else(|| Error::JobNotFound {
                job: job.to_string(),
            })?;
        if let JobRecord::PendingRemove(pending) = original {
            self.finish_remove(&mut locked, &mut registry, job, &pending.job, deadline)?;
            return Ok(StopData);
        }
        if matches!(original, JobRecord::PendingStart(_))
            && let Err(Error::PendingStartAbsent { .. }) =
                self.reconcile_job(&mut locked, &mut registry, job, deadline)
        {
            if registry.jobs.len() == 1 {
                self.delete_owned_empty_session(&registry, deadline)?;
            }
            registry.jobs.remove(job);
            locked.save(&registry)?;
            return Ok(StopData);
        }
        self.reconcile_job(&mut locked, &mut registry, job, deadline)?;
        let active = Self::active_from(&registry, job)?;
        let pane = self.live_pane(&registry, &active, deadline)?;
        if pane.as_ref().is_some_and(|snapshot| !snapshot.exited) {
            let target = Self::target(&registry, &active);
            if let Err(error) = self.zellij.send_keys(
                &target,
                &["Ctrl c".to_owned()],
                deadline.timeout(self.zellij.command_timeout())?,
            ) && !matches!(error, Error::ZellijTimeout)
            {
                return Err(error);
            }
            let _ = self.wait_for_exit(&registry, &active, deadline)?;
        }
        registry.jobs.insert(
            job.clone(),
            JobRecord::PendingRemove(PendingRemove {
                job: active.clone(),
            }),
        );
        locked.save(&registry)?;
        self.finish_remove(&mut locked, &mut registry, job, &active, deadline)?;
        Ok(StopData)
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
                Err(error) => {
                    if matches!(error, Error::ZellijFailed { .. } | Error::ZellijTimeout) {
                        return Ok(last_running);
                    }
                    return Err(error);
                }
            }
            if deadline.timeout(self.zellij.command_timeout()).is_err() {
                return Ok(last_running);
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}
