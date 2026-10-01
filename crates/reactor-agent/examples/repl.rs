//! A development harness, not a product: the agent in a terminal, a line-oriented REPL
//! over the same [`reactor_agent::agent::Agent`] the GUI will drive.
//!
//! REactor has no TUI by design (ADR-0033); the GUI is the frontend. This exists only so
//! the loop can be run against a real model before the GUI runs on it (MIGRATE.md phase
//! 4's gate), and to reproduce bugs. It is an *example*: never built by default, never
//! installed by `cargo install`, and it goes away once phase 5 lands.
//!
//! ```text
//! cargo run -p reactor-agent --example repl -- --model anthropic/claude-sonnet-5-5 [--window N] [--mode auto|fade|compact]
//!               [--summarizer provider/name] [--cwd DIR] [--resume SESSION_DIR]
//!               [--scenarios DIR] [--skills DIR]
//! ```
//! Lines starting with `/` are commands: `/goal`, `/guidelines`, `/manifest`, `/frame`,
//! `/identity`, `/report`, `/reactor-scenario`, `/preview`, `/reduce [mode]`, `/undo`,
//! `/history [from to]`, `/quit`. Anything else is a prompt. Ctrl-C cancels the turn.

use std::io::Write as _;
use std::path::PathBuf;

use reactor_agent::agent::{Agent, AgentConfig, Event};
use reactor_agent::context::{active_reductions, context_tokens_of};
use reactor_agent::entry::{Kind, Mode};
use reactor_agent::history;
use reactor_agent::llm::LlmSummarizer;
use reactor_agent::provider::{AnyLlm, default_window};
use reactor_agent::store::Store;
use reactor_agent::tools::Tools;
use reactor_core::Paths;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

struct Args {
    model: Option<String>,
    summarizer: Option<String>,
    window: Option<u64>,
    mode: Mode,
    cwd: PathBuf,
    resume: Option<PathBuf>,
    scenarios: Option<PathBuf>,
    skills: Option<PathBuf>,
}

