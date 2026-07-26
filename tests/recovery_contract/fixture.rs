use std::{fs, path::PathBuf};

use agent_terminal::{
    controller::Controller,
    domain::{JobName, TerminalPaneId},
    error::Error,
    paths::ProjectPaths,
    state::{ActiveJob, JobRecord, PendingStart, Registry, StateStore},
    zellij::PaneSnapshot,
};
use tempfile::TempDir;

use crate::support::{FakeZellij, pane};

pub struct Fixture {
    _temp: TempDir,
    pub project: PathBuf,
    pub store: StateStore,
    pub registry: Registry,
    pub fake: FakeZellij,
}

impl Fixture {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
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

    pub fn controller(&self) -> Controller<FakeZellij> {
        Controller::new(self.store.clone(), self.fake.clone())
    }

    pub fn save(&self, registry: &Registry) -> Result<(), Error> {
        self.store.try_lock()?.save(registry)
    }

    pub fn save_current(&self) -> Result<(), Error> {
        self.save(&self.registry)
    }

    pub fn reload(&self) -> Result<Registry, Error> {
        self.store.try_lock()?.load_or_create(&self.project)
    }

    pub fn active(&self, job: &JobName, id: u32) -> ActiveJob {
        ActiveJob {
            operation_nonce: "0123456789abcdef0123456789abcdef".to_owned(),
            title: format!("agent-terminal:{job}:0123456789ab"),
            cwd: self.project.clone(),
            command: vec!["sh".to_owned()],
            pane_id: TerminalPaneId::new(id),
        }
    }

    pub fn install_active(&mut self, job: &JobName, id: u32) {
        self.registry
            .jobs
            .insert(job.clone(), JobRecord::Active(self.active(job, id)));
    }

    pub fn pending(&self, job: &JobName, id: Option<u32>, expired: bool) -> PendingStart {
        let mut pending = PendingStart::for_job(job, self.project.clone(), vec!["sh".to_owned()]);
        pending.pane_id = id.map(TerminalPaneId::new);
        if expired {
            pending.created_millis = 0;
        }
        pending
    }

    pub fn keeper(&self) -> PaneSnapshot {
        pane(
            0,
            &format!("agent-terminal:keeper:{}", &self.registry.owner_nonce[..12]),
            false,
        )
    }

    pub fn owned_pane(&self, job: &JobName, id: u32, exited: bool) -> PaneSnapshot {
        pane(id, &self.active(job, id).title, exited)
    }
}
