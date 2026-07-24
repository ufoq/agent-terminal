use std::io;

use tracing_subscriber::EnvFilter;

use crate::error::Error;

pub fn init(verbose: u8) -> Result<(), Error> {
    let default_level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .compact()
        .with_writer(io::stderr)
        .try_init()
        .map_err(|source| Error::InvalidInput {
            message: format!("could not initialize tracing: {source}"),
        })
}
