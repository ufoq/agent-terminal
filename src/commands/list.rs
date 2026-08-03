use crate::{
    controller::{Controller, Deadline},
    error::Error,
    output::{JobSummary, ListData},
    state::JobRecord,
    zellij::Zellij,
};

impl<Z: Zellij> Controller<Z> {
    pub fn list(&self) -> Result<ListData, Error> {
        let (mut locked, mut registry) = self.open()?;
        let names: Vec<_> = registry.jobs.keys().cloned().collect();
        for job in names {
            self.reconcile_job(&mut locked, &mut registry, &job, Deadline::per_call())?;
        }
        let mut jobs = Vec::with_capacity(registry.jobs.len());
        for (job, record) in &registry.jobs {
            if let JobRecord::Active(active) = record {
                let pane = self.live_pane(&registry, active, Deadline::per_call())?;
                if let Some(pane) = pane {
                    let (state, exit_code) = Self::pane_state(&pane);
                    jobs.push(JobSummary {
                        job: job.clone(),
                        state,
                        exit_code,
                    });
                }
            }
        }
        Ok(ListData { jobs })
    }
}
