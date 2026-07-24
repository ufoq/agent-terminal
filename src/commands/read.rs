use crate::{
    controller::{Controller, Deadline},
    domain::JobName,
    error::Error,
    output::ReadData,
    zellij::Zellij,
};

impl<Z: Zellij> Controller<Z> {
    pub fn read(&self, job: JobName) -> Result<ReadData, Error> {
        let (mut locked, mut registry) = self.open()?;
        self.reconcile_job(&mut locked, &mut registry, &job, Deadline::per_call())?;
        let active = Self::active_from(&registry, &job)?;
        let pane = self.live_pane(&registry, &active, Deadline::per_call())?;
        let (state, exit_code) = Self::pane_state(pane.as_ref());
        let screen = match pane {
            Some(_) => Some(self.capture_screen(&registry, &active, Deadline::per_call())?),
            None => None,
        };
        Ok(ReadData {
            job,
            state,
            exit_code,
            screen_available: screen.is_some(),
            screen: screen.as_ref().map(|bounded| bounded.screen.clone()),
            truncated: screen.map(|bounded| bounded.truncated),
        })
    }
}
