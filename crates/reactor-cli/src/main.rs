//! `reactor` — a thin binary over `reactor-core`.
//!
//! Every subcommand supports `--format json`. That output shape is REactor's
//! contract with every harness that shells out to this: it never changes without a schema bump, and stdout carries nothing
//! but the payload in that mode. Human output carries no such guarantee.

use std::io::Write;
use std::process::ExitCode;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use reactor_core::commands::{self, DoctorFlags, ProbeFlags, ToolsListFlags};
use reactor_core::config::{self, SetupOpts};
use reactor_core::host::SystemHost;
use reactor_core::install::{self, InstallOpts};
use reactor_core::report::{Done, Report, json_error, json_of};
use reactor_core::{Paths, ReactorError, completion};

#[derive(Parser)]
#[command(
    name = "reactor",
    version,
    about = "Describe the reverse-engineering toolbox on this machine.",
    arg_required_else_help = true
)]
struct Cli {
    /// json is the harnesses' interface and is stable; text is not
    #[arg(long, value_enum, default_value_t = Format::Text, global = true)]
    format: Format,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Args, Clone, Copy)]
struct Probing {
    /// ignore the probe cache
    #[arg(long)]
    refresh: bool,
    /// never probe; report cached values, unknown where absent
    #[arg(long)]
    cached: bool,
}

