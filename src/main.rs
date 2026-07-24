use std::process::ExitCode;

use agent_terminal::{
    cli::{Cli, run},
    error::Error,
    output::{Response, print},
    telemetry,
};
use clap::{Parser as _, error::ErrorKind};

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            return if error.print().is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            };
        }
        Err(error) => {
            return emit(
                &Response::error(&Error::InvalidInput {
                    message: error.to_string(),
                }),
                false,
                2,
            );
        }
    };
    let pretty = cli.pretty;
    if let Err(error) = telemetry::init(cli.verbose) {
        return emit(&Response::error(&error), pretty, 2);
    }
    match run(cli) {
        Ok(data) => emit(&Response::ok(data), pretty, 0),
        Err(error) => {
            let code = error.exit_code();
            emit(&Response::error(&error), pretty, code)
        }
    }
}

fn emit(response: &Response, pretty: bool, code: u8) -> ExitCode {
    match print(response, pretty) {
        Ok(()) => ExitCode::from(code),
        Err(error) => {
            tracing::error!(%error, "failed to write response");
            ExitCode::from(2)
        }
    }
}
