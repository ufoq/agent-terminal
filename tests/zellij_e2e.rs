#![cfg(unix)]

#[path = "zellij_e2e/concurrency.rs"]
mod concurrency;
#[path = "zellij_e2e/harness.rs"]
mod harness;
#[path = "zellij_e2e/isolation.rs"]
mod isolation;
#[path = "zellij_e2e/lifecycle.rs"]
mod lifecycle;

pub(crate) use harness::E2E_LOCK;
