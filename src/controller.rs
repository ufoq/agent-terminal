use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use crate::{
    config::write_private_files,
    domain::{BoundedScreen, JobState, bound_screen, normalize_screen},
    error::Error,
    state::{ActiveJob, LockedState, Registry, StateStore},
    zellij::{PaneSnapshot, PaneTarget, Zellij, find_owned_pane},
};

pub(crate) const START_DEADLINE: Duration = Duration::from_secs(10);
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(5);
pub(crate) const STOP_DEADLINE: Duration = Duration::from_secs(15);
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);
const SCREEN_LINES: usize = 200;
const SCREEN_BYTES: usize = 32 * 1024;
static DUMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct Controller<Z> {
    pub(crate) store: StateStore,
    pub(crate) zellij: Z,
}

#[derive(Clone, Copy)]
pub(crate) struct Deadline {
    end: Option<Instant>,
}

impl Deadline {
    pub(crate) const fn per_call() -> Self {
        Self { end: None }
    }

    pub(crate) fn after(duration: Duration) -> Self {
        Self {
            end: Some(Instant::now() + duration),
        }
    }

    pub(crate) fn cap_after(self, duration: Duration) -> Self {
        let phase_end = Instant::now() + duration;
        self.end.map_or(
            Self {
                end: Some(phase_end),
            },
            |end| Self {
                end: Some(end.min(phase_end)),
            },
        )
    }

    pub(crate) fn timeout(self, maximum: Duration) -> Result<Duration, Error> {
        let Some(end) = self.end else {
            return Ok(maximum);
        };
        let remaining = end.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::ZellijFailed {
                message: "operation deadline was exceeded".to_owned(),
            });
        }
        Ok(remaining.min(maximum))
    }
}

impl<Z: Zellij> Controller<Z> {
    #[must_use]
    pub const fn new(store: StateStore, zellij: Z) -> Self {
        Self { store, zellij }
    }

    pub(crate) fn open(&self) -> Result<(LockedState, Registry), Error> {
        let mut locked = self.store.try_lock()?;
        let registry = locked.load_or_create(self.store.paths().project_root())?;
        Ok((locked, registry))
    }

    pub(crate) fn target(registry: &Registry, active: &ActiveJob) -> PaneTarget {
        PaneTarget {
            session: registry.session.clone(),
            pane_id: active.pane_id,
            title: active.title.clone(),
        }
    }

    pub(crate) fn live_pane(
        &self,
        registry: &Registry,
        active: &ActiveJob,
        deadline: Deadline,
    ) -> Result<Option<PaneSnapshot>, Error> {
        if !self.zellij.session_exists(
            &registry.session,
            deadline.timeout(self.zellij.command_timeout())?,
        )? {
            return Ok(None);
        }
        let target = Self::target(registry, active);
        let panes = self.zellij.list_panes(
            &registry.session,
            deadline.timeout(self.zellij.command_timeout())?,
        )?;
        Ok(find_owned_pane(&panes, &target).cloned())
    }

    pub(crate) fn ensure_session(&self, registry: &Registry) -> Result<(), Error> {
        write_private_files(self.store.paths(), &registry.owner_nonce)?;
        let deadline = Deadline::after(START_DEADLINE);
        let _bootstrap_lock = self
            .store
            .lock_bootstrap(deadline.timeout(START_DEADLINE)?)?;
        if !self.zellij.session_exists(
            &registry.session,
            deadline.timeout(self.zellij.command_timeout())?,
        )? {
            self.zellij.create_background(
                &registry.session,
                &self.store.paths().layout_file(),
                deadline.timeout(self.zellij.command_timeout())?,
            )?;
        }
        let keeper_title = format!("agent-terminal:keeper:{}", &registry.owner_nonce[..12]);
        loop {
            let timeout = deadline.timeout(self.zellij.command_timeout())?;
            if let Ok(panes) = self.zellij.list_panes(&registry.session, timeout)
                && panes
                    .iter()
                    .any(|pane| !pane.is_plugin && pane.title == keeper_title)
            {
                return Ok(());
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    pub(crate) fn wait_for_target(
        &self,
        registry: &Registry,
        active: &ActiveJob,
        timeout: Duration,
    ) -> Result<Option<PaneSnapshot>, Error> {
        let deadline = Deadline::after(timeout);
        loop {
            if let Some(pane) = self.live_pane(registry, active, deadline)? {
                return Ok(Some(pane));
            }
            if deadline.timeout(self.zellij.command_timeout()).is_err() {
                return Ok(None);
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    pub(crate) fn capture_screen(
        &self,
        registry: &Registry,
        active: &ActiveJob,
        deadline: Deadline,
    ) -> Result<BoundedScreen, Error> {
        let target = Self::target(registry, active);
        let suffix = DUMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = self.store.paths().project_dir().join(format!(
            ".screen.{}.{}",
            std::process::id(),
            suffix
        ));
        let dump_result = self.zellij.dump_screen(
            &target,
            &path,
            deadline.timeout(self.zellij.command_timeout())?,
        );
        if let Err(error) = dump_result {
            cleanup_dump(&path);
            return Err(error);
        }
        let bytes = fs::read(&path).map_err(|source| Error::StateIo {
            action: "read screen dump",
            path: path.clone(),
            source,
        });
        cleanup_dump(&path);
        let normalized = normalize_screen(&String::from_utf8_lossy(&bytes?));
        Ok(bound_screen(&normalized, SCREEN_LINES, SCREEN_BYTES))
    }

    pub(crate) const fn pane_state(pane: &PaneSnapshot) -> (JobState, Option<i32>) {
        if pane.exited {
            (JobState::Exited, pane.exit_status)
        } else {
            (JobState::Running, None)
        }
    }

    pub(crate) fn active_from(
        registry: &Registry,
        job: &crate::domain::JobName,
    ) -> Result<ActiveJob, Error> {
        match registry.jobs.get(job) {
            Some(crate::state::JobRecord::Active(active)) => Ok(active.clone()),
            Some(_) => Err(Error::ZellijFailed {
                message: format!("job {job:?} did not reconcile to an active record"),
            }),
            None => Err(Error::JobNotFound {
                job: job.to_string(),
            }),
        }
    }
}

fn cleanup_dump(path: &std::path::Path) {
    if let Err(source) = fs::remove_file(path)
        && source.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %source, "failed to remove screen dump");
    }
}
