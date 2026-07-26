use std::{path::PathBuf, str::FromStr as _, time::Duration};

use clap::{Args, Parser, Subcommand};

use crate::{
    controller::Controller,
    domain::{JobName, Key},
    error::Error,
    output::CommandData,
    paths::{ProjectPaths, find_project_root},
    state::StateStore,
    zellij::ZellijCli,
};

#[derive(Debug, Parser)]
#[command(
    name = "agent-terminal",
    version,
    about = "Agent-native terminal job control backed by Zellij"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub project: Option<PathBuf>,
    #[arg(long, env = "AGENT_TERMINAL_STATE", global = true)]
    pub state_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    pub pretty: bool,
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: CliCommand,
}

#[derive(Debug, Subcommand)]
pub enum CliCommand {
    Start(StartArgs),
    Read(JobArgs),
    Send(SendArgs),
    Press(PressArgs),
    Stop(StopArgs),
    List,
}

#[derive(Debug, Args)]
pub struct StartArgs {
    pub job: String,
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

#[derive(Debug, Args)]
pub struct JobArgs {
    pub job: String,
}

#[derive(Debug, Args)]
pub struct SendArgs {
    pub job: String,
    #[arg(long)]
    pub no_submit: bool,
    #[arg(last = true, required = true)]
    pub text: String,
}

#[derive(Debug, Args)]
pub struct PressArgs {
    pub job: String,
    #[arg(required = true, last = true, num_args = 1.., allow_hyphen_values = true)]
    pub keys: Vec<String>,
}

#[derive(Debug, Args)]
pub struct StopArgs {
    pub job: String,
    #[arg(long)]
    pub force: bool,
}

pub fn run(cli: Cli) -> Result<CommandData, Error> {
    let invocation_dir = std::env::current_dir().map_err(|source| Error::StateIo {
        action: "read current directory",
        path: PathBuf::from("."),
        source,
    })?;
    let project = match cli.project {
        Some(project) => project,
        None => find_project_root(&invocation_dir)?,
    };
    let paths = ProjectPaths::new(&project, cli.state_dir.as_deref())?;
    let store = StateStore::new(paths.clone());
    let zellij = ZellijCli::new(
        PathBuf::from("zellij"),
        paths.config_file(),
        Duration::from_secs(2),
    );
    let controller = Controller::new(store, zellij);

    match cli.command {
        CliCommand::List => controller.list().map(CommandData::List),
        CliCommand::Start(args) => controller
            .start(
                JobName::from_str(&args.job)?,
                args.cwd,
                args.command,
                &invocation_dir,
            )
            .map(CommandData::Start),
        CliCommand::Read(args) => controller
            .read(JobName::from_str(&args.job)?)
            .map(CommandData::Read),
        CliCommand::Send(args) => controller
            .send(JobName::from_str(&args.job)?, &args.text, !args.no_submit)
            .map(CommandData::Send),
        CliCommand::Press(args) => {
            let keys = args
                .keys
                .iter()
                .map(|key| Key::from_str(key))
                .collect::<Result<Vec<_>, _>>()?;
            controller
                .press(JobName::from_str(&args.job)?, &keys)
                .map(CommandData::Press)
        }
        CliCommand::Stop(args) => controller
            .stop(JobName::from_str(&args.job)?, args.force)
            .map(CommandData::Stop),
    }
}
