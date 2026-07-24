use std::thread;

use crate::{
    controller::{Controller, Deadline, POLL_INTERVAL, START_DEADLINE},
    domain::JobName,
    error::Error,
    state::{ActiveJob, JobRecord, LockedState, PendingStart, Registry, elapsed_since},
    zellij::{PaneSnapshot, Zellij},
};

impl<Z: Zellij> Controller<Z> {
    pub(crate) fn reconcile_job(
        &self,
        locked: &mut LockedState,
        registry: &mut Registry,
        job: &JobName,
        deadline: Deadline,
    ) -> Result<(), Error> {
        let record = registry
            .jobs
            .get(job)
            .cloned()
            .ok_or_else(|| Error::JobNotFound {
                job: job.to_string(),
            })?;
        match record {
            JobRecord::Active(_) => Ok(()),
            JobRecord::PendingStart(pending) => {
                let pane = self.adopt_pending(registry, &pending, deadline)?;
                match (pane, pending.pane_id) {
                    (Some(pane), _) => {
                        registry.jobs.insert(
                            job.clone(),
                            JobRecord::Active(ActiveJob::from_pending(
                                pending,
                                crate::domain::TerminalPaneId::new(pane.id),
                            )),
                        );
                        locked.save(registry)
                    }
                    (None, Some(pane_id)) => {
                        registry.jobs.insert(
                            job.clone(),
                            JobRecord::Active(ActiveJob::from_pending(pending, pane_id)),
                        );
                        locked.save(registry)
                    }
                    (None, None) => Err(Error::PendingStartAbsent {
                        job: job.to_string(),
                    }),
                }
            }
            JobRecord::PendingRemove(pending) => self
                .finish_remove(
                    locked,
                    registry,
                    job,
                    &pending.job,
                    pending.force_authorized,
                    deadline,
                )
                .map(|_| ()),
        }
    }

    fn adopt_pending(
        &self,
        registry: &Registry,
        pending: &PendingStart,
        operation_deadline: Deadline,
    ) -> Result<Option<PaneSnapshot>, Error> {
        let remaining = START_DEADLINE.saturating_sub(elapsed_since(pending.created_millis));
        let deadline = operation_deadline.cap_after(remaining);
        loop {
            let timeout = match deadline.timeout(self.zellij.command_timeout()) {
                Ok(timeout) => timeout,
                Err(_) if elapsed_since(pending.created_millis) >= START_DEADLINE => {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            if self.zellij.session_exists(&registry.session, timeout)? {
                let panes = self.zellij.list_panes(
                    &registry.session,
                    deadline.timeout(self.zellij.command_timeout())?,
                )?;
                let found = panes.into_iter().find(|pane| {
                    !pane.is_plugin
                        && pane.title == pending.title
                        && pending
                            .pane_id
                            .is_none_or(|pane_id| pane.id == pane_id.get())
                });
                if found.is_some() {
                    return Ok(found);
                }
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    pub(crate) fn finish_remove(
        &self,
        locked: &mut LockedState,
        registry: &mut Registry,
        job: &JobName,
        active: &ActiveJob,
        force_authorized: bool,
        deadline: Deadline,
    ) -> Result<bool, Error> {
        let target = Self::target(registry, active);
        let pane = self.live_pane(registry, active, deadline)?;
        let forced = pane.as_ref().is_some_and(|snapshot| !snapshot.exited);
        if forced && !force_authorized {
            return Err(Error::JobStillRunning {
                job: job.to_string(),
            });
        }
        if pane.is_some() {
            if let Err(error) = self
                .zellij
                .close_pane(&target, deadline.timeout(self.zellij.command_timeout())?)
                && self.live_pane(registry, active, deadline)?.is_some()
            {
                return Err(error);
            }
            self.verify_absent(registry, active, deadline)?;
        }
        if registry.jobs.len() == 1 && self.session_exists(registry, deadline)? {
            self.verify_owned_empty_session(registry, deadline)?;
            if let Err(error) = self.zellij.kill_session(
                &registry.session,
                deadline.timeout(self.zellij.command_timeout())?,
            ) && self.session_exists(registry, deadline)?
            {
                return Err(error);
            }
            self.verify_session_absent(registry, deadline)?;
        }
        registry.jobs.remove(job);
        locked.save(registry)?;
        Ok(forced)
    }

    pub(crate) fn delete_owned_empty_session(
        &self,
        registry: &Registry,
        deadline: Deadline,
    ) -> Result<(), Error> {
        if !self.session_exists(registry, deadline)? {
            return Ok(());
        }
        self.verify_owned_empty_session(registry, deadline)?;
        self.zellij.kill_session(
            &registry.session,
            deadline.timeout(self.zellij.command_timeout())?,
        )?;
        self.verify_session_absent(registry, deadline)
    }

    fn session_exists(&self, registry: &Registry, deadline: Deadline) -> Result<bool, Error> {
        self.zellij.session_exists(
            &registry.session,
            deadline.timeout(self.zellij.command_timeout())?,
        )
    }

    fn verify_owned_empty_session(
        &self,
        registry: &Registry,
        deadline: Deadline,
    ) -> Result<(), Error> {
        let expected = format!("agent-terminal:keeper:{}", &registry.owner_nonce[..12]);
        let panes = self.zellij.list_panes(
            &registry.session,
            deadline.timeout(self.zellij.command_timeout())?,
        )?;
        let terminals: Vec<_> = panes.iter().filter(|pane| !pane.is_plugin).collect();
        if terminals.len() != 1 || terminals[0].title != expected {
            return Err(Error::ZellijFailed {
                message: format!(
                    "refusing to delete session {} because keeper ownership is not exclusive",
                    registry.session
                ),
            });
        }
        Ok(())
    }

    fn verify_absent(
        &self,
        registry: &Registry,
        active: &ActiveJob,
        operation_deadline: Deadline,
    ) -> Result<(), Error> {
        let deadline = operation_deadline.cap_after(self.zellij.command_timeout());
        loop {
            if self.live_pane(registry, active, deadline)?.is_none() {
                return Ok(());
            }
            if deadline.timeout(self.zellij.command_timeout()).is_err() {
                return Err(Error::ZellijFailed {
                    message: format!("pane {} remained after close", active.pane_id),
                });
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn verify_session_absent(
        &self,
        registry: &Registry,
        operation_deadline: Deadline,
    ) -> Result<(), Error> {
        let deadline = operation_deadline.cap_after(self.zellij.command_timeout());
        loop {
            if !self.session_exists(registry, deadline)? {
                return Ok(());
            }
            if deadline.timeout(self.zellij.command_timeout()).is_err() {
                return Err(Error::ZellijFailed {
                    message: format!("session {} remained after deletion", registry.session),
                });
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}
