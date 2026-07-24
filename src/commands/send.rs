use crate::{
    controller::{Controller, Deadline},
    domain::JobName,
    error::Error,
    output::{Issued, SendData},
    zellij::Zellij,
};

impl<Z: Zellij> Controller<Z> {
    pub fn send(&self, job: JobName, text: &str, submit: bool) -> Result<SendData, Error> {
        if text.is_empty() {
            return Err(Error::InvalidInput {
                message: "send text must not be empty".to_owned(),
            });
        }
        let (mut locked, mut registry) = self.open()?;
        self.reconcile_job(&mut locked, &mut registry, &job, Deadline::per_call())?;
        let active = Self::active_from(&registry, &job)?;
        let pane = self.live_pane(&registry, &active, Deadline::per_call())?;
        if pane.as_ref().is_none_or(|pane| pane.exited) {
            return Err(Error::JobNotRunning {
                job: job.to_string(),
            });
        }
        let target = Self::target(&registry, &active);
        self.zellij
            .paste(&target, text, self.zellij.command_timeout())?;
        if submit {
            let pane = self.live_pane(&registry, &active, Deadline::per_call())?;
            if pane.as_ref().is_none_or(|pane| pane.exited) {
                return Err(Error::JobNotRunning {
                    job: job.to_string(),
                });
            }
            self.zellij.send_keys(
                &target,
                &["Enter".to_owned()],
                self.zellij.command_timeout(),
            )?;
        }
        Ok(SendData {
            job,
            issued: Issued::Text,
            submitted: submit,
        })
    }
}
