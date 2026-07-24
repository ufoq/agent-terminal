#![allow(clippy::disallowed_methods)]

use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use agent_terminal::{
    controller::Controller,
    domain::{JobName, SessionName, TerminalPaneId},
    error::Error,
    paths::ProjectPaths,
    state::{ActiveJob, JobRecord, PendingRemove, PendingStart, Registry, StateStore},
    zellij::{PaneSnapshot, PaneSpec, PaneTarget, Zellij},
};
use tempfile::TempDir;

#[derive(Clone, Default)]
struct FakeZellij {
    inner: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    session_exists: bool,
    panes: Vec<PaneSnapshot>,
    fail_session_lookup: bool,
    replace_after_paste: bool,
    killed_sessions: usize,
    sent_keys: Vec<Vec<String>>,
}

impl FakeZellij {
    fn state(&self) -> Result<MutexGuard<'_, FakeState>, Error> {
        self.inner.lock().map_err(|_| Error::ZellijFailed {
            message: "fake state lock was poisoned".to_owned(),
        })
    }
}

impl Zellij for FakeZellij {
    fn create_background(
        &self,
        _session: &SessionName,
        _layout: &Path,
        _timeout: Duration,
    ) -> Result<(), Error> {
        self.state()?.session_exists = true;
        Ok(())
    }

    fn session_exists(&self, _session: &SessionName, _timeout: Duration) -> Result<bool, Error> {
        let state = self.state()?;
        if state.fail_session_lookup {
            return Err(Error::ZellijFailed {
                message: "injected session lookup failure".to_owned(),
            });
        }
        Ok(state.session_exists)
    }

    fn list_panes(
        &self,
        _session: &SessionName,
        _timeout: Duration,
    ) -> Result<Vec<PaneSnapshot>, Error> {
        Ok(self.state()?.panes.clone())
    }

    fn create_pane(&self, _spec: &PaneSpec, _timeout: Duration) -> Result<TerminalPaneId, Error> {
        Ok(TerminalPaneId::new(42))
    }

    fn dump_screen(
        &self,
        _target: &PaneTarget,
        path: &Path,
        _timeout: Duration,
    ) -> Result<(), Error> {
        fs::write(path, "screen").map_err(|source| Error::StateIo {
            action: "write fake screen",
            path: path.to_path_buf(),
            source,
        })
    }

    fn paste(&self, target: &PaneTarget, _text: &str, _timeout: Duration) -> Result<(), Error> {
        let mut state = self.state()?;
        if state.replace_after_paste {
            for pane in &mut state.panes {
                if pane.id == target.pane_id.get() {
                    "foreign-pane".clone_into(&mut pane.title);
                }
            }
        }
        Ok(())
    }

    fn send_keys(
        &self,
        _target: &PaneTarget,
        keys: &[String],
        _timeout: Duration,
    ) -> Result<(), Error> {
        self.state()?.sent_keys.push(keys.to_vec());
        Ok(())
    }

    fn close_pane(&self, target: &PaneTarget, _timeout: Duration) -> Result<(), Error> {
        self.state()?.panes.retain(|pane| {
            pane.is_plugin || pane.id != target.pane_id.get() || pane.title != target.title
        });
        Ok(())
    }

    fn kill_session(&self, _session: &SessionName, _timeout: Duration) -> Result<(), Error> {
        let mut state = self.state()?;
        state.killed_sessions += 1;
        state.session_exists = false;
        state.panes.clear();
        drop(state);
        Ok(())
    }

    fn command_timeout(&self) -> Duration {
        Duration::from_millis(20)
    }
}

struct Fixture {
    _temp: TempDir,
    project: PathBuf,
    store: StateStore,
    registry: Registry,
    fake: FakeZellij,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let project = temp.path().join("project");
        fs::create_dir_all(&project)?;
        let project = project.canonicalize()?;
        let paths = ProjectPaths::new(&project, Some(&temp.path().join("state")))?;
        let store = StateStore::new(paths);
        let registry = Registry::new(project.clone())?;
        Ok(Self {
            _temp: temp,
            project,
            store,
            registry,
            fake: FakeZellij::default(),
        })
    }

    fn save(&self, registry: &Registry) -> Result<(), Error> {
        self.store.try_lock()?.save(registry)
    }

    fn reload(&self) -> Result<Registry, Error> {
        self.store.try_lock()?.load_or_create(&self.project)
    }

    fn active(&self, job: &JobName, id: u32) -> ActiveJob {
        ActiveJob {
            operation_nonce: "0123456789abcdef0123456789abcdef".to_owned(),
            title: format!("agent-terminal:{job}:0123456789ab"),
            cwd: self.project.clone(),
            command: vec!["sh".to_owned()],
            pane_id: TerminalPaneId::new(id),
        }
    }

    fn keeper(&self) -> PaneSnapshot {
        pane(
            0,
            &format!("agent-terminal:keeper:{}", &self.registry.owner_nonce[..12]),
            false,
        )
    }
}