fn parse_mode(s: &str) -> Option<Mode> {
    match s {
        "fade" => Some(Mode::Fade),
        "compact" => Some(Mode::Compact),
        "auto" => Some(Mode::Auto),
        _ => None,
    }
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        model: std::env::var("REACTOR_MODEL").ok(),
        summarizer: None,
        window: None,
        mode: Mode::Auto,
        cwd: std::env::current_dir().map_err(|e| e.to_string())?,
        resume: None,
        scenarios: None,
        skills: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match flag.as_str() {
            "--model" => a.model = Some(value("--model")?),
            "--summarizer" => a.summarizer = Some(value("--summarizer")?),
            "--window" => {
                a.window = Some(
                    value("--window")?
                        .parse()
                        .map_err(|_| "--window must be a number of tokens".to_string())?,
                )
            }
            "--mode" => {
                a.mode = parse_mode(&value("--mode")?).ok_or("--mode is fade, compact or auto")?
            }
            "--cwd" => a.cwd = PathBuf::from(value("--cwd")?),
            "--resume" => a.resume = Some(PathBuf::from(value("--resume")?)),
            "--scenarios" => a.scenarios = Some(PathBuf::from(value("--scenarios")?)),
            "--skills" => a.skills = Some(PathBuf::from(value("--skills")?)),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(a)
}

const USAGE: &str = "usage: cargo run -p reactor-agent --example repl -- --model provider/name [--window N] [--mode auto|fade|compact] [--summarizer provider/name] [--cwd DIR] [--resume SESSION_DIR] [--scenarios DIR] [--skills DIR]\n\nproviders: anthropic, openai, gemini, openrouter, ollama (credentials from the provider's usual environment variable)";

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("repl: {e}");
            }
            eprintln!("{USAGE}");
            std::process::exit(if e.is_empty() { 0 } else { 2 });
        }
    };
    if let Err(e) = run(args).await {
        eprintln!("repl: {e}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), String> {
    let spec = args
        .model
        .clone()
        .ok_or("no model: pass --model provider/name or set REACTOR_MODEL")?;
    let (llm, provider) = AnyLlm::from_spec(&spec).map_err(|e| e.to_string())?;
    let summarizer_llm = match &args.summarizer {
        Some(s) => AnyLlm::from_spec(s).map_err(|e| e.to_string())?.0,
        None => llm.clone(),
    };

    let paths = Paths::from_env();
    let store = match &args.resume {
        Some(dir) => Store::open(dir).map_err(|e| e.to_string())?,
        None => {
            let id = format!(
                "{}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                std::process::id()
            );
            Store::create(paths.config_dir.join("sessions").join(&id), &id, &args.cwd)
                .map_err(|e| e.to_string())?
        }
    };
    eprintln!("session {}", store.dir().display());

    let mut cfg = AgentConfig::new(
        args.cwd.clone(),
        paths,
        args.window.unwrap_or_else(|| default_window(&provider)),
    );
    cfg.budget.mode = args.mode;
    if let Some(s) = args
        .scenarios
        .or_else(|| Some(PathBuf::from("prompts/scenarios")).filter(|p| p.is_dir()))
    {
        cfg.scenarios_dir = s;
    }
    cfg.skills_dir = args
        .skills
        .or_else(|| Some(PathBuf::from("skills")).filter(|p| p.is_dir()));

    let (agent, mut events) = Agent::new(
        llm,
        LlmSummarizer {
            llm: summarizer_llm,
        },
        store,
        Tools::standard(args.cwd.clone()),
        cfg,
    );

    // Print what happens, as it happens.
    tokio::spawn(async move {
        while let Some(e) = events.recv().await {
            match e {
                Event::Text(t) => {
                    print!("{t}");
                    let _ = std::io::stdout().flush();
                }
                Event::Thinking(_) | Event::Appended(_) | Event::ToolCallStarted { .. } => {}
                Event::ToolStart { name, args, .. } => {
                    eprintln!("\n\x1b[36m→ {name} {args}\x1b[0m")
                }
                Event::ToolOutput { chunk, .. } => eprint!("\x1b[2m{chunk}\x1b[0m"),
                Event::ToolEnd {
                    entry, is_error, ..
                } => eprintln!(
                    "\x1b[36m  ← #{entry}{}\x1b[0m",
                    if is_error { " (error)" } else { "" }
                ),
                Event::Usage(u) => eprintln!(
                    "\x1b[2m[{} in / {} out tokens]\x1b[0m",
                    u.input_tokens, u.output_tokens
                ),
                Event::Reduced {
                    entry,
                    mode,
                    trigger,
                    before_tokens,
                    after_tokens,
                } => {
                    eprintln!(
                        "\x1b[33m[context reduced #{entry}: {mode:?} ({trigger:?}) ~{before_tokens} → ~{after_tokens} tokens]\x1b[0m"
                    )
                }
                Event::Notice(n) => eprintln!("\x1b[33m{n}\x1b[0m"),
                Event::Finished => println!(),
            }
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        eprint!("\n\x1b[1m> \x1b[0m");
        let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? else {
            break;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "/quit" || line == "/exit" {
            break;
        }
        let prompt = match line.strip_prefix('/') {
            Some(cmd) => match handle_command(&agent, cmd).await {
                Some(text) => text,
                None => continue,
            },
            None => line.to_string(),
        };
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let interrupt = tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            c2.cancel();
        });
        match agent.prompt(&prompt, cancel).await {
            Ok(o) => eprintln!(
                "\x1b[2m[{} round(s), {} tool call(s), {} reduction(s)]\x1b[0m",
                o.rounds, o.tool_calls, o.reductions
            ),
            Err(e) => eprintln!("\x1b[31mturn stopped: {e}\x1b[0m"),
        }
        interrupt.abort();
    }
    Ok(())
}

/// Run a `/command`. Returns text to send as a turn, if the command asks for one.
async fn handle_command<L, S>(agent: &Agent<L, S>, cmd: &str) -> Option<String>
where
    L: reactor_agent::llm::Llm,
    S: reactor_agent::budget::Summarizer,
{
    let (name, rest) = cmd
        .split_once(' ')
        .map(|(n, r)| (n, r.trim()))
        .unwrap_or((cmd, ""));
    match name {
        "preview" | "reduce" => {
            let mode = parse_mode(rest).unwrap_or(Mode::Compact);
            if name == "preview" {
                match agent.preview(mode).await {
                    Ok(p) => println!(
                        "would reduce {} entries ({} mechanical ~{} tokens, {} conceptual ~{} tokens), keeping the newest; est. {} -> {} tokens",
                        p.covers.len(),
                        p.mechanical.len(),
                        p.mechanical_tokens,
                        p.conceptual.len(),
                        p.conceptual_tokens,
                        p.before_tokens,
                        p.estimated_after_tokens
                    ),
                    Err(e) => println!("{e}"),
                }
            } else {
                match agent.reduce_now(mode).await {
                    Ok(id) => println!("reduced (#{id}); /undo restores it"),
                    Err(e) => println!("{e}"),
                }
            }
            None
        }
        "undo" => {
            let store = agent.store();
            let mut store = store.lock().unwrap();
            match active_reductions(&store).last().copied() {
                Some(r) => {
                    let _ = store.append(Kind::Restore { reduction: r });
                    println!("restored #{r}");
                }
                None => println!("no reduction in force"),
            }
            None
        }
        "history" => {
            let mut nums = rest
                .split_whitespace()
                .filter_map(|n| n.parse::<u64>().ok());
            let (from, to) = (nums.next(), nums.next());
            println!(
                "{}",
                history::index_listing(&agent.store().lock().unwrap(), from, to)
            );
            None
        }
        "context" => {
            println!(
                "~{} tokens in the projected messages",
                context_tokens_of(&agent.context())
            );
            None
        }
        other => match agent.command(other, rest) {
            Ok(r) => {
                for n in r.notices {
                    println!("{}", n.message);
                }
                r.trigger_turn
            }
            Err(e) => {
                println!("{e}");
                None
            }
        },
    }
}
