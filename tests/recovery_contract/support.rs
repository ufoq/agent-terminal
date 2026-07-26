use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use agent_terminal::{
    domain::{SessionName, TerminalPaneId},
    error::Error,
    zellij::{PaneSnapshot, PaneSpec, PaneTarget, Zellij},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    CreateBackground,
    SessionExists,
    ListPanes,
    CreatePane,
    DumpScreen,
    Paste(String),
    SendKeys(Vec<String>),
    ClosePane,
    KillSession,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub enum Fault {
    #[default]
    Pass,
    Fail,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub enum AfterPaste {
    #[default]
    Unchanged,
    ReplaceIdentity,
    FailSessionLookup,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub enum AfterCtrlC {
    #[default]
    Running,
    Exit,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub enum DestructiveEffect {
    #[default]
    Succeed,
    FailAndRemain,
    FailAndDisappear,
}

#[derive(Clone, Default)]
pub struct FakeZellij {
    inner: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
pub struct FakeState {
    pub session_exists: bool,
    pub panes: Vec<PaneSnapshot>,
    pub screen: Vec<u8>,
    pub operations: Vec<Operation>,
    pub dump_paths: Vec<PathBuf>,
    pub killed_sessions: usize,
    pub create_background: Fault,
    pub session_lookup: Fault,
    pub pane_listing: Fault,
    pub create_pane: Fault,
    pub dump: Fault,
    pub paste: Fault,
    pub after_paste: AfterPaste,
    pub after_ctrl_c: AfterCtrlC,
    pub close: DestructiveEffect,
    pub kill: DestructiveEffect,
}

impl FakeZellij {
    pub fn state(&self) -> Result<MutexGuard<'_, FakeState>, Error> {
        self.inner.lock().map_err(|_| Error::ZellijFailed {
            message: "fake state lock was poisoned".to_owned(),
        })
    }
}

fn injected(message: &str) -> Error {
    Error::ZellijFailed {
        message: message.to_owned(),
    }
}

impl Zellij for FakeZellij {
    fn create_background(
        &self,
        _session: &SessionName,
        _layout: &Path,
        _timeout: Duration,
    ) -> Result<(), Error> {
        let mut state = self.state()?;
        state.operations.push(Operation::CreateBackground);
        if state.create_background == Fault::Fail {
            return Err(injected("injected create background failure"));
        }
        state.session_exists = true;
        drop(state);
        Ok(())
    }

    fn session_exists(&self, _session: &SessionName, _timeout: Duration) -> Result<bool, Error> {
        let mut state = self.state()?;
        state.operations.push(Operation::SessionExists);
        if state.session_lookup == Fault::Fail {
            return Err(injected("injected session lookup failure"));
        }
        Ok(state.session_exists)
    }

    fn list_panes(
        &self,
        _session: &SessionName,
        _timeout: Duration,
    ) -> Result<Vec<PaneSnapshot>, Error> {
        let mut state = self.state()?;
        state.operations.push(Operation::ListPanes);
        if state.pane_listing == Fault::Fail {
            return Err(injected("injected pane listing failure"));
        }
        Ok(state.panes.clone())
    }

    fn create_pane(&self, _spec: &PaneSpec, _timeout: Duration) -> Result<TerminalPaneId, Error> {
        let mut state = self.state()?;
        state.operations.push(Operation::CreatePane);
        if state.create_pane == Fault::Fail {
            return Err(injected("injected create pane failure"));
        }
        drop(state);
        Ok(TerminalPaneId::new(42))
    }

    fn dump_screen(
        &self,
        _target: &PaneTarget,
        path: &Path,
        _timeout: Duration,
    ) -> Result<(), Error> {
        let mut state = self.state()?;
        state.operations.push(Operation::DumpScreen);
        state.dump_paths.push(path.to_path_buf());
        if state.dump == Fault::Fail {
            return Err(injected("injected dump failure"));
        }
        fs::write(path, &state.screen).map_err(|source| Error::StateIo {
            action: "write fake screen",
            path: path.to_path_buf(),
            source,
        })
    }

    fn paste(&self, target: &PaneTarget, text: &str, _timeout: Duration) -> Result<(), Error> {
        let mut state = self.state()?;
        state.operations.push(Operation::Paste(text.to_owned()));
        if state.paste == Fault::Fail {
            return Err(injected("injected paste failure"));
        }
        match state.after_paste {
            AfterPaste::Unchanged => {}
            AfterPaste::ReplaceIdentity => {
                for pane in &mut state.panes {
                    if pane.id == target.pane_id.get() {
                        "foreign-pane".clone_into(&mut pane.title);
                    }
                }
            }
            AfterPaste::FailSessionLookup => state.session_lookup = Fault::Fail,
        }
        drop(state);
        Ok(())
    }

    fn send_keys(
        &self,
        target: &PaneTarget,
        keys: &[String],
        _timeout: Duration,
    ) -> Result<(), Error> {
        let mut state = self.state()?;
        state.operations.push(Operation::SendKeys(keys.to_vec()));
        if state.after_ctrl_c == AfterCtrlC::Exit && keys == ["Ctrl c"] {
            for pane in &mut state.panes {
                if pane.id == target.pane_id.get() && pane.title == target.title {
                    pane.exited = true;
                    pane.exit_status = Some(130);
                }
            }
        }
        Ok(())
    }

    fn close_pane(&self, target: &PaneTarget, _timeout: Duration) -> Result<(), Error> {
        let mut state = self.state()?;
        state.operations.push(Operation::ClosePane);
        match state.close {
            DestructiveEffect::Succeed | DestructiveEffect::FailAndDisappear => {
                state.panes.retain(|pane| {
                    pane.is_plugin || pane.id != target.pane_id.get() || pane.title != target.title
                });
            }
            DestructiveEffect::FailAndRemain => {}
        }
        if state.close != DestructiveEffect::Succeed {
            return Err(injected("injected close failure"));
        }
        drop(state);
        Ok(())
    }

    fn kill_session(&self, _session: &SessionName, _timeout: Duration) -> Result<(), Error> {
        let mut state = self.state()?;
        state.operations.push(Operation::KillSession);
        state.killed_sessions += 1;
        match state.kill {
            DestructiveEffect::Succeed | DestructiveEffect::FailAndDisappear => {
                state.session_exists = false;
                state.panes.clear();
            }
            DestructiveEffect::FailAndRemain => {}
        }
        if state.kill != DestructiveEffect::Succeed {
            return Err(injected("injected kill failure"));
        }
        drop(state);
        Ok(())
    }

    fn command_timeout(&self) -> Duration {
        Duration::from_millis(20)
    }
}

pub fn pane(id: u32, title: &str, exited: bool) -> PaneSnapshot {
    PaneSnapshot {
        id,
        is_plugin: false,
        title: title.to_owned(),
        exited,
        exit_status: exited.then_some(0),
    }
}
