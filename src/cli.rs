use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{
    commands,
    domain::{JobName, Key},
    error::Error,
    output::CommandData,
};

#[derive(Debug, Parser)]
#[command(name = "agent-terminal", version, about = "Agent-native terminal jobs")]
pub struct Cli {
    /// Project root; defaults to the nearest Git root of the current directory.
    #[arg(long, global = true, value_name = "PATH")]
    pub project: Option<PathBuf>,
    /// Runtime state directory (tests and isolated runs).
    #[arg(long, global = true, env = "AGENT_TERMINAL_STATE", value_name = "PATH")]
    pub state_dir: Option<PathBuf>,
    /// Worker-lifetime namespace for jobs that outlive pi.
    #[arg(long, global = true)]
    pub global: bool,
    /// Pretty-print JSON output.
    #[arg(long, global = true)]
    pub pretty: bool,
    /// Increase log verbosity.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List jobs in this project and scope.
    List,
    /// Start a command in a named job.
    Start {
        /// Job name.
        job: JobName,
        /// Working directory for the job.
        #[arg(long, value_name = "PATH")]
        cwd: Option<PathBuf>,
        /// Command to run.
        #[arg(last = true, required = true, num_args = 1.., value_name = "COMMAND")]
        command: Vec<String>,
    },
    /// Read the current screen of a job.
    Read {
        /// Job name.
        job: JobName,
    },
    /// Send text to a job and submit it with Enter.
    Send {
        /// Job name.
        job: JobName,
        /// Text to send and submit; multiple words are joined with spaces.
        #[arg(required = true, num_args = 1.., value_name = "TEXT")]
        text: Vec<String>,
    },
    /// Send one key to a job.
    Press {
        /// Job name.
        job: JobName,
        /// Key name, or Ctrl+X / Alt+X.
        key: Key,
    },
    /// Stop a job.
    Stop {
        /// Job name.
        job: JobName,
    },
    /// Internal keeper (not part of the agent interface).
    #[command(hide = true, name = "__keeper")]
    Keeper {
        owners: PathBuf,
        ready: PathBuf,
        zellij: PathBuf,
        lock: PathBuf,
    },
}

pub fn run(cli: Cli) -> Result<CommandData, Error> {
    if let Command::Keeper {
        owners,
        ready,
        zellij,
        lock,
    } = &cli.command
    {
        crate::keeper::run(owners, ready, zellij, lock)?;
        return Ok(CommandData::Stop(crate::output::StopData {
            job: "keeper".parse()?,
        }));
    }
    let context =
        commands::build_context(cli.project.as_deref(), cli.state_dir.as_deref(), cli.global)?;
    let command = match cli.command {
        Command::List => CliCommand::List,
        Command::Start { job, cwd, command } => CliCommand::Start { job, cwd, command },
        Command::Read { job } => CliCommand::Read { job },
        Command::Send { job, text } => CliCommand::Send {
            job,
            text: text.join(" "),
        },
        Command::Press { job, key } => CliCommand::Press { job, key },
        Command::Stop { job } => CliCommand::Stop { job },
        Command::Keeper { .. } => {
            return Err(Error::InvalidInput {
                message: "internal keeper command cannot be run here".to_owned(),
            });
        }
    };
    commands::run_command(&context, command)
}

pub enum CliCommand {
    List,
    Start {
        job: JobName,
        cwd: Option<PathBuf>,
        command: Vec<String>,
    },
    Read {
        job: JobName,
    },
    Send {
        job: JobName,
        text: String,
    },
    Press {
        job: JobName,
        key: Key,
    },
    Stop {
        job: JobName,
    },
}
