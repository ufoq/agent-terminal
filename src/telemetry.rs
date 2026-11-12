use crate::error::Error;

pub fn init(verbose: u8) -> Result<(), Error> {
    let default = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|source| Error::InvalidInput {
            message: format!("could not initialize logging: {source}"),
        })
}
