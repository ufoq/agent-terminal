use crate::{
    controller::{Controller, Deadline},
    domain::{JobName, Key},
    error::Error,
    output::PressData,
    zellij::Zellij,
};

impl<Z: Zellij> Controller<Z> {
    pub fn press(&self, job: &JobName, keys: &[Key]) -> Result<PressData, Error> {
        if keys.is_empty() {
            return Err(Error::InvalidInput {
                message: "press requires at least one key".to_owned(),
            });
        }
        let (mut locked, mut registry) = self.open()?;
        self.reconcile_job(&mut locked, &mut registry, job, Deadline::per_call())?;
        let active = Self::active_from(&registry, job)?;
        let pane = self.live_pane(&registry, &active, Deadline::per_call())?;
        if pane.as_ref().is_none_or(|pane| pane.exited) {
            return Err(Error::JobNotRunning {
                job: job.to_string(),
            });
        }
        let tokens: Vec<_> = keys
            .iter()
            .map(|key| key.zellij_token().to_owned())
            .collect();
        match self.zellij.send_keys(
            &Self::target(&registry, &active),
            &tokens,
            self.zellij.command_timeout(),
        ) {
            Ok(()) => {}
            Err(Error::ZellijTimeout) => {
                return Err(Error::DeliveryUncertain);
            }
            Err(error) => return Err(error),
        }
        Ok(PressData)
    }
}
