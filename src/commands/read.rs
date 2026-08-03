use crate::{
    controller::{Controller, Deadline},
    domain::JobName,
    error::Error,
    output::ReadData,
    zellij::Zellij,
};

impl<Z: Zellij> Controller<Z> {
    pub fn read(&self, job: &JobName) -> Result<ReadData, Error> {
        let (mut locked, mut registry) = self.open()?;
        self.reconcile_job(&mut locked, &mut registry, job, Deadline::per_call())?;
        let active = Self::active_from(&registry, job)?;
        let Some(pane) = self.live_pane(&registry, &active, Deadline::per_call())? else {
            self.reconcile_job(&mut locked, &mut registry, job, Deadline::per_call())?;
            return Err(Error::JobNotFound {
                job: job.to_string(),
            });
        };
        let (state, exit_code) = Self::pane_state(&pane);
        let bounded = self.capture_screen(&registry, &active, Deadline::per_call())?;
        Ok(ReadData {
            state,
            exit_code,
            screen: bounded.screen,
            truncated: bounded.truncated,
        })
    }
}