impl From<Probing> for ProbeFlags {
    fn from(p: Probing) -> Self {
        ProbeFlags { refresh: p.refresh, cached: p.cached }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ConfigFile {
    Tools,
    Toolsets,
}

impl ConfigFile {
    fn name(self) -> &'static str {
        match self {
            ConfigFile::Tools => "tools",
            ConfigFile::Toolsets => "toolsets",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Shell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Clone, Copy, ValueEnum)]
enum IdKind {
    Tools,
    Toolsets,
}

#[derive(Subcommand)]
enum Command {
    /// what is present, what is missing, how to get it
    Doctor {
        /// use cached probes instead of re-probing
        #[arg(long)]
        cached: bool,
        /// also check fetched skills against their remote ref (network)
        #[arg(long)]
        check_skills: bool,
    },
    /// the block injected into the agent's system prompt
    Registry(Probing),
    /// what is running: service state for the tools that have one
    Services(Probing),
    /// catalogue queries and activation
    #[command(subcommand_required = true, arg_required_else_help = true)]
    Tools {
        #[command(subcommand)]
        cmd: ToolsCmd,
    },
    /// named groups of tools
    #[command(subcommand_required = true, arg_required_else_help = true)]
    Toolsets {
        #[command(subcommand)]
        cmd: ToolsetsCmd,
    },
    /// current activation state
    State,
    /// upstream skills REactor has fetched
    #[command(subcommand_required = true, arg_required_else_help = true)]
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },
    /// install a missing tool
    Install {
        /// tool id(s) to install, or 'all' for the whole catalogue;
        /// 'decompile-python[all]' installs every python3.x version the
        /// platform's own package manager offers (opt-in, not part of 'all' --
        /// quote it in your shell)
        #[arg(required = true)]
        id: Vec<String>,
        /// force a specific install method (see `reactor tools show`)
        #[arg(long)]
        method: Option<String>,
        /// print the plan and stop
        #[arg(long)]
        dry_run: bool,
        /// do not ask before running
        #[arg(long, short = 'y')]
        yes: bool,
        /// run pip recipes inside a dedicated venv (~/.reactor/venv), printing
        /// its location when done -- sidesteps a system Python refusing pip
        /// with 'externally-managed-environment' (PEP 668)
        #[arg(long)]
        create_venv: bool,
        /// when no manager recipe is runnable, fall back to the tool's
        /// manual-install-oneliner (ADR-0027), run in a scratch dir and with
        /// the resulting binary symlinked into ~/.local/bin
        #[arg(long)]
        auto_install_manual: bool,
        /// install via the manual-install-oneliner (ADR-0027) even when a
        /// manager recipe is runnable; mutually exclusive with
        /// --auto-install-manual and --method
        #[arg(long)]
        force_install_manual: bool,
    },
    /// drop the probe cache and re-probe
    Refresh,
    /// print a shell completion script (see ADR-0015)
    Completion { shell: Shell },
    /// bare, unprobed ids for the completion scripts
    #[command(name = "__complete", hide = true)]
    Complete { kind: IdKind },
    /// diff(1) shipped config against yours
    DiffConfig {
        /// just one of the two
        #[arg(long, value_enum)]
        file: Option<ConfigFile>,
    },
    /// force-replace your config with the shipped copy (backs up first)
    OverwriteConfig {
        /// just one of the two
        #[arg(long, value_enum)]
        file: Option<ConfigFile>,
        /// do not ask
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// seed ~/.reactor/, fetch upstream skills, install shell completions and the GUI launcher
    Setup {
        /// skip fetching upstream skills
        #[arg(long)]
        no_skills: bool,
        /// skip installing bash/zsh/fish completion scripts
        #[arg(long)]
        no_completions: bool,
        /// skip installing the GUI's desktop entry and icon
        #[arg(long)]
        no_launcher: bool,
        /// report what would happen and stop
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum ToolsCmd {
    /// list catalogue entries
    List {
        /// only tools carrying this tag (repeatable; repeats narrow)
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// only currently active tools
        #[arg(long)]
        active: bool,
        /// only tools detected on this machine
        #[arg(long)]
        present: bool,
        /// only tools that are absent
        #[arg(long)]
        missing: bool,
        #[command(flatten)]
        probing: Probing,
    },
    /// one entry in full
    Show {
        id: String,
        #[command(flatten)]
        probing: Probing,
    },
    /// activate tools (hint only; never blocks)
    Enable {
        #[arg(required = true)]
        id: Vec<String>,
    },
    /// deactivate tools (hint only; never blocks)
    Disable {
        #[arg(required = true)]
        id: Vec<String>,
    },
    /// drop the override and go back to what the toolsets say
    Reset {
        #[arg(required = true)]
        id: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ToolsetsCmd {
    /// list toolsets
    List,
    /// one toolset and its members
    Show { id: String },
    /// activate a toolset
    Enable {
        #[arg(required = true)]
        id: Vec<String>,
    },
    /// deactivate a toolset
    Disable {
        #[arg(required = true)]
        id: Vec<String>,
    },
}

#[derive(Subcommand)]
enum SkillsCmd {
    /// configured skills and whether they are here
    List,
    /// print a skill for review before trusting it
    Show { id: String },
    /// fetch configured upstream skills
    Fetch {
        /// tools to fetch for; default is every configured one
        id: Vec<String>,
    },
}

/// Write to stdout; a closed pipe (`reactor … | head`) is not an error.
fn out(text: &str) -> bool {
    let mut so = std::io::stdout().lock();
    so.write_all(text.as_bytes()).and_then(|_| so.flush()).is_ok()
}

fn emit<R: Report>(format: Format, done: Done<R>) -> i32 {
    match format {
        Format::Json => {
            out(&(json_of(&done.report) + "\n"));
        }
        Format::Text => {
            let human = done.report.human();
            if !human.is_empty() {
                out(&(human + "\n"));
            }
        }
    }
    done.code
}

fn run(cli: Cli) -> Result<i32, ReactorError> {
    let paths = Paths::from_env();
    let f = cli.format;
    let host = |yes: bool| SystemHost { json: f == Format::Json, yes };
    Ok(match cli.command {
        Command::Doctor { cached, check_skills } => {
            emit(f, commands::doctor(&paths, DoctorFlags { cached, check_skills })?)
        }
        Command::Registry(p) => emit(f, commands::registry(&paths, p.into())?),
        Command::Services(p) => emit(f, commands::services(&paths, p.into())?),
        Command::Tools { cmd } => match cmd {
            ToolsCmd::List { tags, active, present, missing, probing } => emit(
                f,
                commands::tools_list(&paths, &ToolsListFlags { tags, active, present, missing, probe: probing.into() })?,
            ),
            ToolsCmd::Show { id, probing } => emit(f, commands::tools_show(&paths, &id, probing.into())?),
            ToolsCmd::Enable { id } => emit(f, commands::set_activation(&paths, &id, true, Some(true))?),
            ToolsCmd::Disable { id } => emit(f, commands::set_activation(&paths, &id, true, Some(false))?),
            ToolsCmd::Reset { id } => emit(f, commands::set_activation(&paths, &id, true, None)?),
        },
        Command::Toolsets { cmd } => match cmd {
            ToolsetsCmd::List => emit(f, commands::toolsets_list(&paths)?),
            ToolsetsCmd::Show { id } => emit(f, commands::toolsets_show(&paths, &id)?),
            ToolsetsCmd::Enable { id } => emit(f, commands::set_activation(&paths, &id, false, Some(true))?),
            ToolsetsCmd::Disable { id } => emit(f, commands::set_activation(&paths, &id, false, Some(false))?),
        },
        Command::State => emit(f, commands::state(&paths)?),
        Command::Skills { cmd } => match cmd {
            SkillsCmd::List => emit(f, commands::skills_list(&paths)?),
            SkillsCmd::Show { id } => emit(f, commands::skills_show(&paths, &id)?),
            SkillsCmd::Fetch { id } => emit(f, commands::skills_fetch(&paths, &id)?),
        },
        Command::Install { id, method, dry_run, yes, create_venv, auto_install_manual, force_install_manual } => {
            let opts = InstallOpts {
                ids: id,
                method,
                dry_run,
                create_venv,
                auto_manual: auto_install_manual,
                force_manual: force_install_manual,
            };
            emit(f, install::install(&paths, &opts, &host(yes))?)
        }
        Command::Refresh => emit(f, commands::refresh(&paths)?),
        Command::Completion { shell } => {
            let name = match shell {
                Shell::Bash => "bash",
                Shell::Zsh => "zsh",
                Shell::Fish => "fish",
            };
            out(completion::script(name).expect("every ValueEnum shell has a script"));
            0
        }
        Command::Complete { kind } => {
            let ids = commands::complete_ids(&paths, if matches!(kind, IdKind::Tools) { "tools" } else { "toolsets" })?;
            if !ids.is_empty() {
                out(&(ids.join("\n") + "\n"));
            }
            0
        }
        Command::DiffConfig { file } => emit(f, config::diff_config(&paths, file.map(ConfigFile::name))?),
        Command::OverwriteConfig { file, yes } => {
            emit(f, config::overwrite_config(&paths, file.map(ConfigFile::name), &host(yes))?)
        }
        Command::Setup { no_skills, no_completions, no_launcher, dry_run } => emit(
            f,
            config::setup(&paths, SetupOpts { skills: !no_skills, completions: !no_completions, launcher: !no_launcher, dry_run })?,
        ),
    })
}

/// argparse let a flag repeat with the last one winning, and callers rely on it:
/// the GUI's client appends `--format json` to arguments that already carry it.
/// clap rejects that by default, so every flag on every subcommand is told to
/// override itself. (List-valued options such as `--tag` are untouched: those
/// are declared to append.)
fn tolerant(cmd: clap::Command) -> clap::Command {
    cmd.args_override_self(true).mut_subcommands(tolerant)
}

fn main() -> ExitCode {
    let matches = tolerant(Cli::command()).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let format = cli.format;
    match run(cli) {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(e) => {
            if format == Format::Json {
                out(&(json_error(&e.to_string()) + "\n"));
            } else {
                eprintln!("reactor: {e}");
            }
            ExitCode::from(1)
        }
    }
}