#[test]
fn stale_last_job_flag_does_not_kill_a_new_job() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let old = JobName::from_str("old")?;
    let new = JobName::from_str("new")?;
    fixture.registry.jobs.insert(
        old,
        JobRecord::PendingRemove(PendingRemove {
            job: fixture.active(&JobName::from_str("old")?, 1),
            force_authorized: false,
        }),
    );
    fixture
        .registry
        .jobs
        .insert(new.clone(), JobRecord::Active(fixture.active(&new, 2)));
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![
        fixture.keeper(),
        pane(2, "agent-terminal:new:0123456789ab", false),
    ];
    fixture.save(&fixture.registry)?;
    let _round_trip = fixture.reload()?;

    let listed = Controller::new(fixture.store.clone(), fixture.fake.clone()).list()?;

    assert_eq!(listed.jobs.len(), 1);
    assert_eq!(listed.jobs[0].job, new);
    assert_eq!(fixture.fake.state()?.killed_sessions, 0);
    Ok(())
}

#[test]
fn removal_kills_the_session_when_it_is_now_last() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("old")?;
    fixture.registry.jobs.insert(
        job,
        JobRecord::PendingRemove(PendingRemove {
            job: fixture.active(&JobName::from_str("old")?, 1),
            force_authorized: false,
        }),
    );
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![fixture.keeper()];
    fixture.save(&fixture.registry)?;

    Controller::new(fixture.store.clone(), fixture.fake.clone()).list()?;

    assert_eq!(fixture.fake.state()?.killed_sessions, 1);
    Ok(())
}

#[test]
fn backend_failure_does_not_discard_pending_start() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("pending")?;
    fixture.registry.jobs.insert(
        job.clone(),
        JobRecord::PendingStart(PendingStart::for_job(
            &job,
            fixture.project.clone(),
            vec!["sh".to_owned()],
        )),
    );
    fixture.fake.state()?.fail_session_lookup = true;
    fixture.save(&fixture.registry)?;

    let result =
        Controller::new(fixture.store.clone(), fixture.fake.clone()).stop(job.clone(), false);

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert!(matches!(
        fixture.reload()?.jobs.get(&job),
        Some(JobRecord::PendingStart(_))
    ));
    Ok(())
}

#[test]
fn session_deletion_requires_owned_keeper_and_no_foreign_pane()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("old")?;
    fixture.registry.jobs.insert(
        job,
        JobRecord::PendingRemove(PendingRemove {
            job: fixture.active(&JobName::from_str("old")?, 1),
            force_authorized: false,
        }),
    );
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![pane(9, "foreign-pane", false)];
    fixture.save(&fixture.registry)?;

    let result = Controller::new(fixture.store.clone(), fixture.fake.clone()).list();

    assert!(matches!(result, Err(Error::ZellijFailed { .. })));
    assert_eq!(fixture.fake.state()?.killed_sessions, 0);
    Ok(())
}

#[test]
fn recovered_force_stop_reports_actual_forced_close() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("old")?;
    fixture.registry.jobs.insert(
        job.clone(),
        JobRecord::PendingRemove(PendingRemove {
            job: fixture.active(&job, 1),
            force_authorized: true,
        }),
    );
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.panes = vec![
        fixture.keeper(),
        pane(1, "agent-terminal:old:0123456789ab", false),
    ];
    fixture.save(&fixture.registry)?;

    let stopped = Controller::new(fixture.store.clone(), fixture.fake).stop(job, true)?;

    assert!(stopped.forced);
    Ok(())
}

#[test]
fn submit_revalidates_identity_before_pressing_enter() -> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("prompt")?;
    fixture
        .registry
        .jobs
        .insert(job.clone(), JobRecord::Active(fixture.active(&job, 1)));
    fixture.fake.state()?.session_exists = true;
    fixture.fake.state()?.replace_after_paste = true;
    fixture.fake.state()?.panes = vec![
        fixture.keeper(),
        pane(1, "agent-terminal:prompt:0123456789ab", false),
    ];
    fixture.save(&fixture.registry)?;

    let result =
        Controller::new(fixture.store.clone(), fixture.fake.clone()).send(job, "yes", true);

    assert!(matches!(result, Err(Error::JobNotRunning { .. })));
    assert!(fixture.fake.state()?.sent_keys.is_empty());
    Ok(())
}

#[test]
fn semantically_invalid_registry_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fixture.save(&fixture.registry)?;
    let mut value = serde_json::to_value(&fixture.registry)?;
    value["session"] = serde_json::Value::String("agent-terminal-foreign-session".to_owned());
    fs::write(
        fixture.store.paths().state_file(),
        serde_json::to_vec(&value)?,
    )?;

    let result = fixture.reload();

    assert!(matches!(result, Err(Error::StateCorrupt { .. })));
    Ok(())
}

#[test]
fn active_title_must_match_its_persisted_operation_nonce() -> Result<(), Box<dyn std::error::Error>>
{
    let mut fixture = Fixture::new()?;
    let job = JobName::from_str("api")?;
    fixture.registry.jobs.insert(
        job,
        JobRecord::Active(ActiveJob {
            operation_nonce: "0123456789abcdef0123456789abcdef".to_owned(),
            title: "agent-terminal:api:ffffffffffff".to_owned(),
            cwd: fixture.project.clone(),
            command: vec!["sh".to_owned()],
            pane_id: TerminalPaneId::new(1),
        }),
    );
    fixture.save(&fixture.registry)?;

    assert!(matches!(fixture.reload(), Err(Error::StateCorrupt { .. })));
    Ok(())
}

fn pane(id: u32, title: &str, exited: bool) -> PaneSnapshot {
    PaneSnapshot {
        id,
        is_plugin: false,
        title: title.to_owned(),
        exited,
        exit_status: exited.then_some(0),
        is_held: exited,
    }
}
