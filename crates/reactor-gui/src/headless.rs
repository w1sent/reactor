//! Commands that run without a window
//! ([ADR-0045](../../../docs/adr/0045-a-case-is-the-working-directory.md)): automation
//! over SSH and in scripts.
//!
//! [`dispatch`] is called before gpui starts, so nothing here needs a display. It
//! answers `None` for anything that is not a headless command, and the caller opens the
//! window as before.

use std::io::Write;
use std::path::{Path, PathBuf};

use reactor_db::{Case, case, corpus, export, sessions};

const USAGE: &str = "usage:
  reactor-gui case create <dir> [--title TEXT]   make a folder a case
  reactor-gui case save <dir> [--wait]           save a folder's unsaved state as its case
  reactor-gui case discard <dir> [--wait]        delete a folder's unsaved state
  reactor-gui case list                          the cases this machine knows
  reactor-gui case alias <dir> [<alias>|--clear]  the name references to the case are written with
                                                 (letters, digits, - and _); no argument shows it
  reactor-gui case get <dir> <reference> [--limit N] [--offset N]
                                                 read what a reference names (graph:<id>,
                                                 session:<id>#entry=<n>, graph:?type=net., ...)
  reactor-gui case sessions <dir>                the sessions of a case
  reactor-gui case export-session <dir> <id>     a session as JSON lines, on stdout
  reactor-gui case export <dir> <file.zip> [--password P]
                                                 the whole case as one encrypted archive
                                                 (default password: infected)
  reactor-gui case import <file.zip> <dir> [--password P]
                                                 unpack an exported case into an empty folder
                                                 (a password given on the command line is visible to
                                                 other users; REACTOR_CASE_PASSWORD is not)
  reactor-gui case backup <dir> <file>           a consistent copy of the case's database
  reactor-gui ingest <dir> <files, folders or URLs…>
                                                 add context documents to a case: converted,
                                                 cut into passages, searchable
  reactor-gui embed <dir>                        make the vectors the passages are missing, with the
                                                 embedding model in settings.json
  reactor-gui case rebuild-indexes <dir>         regenerate the keyword index from the passages and
                                                 drop the vectors, to be made again
  reactor-gui graph export <dir> <file|-> [--format stix|elements|edges|edge-list|graphml]
                           [--include-deleted] [--max-participants N] [--max-prevalence N]
                                                 the case's graph for other tools: a STIX 2.1 bundle,
                                                 two CSV tables, or the entity-to-entity projection
  reactor-gui graph import <dir> <bundle.json>   a STIX 2.1 bundle into the case's graph
  reactor-gui graph check <bundle.json>          structural problems in a STIX bundle
  reactor-gui docs eval <dir> <questions.json> [--k N] [--json]
                                                 how well the documents are searched: your questions, and
                                                 where each answer is, run by keyword, by meaning and both
  reactor-gui db selfcheck                       does this build's SQLite have what it needs";

/// Run `args` if they name a headless command; the exit code if they did.
pub fn dispatch(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> Option<i32> {
    dispatch_in(&reactor_core::Paths::from_env().config_dir, args, out, err)
}

/// [`dispatch`] against a given config directory.
pub fn dispatch_in(
    config_dir: &Path,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Option<i32> {
    let run = match args.first().map(String::as_str) {
        Some("db") => db(&args[1..], out),
        Some("case") => case_command(config_dir, &args[1..], out),
        Some("ingest") => ingest(config_dir, &args[1..], out),
        Some("graph") => graph_command(&args[1..], out),
        Some("embed") => embed(config_dir, &args[1..], out),
        Some("docs") => docs_command(config_dir, &args[1..], out),
        _ => return None,
    };
    Some(match run {
        Ok(()) => 0,
        Err(Failure::Usage) => {
            let _ = writeln!(err, "{USAGE}");
            2
        }
        Err(Failure::Error(message)) => {
            let _ = writeln!(err, "reactor-gui: {message}");
            1
        }
    })
}

/// How long `--wait` gives a closing window to release the case.
const WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Whether a failure means "someone still has it open" rather than "this cannot work".
pub fn is_busy(message: &str) -> bool {
    message.contains("open in a window") || message.contains("still in use")
}

/// Save `dir`'s unsaved state as its case, waiting for a closing window to let go. For
/// the relaunch after *Save as Case*: the new process calls this before it opens.
pub fn save_when_free(config_dir: &Path, dir: &Path) -> Result<(), String> {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let attempt = Case::unsaved(dir, config_dir).and_then(Case::save);
        match attempt {
            Ok(case) => {
                let _ = case::remember(config_dir, case.root());
                return Ok(());
            }
            Err(e) if is_busy(&e.to_string()) && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

enum Failure {
    Usage,
    Error(String),
}

impl From<reactor_db::Error> for Failure {
    fn from(e: reactor_db::Error) -> Failure {
        Failure::Error(e.to_string())
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Failure {
        Failure::Error(e.to_string())
    }
}

/// `graph export|import|check`.
fn graph_command(args: &[String], out: &mut dyn Write) -> Result<(), Failure> {
    use reactor_db::interchange::{projection, stix, tables};
    let Some((cmd, rest)) = args.split_first() else {
        return Err(Failure::Usage);
    };
    let read_json = |path: &str| -> Result<serde_json::Value, Failure> {
        let text =
            std::fs::read_to_string(path).map_err(|e| Failure::Error(format!("{path}: {e}")))?;
        serde_json::from_str(&text).map_err(|e| Failure::Error(format!("{path}: not JSON ({e})")))
    };
    match (cmd.as_str(), rest) {
        ("check", [file]) => {
            let problems = stix::validate(&read_json(file)?);
            if problems.is_empty() {
                writeln!(out, "{file}: no structural problems found")?;
                Ok(())
            } else {
                for p in &problems {
                    writeln!(out, "{p}")?;
                }
                Err(Failure::Error(format!("{} problem(s)", problems.len())))
            }
        }
        ("import", [dir, file]) => {
            let case = Case::open(Path::new(dir))?;
            let name = Path::new(file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let done = stix::import(&case, &read_json(file)?, &name)?;
            for s in &done.skipped {
                writeln!(out, "not imported: {s}")?;
            }
            writeln!(
                out,
                "{} element(s), {} edge(s) {}",
                done.elements,
                done.edges,
                if done.restored {
                    "restored exactly"
                } else {
                    "mapped from another tool's bundle"
                }
            )?;
            Ok(())
        }
        ("export", [dir, file, flags @ ..]) => {
            let (mut format, mut deleted) = ("stix".to_string(), false);
            let mut opts = projection::Options::default();
            let mut i = 0;
            while i < flags.len() {
                let number = |i: usize| -> Result<usize, Failure> {
                    flags
                        .get(i)
                        .and_then(|v| v.parse().ok())
                        .ok_or(Failure::Usage)
                };
                match flags[i].as_str() {
                    "--format" => {
                        i += 1;
                        format = flags.get(i).ok_or(Failure::Usage)?.clone();
                    }
                    "--include-deleted" => deleted = true,
                    "--max-participants" => {
                        i += 1;
                        opts.max_participants = number(i)?;
                    }
                    "--max-prevalence" => {
                        i += 1;
                        opts.max_prevalence = Some(number(i)?);
                    }
                    _ => return Err(Failure::Usage),
                }
                i += 1;
            }
            opts.include_deleted = deleted;
            let case = Case::open(Path::new(dir))?;
            let text = match format.as_str() {
                "stix" => {
                    let bundle = stix::export(
                        &case,
                        &stix::ExportOptions {
                            include_deleted: deleted,
                        },
                    )?;
                    serde_json::to_string_pretty(&bundle)
                        .map_err(|e| Failure::Error(e.to_string()))?
                        + "\n"
                }
                "elements" => tables::elements(&case, deleted)?,
                "edges" => tables::edges(&case, deleted)?,
                "edge-list" | "graphml" => {
                    let p = projection::project(case.db(), &opts)?;
                    if p.skipped_large > 0 || !p.skipped_prevalent.is_empty() {
                        eprintln!(
                            "reactor-gui: left out {} claim(s) with more than {} participants and {} entity(ies) in more than {} claims",
                            p.skipped_large,
                            opts.max_participants,
                            p.skipped_prevalent.len(),
                            opts.max_prevalence.unwrap_or(0)
                        );
                    }
                    if format == "graphml" {
                        projection::graphml(&p)
                    } else {
                        projection::edge_list_csv(&p)
                    }
                }
                other => {
                    return Err(Failure::Error(format!(
                        "`{other}` is not a format (stix, elements, edges, edge-list, graphml)"
                    )));
                }
            };
            if file == "-" {
                write!(out, "{text}")?;
            } else {
                if Path::new(file).exists() {
                    return Err(Failure::Error(format!("{file}: already exists")));
                }
                std::fs::write(file, &text)?;
                writeln!(out, "{file}: {} bytes", text.len())?;
            }
            Ok(())
        }
        _ => Err(Failure::Usage),
    }
}

/// `embed <dir>`.
fn embed(config_dir: &Path, args: &[String], out: &mut dyn Write) -> Result<(), Failure> {
    match args {
        [dir] => embed_case(config_dir, &Case::open(Path::new(dir))?, out),
        _ => Err(Failure::Usage),
    }
}

/// `docs eval <dir> <questions.json> [--k N] [--json]`: run a person's questions against the
/// case's documents and say how the search did (`reactor_agent::eval`). Meaning is tried when
/// a model is chosen and has made every vector; otherwise only keywords, and it says so.
fn docs_command(config_dir: &Path, args: &[String], out: &mut dyn Write) -> Result<(), Failure> {
    let [verb, dir, file, rest @ ..] = args else {
        return Err(Failure::Usage);
    };
    if verb != "eval" {
        return Err(Failure::Usage);
    }
    let (mut k, mut json) = (10usize, false);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--k" => {
                k = it
                    .next()
                    .and_then(|n| n.parse().ok())
                    .filter(|n| *n > 0)
                    .ok_or(Failure::Usage)?
            }
            "--json" => json = true,
            _ => return Err(Failure::Usage),
        }
    }
    let case = Case::open(Path::new(dir))?;
    let questions: Vec<reactor_agent::eval::Question> =
        serde_json::from_str(&std::fs::read_to_string(file)?).map_err(|e| {
            Failure::Error(format!(
                "{file}: a list of {{\"query\": …, \"expect\": […]}} was expected: {e}"
            ))
        })?;
    if questions.is_empty() {
        return Err(Failure::Error(format!("{file}: no questions")));
    }
    if !corpus::has_documents(case.db())? {
        return Err(Failure::Error("the case has no documents to search".into()));
    }
    let settings =
        reactor_context::settings::Settings::load_from(&reactor_context::settings::Settings::path(
            &reactor_core::Paths::new(config_dir, reactor_core::paths::Shipped::Embedded),
        ));
    let embedder = match &settings.embedding.model {
        Some(spec) => {
            Some(reactor_agent::embed::from_spec(spec).map_err(|e| Failure::Error(e.to_string()))?)
        }
        None => None,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let report = runtime
        .block_on(reactor_agent::eval::run(
            &case,
            embedder.as_deref(),
            &questions,
            k,
        ))
        .map_err(|e| Failure::Error(e.to_string()))?;
    if json {
        writeln!(
            out,
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| Failure::Error(e.to_string()))?
        )?;
    } else {
        writeln!(out, "{}", reactor_agent::eval::render(&report, &questions))?;
    }
    Ok(())
}

/// `ingest <dir> <sources…>`: into the case in `dir`, which must already be one.
fn ingest(config_dir: &Path, args: &[String], out: &mut dyn Write) -> Result<(), Failure> {
    let Some((dir, sources)) = args.split_first().filter(|(_, s)| !s.is_empty()) else {
        return Err(Failure::Usage);
    };
    let case = Case::open(Path::new(dir))?;
    let items = crate::import::plan(&case, sources).map_err(Failure::Error)?;
    if items.is_empty() {
        return Err(Failure::Error("nothing to import".into()));
    }
    let summary = crate::import::run(&case, &items, &mut |done, _, _| {
        use crate::import::Outcome::*;
        let _ = match &done.outcome {
            Stored { chunks, replaced } if *replaced > 0 => writeln!(
                out,
                "+ {} ({chunks} passages, replaces an earlier version)",
                done.source
            ),
            Stored { chunks, .. } => writeln!(out, "+ {} ({chunks} passages)", done.source),
            Unchanged => writeln!(out, "= {} (unchanged)", done.source),
            Failed(why) => writeln!(out, "! {}: {why}", done.source),
        };
    });
    writeln!(
        out,
        "{} added, {} unchanged, {} failed — {} passages in all",
        summary.stored,
        summary.unchanged,
        summary.failures.len(),
        corpus::counts(case.db())?.1
    )?;
    // The vectors the new passages need, if the user has chosen a model.
    if summary.stored > 0 {
        embed_case(config_dir, &case, out)?;
    }
    if summary.failures.is_empty() {
        Ok(())
    } else {
        Err(Failure::Error(format!(
            "{} source(s) could not be imported",
            summary.failures.len()
        )))
    }
}

/// Make the vectors a case is missing, with the model the settings name. With none chosen
/// there is nothing to do, and that is said once.
fn embed_case(config_dir: &Path, case: &Case, out: &mut dyn Write) -> Result<(), Failure> {
    let settings =
        reactor_context::settings::Settings::load_from(&reactor_context::settings::Settings::path(
            &reactor_core::Paths::new(config_dir, reactor_core::paths::Shipped::Embedded),
        ));
    let Some(spec) = settings.embedding.model else {
        writeln!(
            out,
            "no embedding model is chosen (embedding.model in settings.json): documents are searched by keyword only"
        )?;
        return Ok(());
    };
    let embedder =
        reactor_agent::embed::from_spec(&spec).map_err(|e| Failure::Error(e.to_string()))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let done = runtime
        .block_on(reactor_agent::embed::embed_pending(
            case,
            embedder.as_ref(),
            &tokio_util::sync::CancellationToken::new(),
            &mut |_| {},
        ))
        .map_err(|e| {
            Failure::Error(format!(
                "embedding stopped: {e} -- it continues where it left off next time"
            ))
        })?;
    writeln!(
        out,
        "embedded {} passage(s) with {spec}: {}/{} done",
        done.now, done.progress.embedded, done.progress.total
    )?;
    Ok(())
}

fn db(args: &[String], out: &mut dyn Write) -> Result<(), Failure> {
    match args {
        [cmd] if cmd == "selfcheck" => {
            let report = reactor_db::selfcheck::run()
                .map_err(|e| Failure::Error(format!("self-check failed: {e}")))?;
            writeln!(out, "{report}")?;
            Ok(())
        }
        _ => Err(Failure::Usage),
    }
}

/// The arguments with `--password P` taken out, and the password: that one, else
/// `REACTOR_CASE_PASSWORD`, else the default.
fn split_password(args: &[String]) -> Result<(Vec<String>, String), Failure> {
    let (mut rest, mut password, mut i) = (Vec::new(), None, 0);
    while i < args.len() {
        if args[i] == "--password" {
            i += 1;
            password = Some(args.get(i).ok_or(Failure::Usage)?.clone());
        } else {
            rest.push(args[i].clone());
        }
        i += 1;
    }
    let password = password
        .or_else(|| {
            std::env::var("REACTOR_CASE_PASSWORD")
                .ok()
                .filter(|p| !p.is_empty())
        })
        .unwrap_or_else(|| export::DEFAULT_PASSWORD.to_string());
    Ok((rest, password))
}

/// `<dir> [--title TEXT]`, in either order.
fn dir_and_title(args: &[String]) -> Option<(PathBuf, Option<String>)> {
    let (mut dir, mut title, mut i) = (None, None, 0);
    while i < args.len() {
        match args[i].as_str() {
            "--title" => {
                i += 1;
                title = Some(args.get(i)?.clone());
            }
            flag if flag.starts_with('-') => return None,
            path if dir.is_none() => dir = Some(PathBuf::from(path)),
            _ => return None,
        }
        i += 1;
    }
    Some((dir?, title))
}

fn case_command(config_dir: &Path, args: &[String], out: &mut dyn Write) -> Result<(), Failure> {
    let Some((cmd, rest)) = args.split_first() else {
        return Err(Failure::Usage);
    };
    match (cmd.as_str(), rest) {
        ("create", rest) => {
            let (dir, title) = dir_and_title(rest).ok_or(Failure::Usage)?;
            if Case::has_unsaved(&dir, config_dir) {
                return Err(Failure::Error(format!(
                    "{}: has an unsaved state -- save it with `reactor-gui case save`, or discard it in the window",
                    dir.display()
                )));
            }
            let case = Case::create(&dir, title.as_deref())?;
            case::remember(config_dir, case.root())?;
            writeln!(out, "{}  {}", case.meta().id, case.root().display())?;
            Ok(())
        }
        ("save" | "discard", [dir, wait]) if wait == "--wait" => {
            // A window that is closing asked for this: give it time to let go.
            let deadline = std::time::Instant::now() + WAIT;
            loop {
                match case_command(config_dir, &[cmd.clone(), dir.clone()], out) {
                    Err(Failure::Error(m))
                        if is_busy(&m) && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    other => return other,
                }
            }
        }
        ("discard", [dir]) => {
            let dir = Path::new(dir);
            if !Case::has_unsaved(dir, config_dir) {
                return Err(Failure::Error(format!(
                    "{}: no unsaved state belongs to this folder",
                    dir.display()
                )));
            }
            Case::unsaved(dir, config_dir)?.discard()?;
            Ok(())
        }
        ("save", [dir]) => {
            let dir = Path::new(dir);
            if Case::is_case(dir) {
                return Err(Failure::Error(format!("{}: already a case", dir.display())));
            }
            if !Case::has_unsaved(dir, config_dir) {
                return Err(Failure::Error(format!(
                    "{}: nothing to save -- no unsaved state belongs to this folder",
                    dir.display()
                )));
            }
            let case = Case::unsaved(dir, config_dir)?.save()?;
            case::remember(config_dir, case.root())?;
            writeln!(out, "{}  {}", case.meta().id, case.root().display())?;
            Ok(())
        }
        ("list", []) => {
            for known in case::known(config_dir) {
                match known.meta {
                    Some(meta) => writeln!(
                        out,
                        "{}  {}  {}",
                        meta.id,
                        known.root.display(),
                        meta.title.as_deref().unwrap_or("")
                    )?,
                    None => writeln!(out, "(gone)  {}", known.root.display())?,
                }
            }
            Ok(())
        }
        ("alias", [dir, rest @ ..]) if rest.len() <= 1 => {
            let case = Case::open(Path::new(dir))?;
            match rest.first().map(String::as_str) {
                None => writeln!(
                    out,
                    "{}  {}",
                    case.alias().unwrap_or_else(|| "(none)".into()),
                    case.meta().id
                )?,
                Some(a) => {
                    case.set_alias((a != "--clear").then_some(a))?;
                    writeln!(out, "{}  {}", case.authority(), case.meta().id)?;
                }
            }
            Ok(())
        }
        ("get", [dir, reference, flags @ ..]) => {
            let (mut limit, mut offset, mut i) = (None, None, 0);
            while i < flags.len() {
                let n = flags.get(i + 1).and_then(|v| v.parse::<usize>().ok());
                match flags[i].as_str() {
                    "--limit" => limit = Some(n.ok_or(Failure::Usage)?),
                    "--offset" => offset = Some(n.ok_or(Failure::Usage)?),
                    _ => return Err(Failure::Usage),
                }
                i += 2;
            }
            let case = Case::open(Path::new(dir))?;
            let r = reactor_db::reference::parse(reference)?;
            let text = reactor_agent::tools::case::read(
                &case,
                None,
                reactor_agent::tools::case::Access::ALL,
                &r,
                limit,
                offset,
            )
            .map_err(Failure::Error)?;
            writeln!(out, "{text}")?;
            Ok(())
        }
        ("sessions", [dir]) => {
            let case = Case::open(Path::new(dir))?;
            for s in sessions::list(case.db())? {
                writeln!(
                    out,
                    "{}  {}  {} entries  {}",
                    s.id,
                    s.created_at,
                    s.entries,
                    s.first_prompt
                        .as_deref()
                        .and_then(|p| p.lines().next())
                        .unwrap_or("")
                )?;
            }
            Ok(())
        }
        ("export-session", [dir, session]) => {
            let case = Case::open(Path::new(dir))?;
            sessions::export_jsonl(case.db(), session, out)?;
            Ok(())
        }
        ("rebuild-indexes", [dir]) => {
            let case = Case::open(Path::new(dir))?;
            corpus::rebuild_indexes(case.db())?;
            let (docs, chunks) = corpus::counts(case.db())?;
            writeln!(
                out,
                "keyword index rebuilt from {chunks} passages of {docs} document(s); vectors dropped -- run `reactor-gui embed` to make them again"
            )?;
            Ok(())
        }
        ("export", rest) => {
            let (positional, password) = split_password(rest)?;
            let [dir, file] = positional.as_slice() else {
                return Err(Failure::Usage);
            };
            let case = Case::open(Path::new(dir))?;
            let done = export::export(&case, Path::new(file), &password)?;
            for s in &done.skipped {
                writeln!(out, "left out: {s}")?;
            }
            writeln!(
                out,
                "{file}: {} files, {} bytes, encrypted with {}",
                done.files,
                done.bytes,
                if password == export::DEFAULT_PASSWORD {
                    "the default password (it prevents accidents, it is not a secret)"
                } else {
                    "your password"
                }
            )?;
            Ok(())
        }
        ("import", rest) => {
            let (positional, password) = split_password(rest)?;
            let [file, dir] = positional.as_slice() else {
                return Err(Failure::Usage);
            };
            let (case, done) = export::import(Path::new(file), Path::new(dir), &password)?;
            case::remember(config_dir, case.root())?;
            for sha in &done.missing_blobs {
                writeln!(out, "missing from the archive: blob {sha}")?;
            }
            writeln!(
                out,
                "{}  {}  ({} files, {} bytes)",
                case.meta().id,
                case.root().display(),
                done.files,
                done.bytes
            )?;
            Ok(())
        }
        ("backup", [dir, file]) => {
            let case = Case::open(Path::new(dir))?;
            case.db().backup_to(Path::new(file))?;
            Ok(())
        }
        _ => Err(Failure::Usage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_in(config: &Path, args: &[&str]) -> (Option<i32>, String, String) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = dispatch_in(config, &args, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn run(args: &[&str]) -> (Option<i32>, String, String) {
        let config = tempfile::tempdir().unwrap();
        run_in(config.path(), args)
    }

    #[test]
    fn a_workdir_or_a_flag_is_not_a_headless_command() {
        assert_eq!(run(&["/work/sample", "-c"]).0, None);
        assert_eq!(run(&[]).0, None);
        assert_eq!(run(&["-r"]).0, None);
    }

    #[test]
    fn selfcheck_reports_the_build_and_exits_zero() {
        let (code, out, err) = run(&["db", "selfcheck"]);
        assert_eq!(code, Some(0), "{err}");
        assert!(out.contains("sqlite-vec  v"), "{out}");
        assert!(out.contains("fts5        ok"), "{out}");
    }

    #[test]
    fn an_unknown_command_is_a_usage_error() {
        for args in [
            &["db", "nope"][..],
            &["case"],
            &["case", "create"],
            &["case", "list", "x"],
        ] {
            let (code, _, err) = run(args);
            assert_eq!(code, Some(2), "{args:?}");
            assert!(err.contains("usage:"));
        }
    }

    #[test]
    fn a_case_is_created_listed_exported_and_backed_up_without_a_window() {
        use reactor_agent::entry::Kind;
        use reactor_agent::store::Store;
        let (config, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let dir = work.path().to_str().unwrap();

        let (code, out, err) = run_in(
            config.path(),
            &["case", "create", dir, "--title", "Incident 17"],
        );
        assert_eq!(code, Some(0), "{err}");
        assert!(Case::is_case(work.path()));
        let id = out.split_whitespace().next().unwrap().to_string();
        assert_eq!(
            run_in(config.path(), &["case", "create", dir]).0,
            Some(1),
            "not twice"
        );

        let (_, out, _) = run_in(config.path(), &["case", "list"]);
        assert!(out.contains(&id) && out.contains("Incident 17"), "{out}");

        {
            let case = Case::open(work.path()).unwrap();
            let mut s = Store::create(&case, "s1").unwrap();
            s.append(Kind::User {
                text: "look at a.out".into(),
            })
            .unwrap();
        }
        let (code, out, _) = run_in(config.path(), &["case", "sessions", dir]);
        assert_eq!(code, Some(0));
        assert!(
            out.contains("s1") && out.contains("2 entries  look at a.out"),
            "{out}"
        );

        let (code, out, _) = run_in(config.path(), &["case", "export-session", dir, "s1"]);
        assert_eq!(code, Some(0));
        assert_eq!(out.lines().count(), 2);
        assert!(
            out.lines()
                .all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok())
        );
        assert_eq!(
            run_in(config.path(), &["case", "export-session", dir, "nope"]).0,
            Some(1)
        );

        let copy = work.path().join("copy.db");
        let (code, _, err) = run_in(
            config.path(),
            &["case", "backup", dir, copy.to_str().unwrap()],
        );
        assert_eq!(code, Some(0), "{err}");
        assert!(copy.is_file());
    }

    #[test]
    fn an_unsaved_state_is_saved_headless_unless_a_window_has_it_open() {
        use reactor_agent::store::Store;
        let (config, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let dir = work.path().to_str().unwrap();

        let (code, _, err) = run_in(config.path(), &["case", "save", dir]);
        assert_eq!(code, Some(1));
        assert!(err.contains("nothing to save"), "{err}");

        let case = Case::open_any(work.path(), config.path()).unwrap();
        let open = Store::create(&case, "s1").unwrap();
        drop(case);
        let (code, _, err) = run_in(config.path(), &["case", "save", dir]);
        assert_eq!(code, Some(1));
        assert!(err.contains("open in a window"), "{err}");
        assert!(!Case::is_case(work.path()));
        // And creating a second, empty case over it would orphan the work.
        assert_eq!(run_in(config.path(), &["case", "create", dir]).0, Some(1));

        // With --wait it holds on until the window lets go.
        let closing = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(open);
        });
        let (code, _, err) = run_in(config.path(), &["case", "save", dir, "--wait"]);
        closing.join().unwrap();
        assert_eq!(code, Some(0), "{err}");
        assert!(Case::is_case(work.path()));
        assert_eq!(
            sessions::list(Case::open(work.path()).unwrap().db())
                .unwrap()
                .len(),
            1
        );
        let (_, out, _) = run_in(config.path(), &["case", "list"]);
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn an_unsaved_state_is_discarded_headless_and_the_folder_is_untouched() {
        let (config, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let dir = work.path().to_str().unwrap();
        std::fs::write(work.path().join("notes.txt"), "the agent wrote this").unwrap();
        assert_eq!(run_in(config.path(), &["case", "discard", dir]).0, Some(1));

        drop(Case::open_any(work.path(), config.path()).unwrap());
        assert!(Case::has_unsaved(work.path(), config.path()));
        let (code, _, err) = run_in(config.path(), &["case", "discard", dir, "--wait"]);
        assert_eq!(code, Some(0), "{err}");
        assert!(!Case::has_unsaved(work.path(), config.path()));
        assert!(work.path().join("notes.txt").is_file());
        assert!(!Case::is_case(work.path()));
    }

    #[test]
    fn the_relaunch_saves_once_the_closing_window_has_let_go() {
        use reactor_agent::entry::Kind;
        use reactor_agent::store::Store;
        let (config, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let case = Case::open_any(work.path(), config.path()).unwrap();
        let mut open = Store::create(&case, "s1").unwrap();
        open.append(Kind::User {
            text: "hello".into(),
        })
        .unwrap();
        drop(case);
        let closing = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(open);
        });
        save_when_free(config.path(), work.path()).unwrap();
        closing.join().unwrap();
        // The same session is there to reopen, now in the folder's own case.
        let case = Case::open(work.path()).unwrap();
        assert_eq!(Store::open(&case, "s1").unwrap().len(), 2);
        assert!(
            save_when_free(config.path(), work.path()).is_err(),
            "not twice"
        );
    }

    #[test]
    fn documents_are_ingested_headless_and_again_without_writing() {
        let (config, work, docs) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let dir = work.path().to_str().unwrap();
        std::fs::write(docs.path().join("a.md"), "# A\n\nfirst").unwrap();
        std::fs::write(docs.path().join("b.txt"), "second").unwrap();
        let src = docs.path().to_str().unwrap();

        // Not a case yet: nothing is created for it.
        let (code, _, err) = run_in(config.path(), &["ingest", dir, src]);
        assert_eq!(code, Some(1), "{err}");
        assert!(!Case::is_case(work.path()));

        run_in(config.path(), &["case", "create", dir]);
        let (code, out, err) = run_in(config.path(), &["ingest", dir, src]);
        assert_eq!(code, Some(0), "{err}");
        assert!(
            out.contains("+ ") && out.contains("a.md (1 passages)"),
            "{out}"
        );
        assert!(
            out.contains("2 added, 0 unchanged, 0 failed — 2 passages in all\n"),
            "{out}"
        );
        let (code, out, _) = run_in(config.path(), &["ingest", dir, src]);
        assert_eq!(code, Some(0));
        assert!(
            out.contains("= ") && out.contains("0 added, 2 unchanged"),
            "{out}"
        );

        // A source that cannot be imported is listed, the rest still go in, and the exit
        // code says something failed.
        std::fs::write(docs.path().join("c.bin"), b"\0\0\0").unwrap();
        let (code, out, err) = run_in(config.path(), &["ingest", dir, src]);
        assert_eq!(code, Some(1));
        assert!(
            out.contains("! ") && out.contains("c.bin") && err.contains("1 source(s)"),
            "{out}{err}"
        );
        assert_eq!(
            run_in(config.path(), &["ingest", dir]).0,
            Some(2),
            "no sources is a usage error"
        );
    }

    /// An Ollama server that answers `/api/embed` the way Ollama does.
    fn fake_ollama() -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let body = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break String::new();
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break text[end + 4..end + 4 + len].to_string();
                        }
                    }
                };
                let request: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let n = request["input"].as_array().map_or(0, Vec::len);
                let embeddings: Vec<Vec<f32>> =
                    (0..n).map(|i| vec![1.0, i as f32, 0.5, 0.0]).collect();
                let reply =
                    serde_json::json!({ "model": request["model"], "embeddings": embeddings })
                        .to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn documents_are_embedded_headless_when_a_model_is_chosen_and_the_indexes_can_be_rebuilt() {
        let (config, work, docs) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let (dir, src) = (work.path().to_str().unwrap(), docs.path().to_str().unwrap());
        std::fs::write(docs.path().join("a.md"), "# A\n\nfirst").unwrap();
        std::fs::write(docs.path().join("b.md"), "# B\n\nsecond").unwrap();
        run_in(config.path(), &["case", "create", dir]);

        // No model chosen: said once, and the documents are still in.
        let (code, out, _) = run_in(config.path(), &["ingest", dir, src]);
        assert_eq!(code, Some(0));
        assert!(out.contains("no embedding model is chosen"), "{out}");

        // A model chosen: `embed` makes what is missing.
        // SAFETY: the only test in this binary that builds an Ollama client.
        unsafe { std::env::set_var("OLLAMA_API_BASE_URL", fake_ollama()) };
        std::fs::write(
            config.path().join("settings.json"),
            r#"{"embedding":{"model":"ollama/fake-embed"}}"#,
        )
        .unwrap();
        let (code, out, err) = run_in(config.path(), &["embed", dir]);
        assert_eq!(code, Some(0), "{err}");
        assert!(
            out.contains("embedded 2 passage(s) with ollama/fake-embed: 2/2 done"),
            "{out}"
        );
        let (_, out, _) = run_in(config.path(), &["embed", dir]);
        assert!(
            out.contains("embedded 0 passage(s)") && out.contains("2/2 done"),
            "{out}"
        );

        // New documents are embedded as they are ingested.
        std::fs::write(docs.path().join("c.md"), "# C\n\nthird").unwrap();
        let (code, out, _) = run_in(config.path(), &["ingest", dir, src]);
        assert_eq!(code, Some(0));
        assert!(
            out.contains("1 added, 2 unchanged")
                && out.contains("embedded 1 passage(s)")
                && out.contains("3/3 done"),
            "{out}"
        );
        let (_, out, _) = run_in(config.path(), &["ingest", dir, src]);
        assert!(
            !out.contains("embedded"),
            "nothing new, nothing embedded: {out}"
        );

        // Rebuild: the vectors are dropped to be made again.
        let (code, out, _) = run_in(config.path(), &["case", "rebuild-indexes", dir]);
        assert_eq!(code, Some(0));
        assert!(out.contains("3 passages of 3 document(s)"), "{out}");
        let (_, out, _) = run_in(config.path(), &["embed", dir]);
        assert!(out.contains("embedded 3 passage(s)"), "{out}");

        // A model that cannot be set up is an error that says so.
        std::fs::write(
            config.path().join("settings.json"),
            r#"{"embedding":{"model":"anthropic/x"}}"#,
        )
        .unwrap();
        let (code, _, err) = run_in(config.path(), &["embed", dir]);
        assert_eq!(code, Some(1));
        assert!(err.contains("does not serve embeddings"), "{err}");
        assert_eq!(run_in(config.path(), &["embed"]).0, Some(2));
    }

    #[test]
    fn a_case_is_exported_and_imported_headless_with_the_default_or_a_given_password() {
        let (config, work, out) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let dir = work.path().to_str().unwrap();
        std::fs::write(work.path().join("sample.bin"), b"MZ pretend").unwrap();
        run_in(
            config.path(),
            &["case", "create", dir, "--title", "Incident 17"],
        );
        let archive = out.path().join("c.zip");
        let file = archive.to_str().unwrap();

        let (code, text, err) = run_in(config.path(), &["case", "export", dir, file]);
        assert_eq!(code, Some(0), "{err}");
        assert!(text.contains("the default password"), "{text}");
        assert_eq!(
            run_in(config.path(), &["case", "export", dir, file]).0,
            Some(1),
            "never overwrites"
        );

        let target = out.path().join("copy");
        let t = target.to_str().unwrap();
        let (code, text, err) = run_in(config.path(), &["case", "import", file, t]);
        assert_eq!(code, Some(0), "{err}");
        assert!(
            text.contains(&target.display().to_string()) && text.contains("files"),
            "{text}"
        );
        assert_eq!(
            std::fs::read(target.join("sample.bin")).unwrap(),
            b"MZ pretend"
        );
        let (_, list, _) = run_in(config.path(), &["case", "list"]);
        assert!(list.contains("Incident 17"), "{list}");

        // Their own password.
        let own = out.path().join("own.zip");
        let own = own.to_str().unwrap();
        let (code, text, _) = run_in(
            config.path(),
            &["case", "export", dir, own, "--password", "s3cret"],
        );
        assert_eq!(code, Some(0));
        assert!(
            text.contains("your password") && !text.contains("s3cret"),
            "{text}"
        );
        let t2 = out.path().join("copy2");
        let t2 = t2.to_str().unwrap();
        let (code, _, err) = run_in(config.path(), &["case", "import", own, t2]);
        assert_eq!(code, Some(1));
        assert!(err.contains("wrong password"), "{err}");
        assert_eq!(
            run_in(
                config.path(),
                &["case", "import", own, t2, "--password", "s3cret"]
            )
            .0,
            Some(0)
        );
        assert_eq!(run_in(config.path(), &["case", "export", dir]).0, Some(2));
        assert_eq!(
            run_in(config.path(), &["case", "export", dir, file, "--password"]).0,
            Some(2)
        );
    }

    #[test]
    fn the_graph_goes_to_other_tools_and_a_stix_bundle_comes_back_headless() {
        use reactor_db::graph::{self, Batch, Kind, Link, Put};
        use reactor_db::manifest::Author;
        let (config, a, b, out) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let (da, db) = (a.path().to_str().unwrap(), b.path().to_str().unwrap());
        run_in(config.path(), &["case", "create", da]);
        run_in(config.path(), &["case", "create", db]);
        let case = Case::open(a.path()).unwrap();
        graph::apply(
            case.db(),
            Batch::new(Author::analyst()).put(Put {
                links: vec![
                    Link::entity("src", "host.name", "ws-042"),
                    Link::entity("dst", "net.ipv4", "198.51.100.7"),
                    Link::source("artifact:fw.csv"),
                ],
                ..Put::new(Kind::Event, "net.connection")
            }),
        )
        .unwrap();
        drop(case);

        let bundle = out.path().join("g.json");
        let f = bundle.to_str().unwrap();
        let (code, text, err) = run_in(config.path(), &["graph", "export", da, f]);
        assert_eq!(code, Some(0), "{err}");
        assert!(text.contains("bytes"), "{text}");
        assert_eq!(
            run_in(config.path(), &["graph", "export", da, f]).0,
            Some(1),
            "never overwrites"
        );
        let (code, text, _) = run_in(config.path(), &["graph", "check", f]);
        assert_eq!(code, Some(0));
        assert!(text.contains("no structural problems"), "{text}");

        let (code, text, err) = run_in(config.path(), &["graph", "import", db, f]);
        assert_eq!(code, Some(0), "{err}");
        assert!(text.contains("restored exactly"), "{text}");
        let into = Case::open(b.path()).unwrap();
        assert_eq!(graph::query::relations(into.db(), None).unwrap().len(), 1);
        drop(into);

        for (format, needle) in [
            ("elements", "id,kind,type,value"),
            ("edges", "id,from_id,from_type"),
            ("edge-list", "source,target,weight,claim_types"),
            ("graphml", "<graphml"),
        ] {
            let (code, text, err) = run_in(
                config.path(),
                &["graph", "export", da, "-", "--format", format],
            );
            assert_eq!(code, Some(0), "{format}: {err}");
            assert!(text.contains(needle), "{format}: {text}");
        }
        assert_eq!(
            run_in(
                config.path(),
                &["graph", "export", da, "-", "--format", "nope"]
            )
            .0,
            Some(1)
        );
        assert_eq!(run_in(config.path(), &["graph", "export", da]).0, Some(2));
        std::fs::write(
            out.path().join("bad.json"),
            r#"{"type":"bundle","id":"x","objects":[]}"#,
        )
        .unwrap();
        let (code, text, _) = run_in(
            config.path(),
            &[
                "graph",
                "check",
                out.path().join("bad.json").to_str().unwrap(),
            ],
        );
        assert_eq!(code, Some(1));
        assert!(text.contains("bundle"), "{text}");
    }

    #[test]
    fn questions_about_the_documents_are_scored_headless() {
        let (config, work, docs) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let dir = work.path().to_str().unwrap();
        std::fs::write(
            docs.path().join("emotet.md"),
            "# Emotet\n\nThe loader survives reboots by writing a run key under HKCU.",
        )
        .unwrap();
        std::fs::write(
            docs.path().join("other.md"),
            "# Other\n\nNothing about that here.",
        )
        .unwrap();
        run_in(config.path(), &["case", "create", dir]);
        let src = docs.path().to_str().unwrap();
        run_in(config.path(), &["ingest", dir, src]);
        let file = docs.path().join("questions.json");
        std::fs::write(
            &file,
            r#"[
              {"query": "run key HKCU", "expect": ["emotet"]},
              {"query": "unobtainium", "expect": ["emotet"]}
            ]"#,
        )
        .unwrap();
        let f = file.to_str().unwrap();

        // No model is chosen: keyword only, said so, one found and one missed.
        let (code, out, err) = run_in(config.path(), &["docs", "eval", dir, f]);
        assert_eq!(code, Some(0), "{err}");
        assert!(out.contains("keyword  first 1/2"), "{out}");
        assert!(out.contains("meaning was not tried"), "{out}");
        assert!(out.contains("not found (keyword): unobtainium"), "{out}");

        let (code, out, _) = run_in(
            config.path(),
            &["docs", "eval", dir, f, "--json", "--k", "3"],
        );
        assert_eq!(code, Some(0));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["k"], 3);
        assert_eq!(v["modes"][0]["first"], 1);

        // What is wrong with the call is said, not guessed at.
        assert_eq!(run_in(config.path(), &["docs", "eval", dir]).0, Some(2));
        std::fs::write(&file, "{}").unwrap();
        let (code, _, err) = run_in(config.path(), &["docs", "eval", dir, f]);
        assert_eq!(code, Some(1));
        assert!(err.contains("a list of"), "{err}");
    }

    #[test]
    fn a_case_has_an_alias_and_a_reference_is_read_without_a_window() {
        let config = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let dir = work.path().to_str().unwrap();
        std::fs::write(work.path().join("a.csv"), "x\n1\n").unwrap();
        run_in(config.path(), &["case", "create", dir]);

        let (code, out, _) = run_in(config.path(), &["case", "alias", dir]);
        assert_eq!(code, Some(0));
        assert!(out.starts_with("(none)"), "{out}");
        let (code, out, err) = run_in(config.path(), &["case", "alias", dir, "acme"]);
        assert_eq!(code, Some(0), "{err}");
        assert!(out.starts_with("acme  "), "{out}");
        let (code, _, err) = run_in(config.path(), &["case", "alias", dir, "has space"]);
        assert_eq!(code, Some(1));
        assert!(err.contains("an alias is"), "{err}");

        let (code, out, err) = run_in(
            config.path(),
            &["case", "get", dir, "reactor://acme/artifact/a.csv"],
        );
        assert_eq!(code, Some(0), "{err}");
        assert!(out.contains("artifact:a.csv") && out.contains("not registered"), "{out}");
        let (code, out, _) = run_in(config.path(), &["case", "get", dir, "graph:?kind=entity"]);
        assert_eq!(code, Some(0));
        assert!(out.contains("no match"), "{out}");
        let (code, _, err) = run_in(config.path(), &["case", "get", dir, "reactor://other/graph/x"]);
        assert_eq!(code, Some(1));
        assert!(err.contains("another case"), "{err}");
        let (code, _, _) = run_in(config.path(), &["case", "get", dir, "nonsense"]);
        assert_eq!(code, Some(1));
    }
}
