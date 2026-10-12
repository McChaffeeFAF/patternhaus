//! `patternhaus-cli`: the shell and MCP front end of Patternhaus (DESIGN §5).
//!
//! ```text
//! patternhaus-cli run pattern.add --params '{"name":"Beat"}'
//! patternhaus-cli script build.json
//! patternhaus-cli render --project ./song --name mix
//! patternhaus-cli analyze --project ./song --what loudness,key
//! patternhaus-cli mcp --headless --project ./song      # MCP over stdio
//! patternhaus-cli mcp --connect                        # proxy to the running app
//! ```
//!
//! Commands start a headless studio (silent audio) on `--project`, or talk to a running app
//! with `--connect`. Anything printed on stdout by `mcp` is MCP protocol; messages go to
//! stderr.

use clap::{Args, Parser, Subcommand};
use ph_control::{
    AudioMode, HeadlessOptions, default_endpoint, parse_grant, proxy_stdio, serve_stdio,
    start_headless,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "patternhaus-cli",
    version,
    about = "Drive Patternhaus from a shell or an MCP client"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Common {
    /// Project folder (opened if it holds a project.json). Default: the current folder when it
    /// holds a project, else ./patternhaus-project.
    #[arg(long, global = true)]
    project: Option<PathBuf>,
    /// Factory assets folder (default: found next to the executable or in the source tree).
    #[arg(long, global = true)]
    assets: Option<PathBuf>,
    /// Use the real audio device instead of the silent simulated one.
    #[arg(long, global = true)]
    audio: bool,
    /// Scopes for headless clients: read, compose, mix, full (comma separated).
    #[arg(long, global = true, default_value = "full")]
    grant: String,
    /// Talk to a running Patternhaus app instead of starting a headless studio.
    #[arg(long, global = true)]
    connect: bool,
    /// Endpoint of the running app (default: the per-user pipe or socket).
    #[arg(long, global = true)]
    endpoint: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run one command: `run note.add --params '{...}'`.
    Run {
        command: String,
        /// Parameters as a JSON object.
        #[arg(long, default_value = "{}")]
        params: String,
        /// Confirm a destructive command.
        #[arg(long)]
        confirm: bool,
        #[command(flatten)]
        common: Common,
    },
    /// Run a JSON script: a list of {command, params, confirm} (or {requests, as_one_undo}).
    Script {
        file: PathBuf,
        /// Make every request its own undo step (default: the whole script is one step).
        #[arg(long)]
        separate: bool,
        #[command(flatten)]
        common: Common,
    },
    /// Render the song to a file and print a quick analysis.
    Render {
        #[arg(long, default_value = "render")]
        name: String,
        #[arg(long, default_value = "wav")]
        format: String,
        #[arg(long)]
        bit_depth: Option<u32>,
        #[arg(long)]
        from_bar: Option<u32>,
        #[arg(long)]
        to_bar: Option<u32>,
        #[command(flatten)]
        common: Common,
    },
    /// Print analysis numbers (loudness, spectrum, health, onsets, key, chords, clashes, masking).
    Analyze {
        /// Comma separated; default is the standard set, `all` for everything.
        #[arg(long)]
        what: Option<String>,
        #[arg(long)]
        from_bar: Option<u32>,
        #[arg(long)]
        to_bar: Option<u32>,
        #[command(flatten)]
        common: Common,
    },
    /// Write a PNG snapshot: waveform, spectrogram, piano_roll or playlist.
    Snapshot {
        kind: String,
        /// Output PNG file.
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        pattern: Option<String>,
        #[arg(long)]
        from_bar: Option<u32>,
        #[arg(long)]
        to_bar: Option<u32>,
        #[command(flatten)]
        common: Common,
    },
    /// List every command.
    Commands {
        #[arg(long, default_value = "")]
        filter: String,
        #[command(flatten)]
        common: Common,
    },
    /// Serve MCP over stdio. `--headless` starts its own studio, `--connect` attaches to the
    /// running app; with neither, attach if an app is running, else go headless.
    Mcp {
        #[arg(long)]
        headless: bool,
        #[command(flatten)]
        common: Common,
    },
}

fn range(from: Option<u32>, to: Option<u32>) -> Value {
    match (from, to) {
        (Some(a), Some(b)) => json!({ "from_bar": a, "to_bar": b }),
        (Some(a), None) => json!({ "from_bar": a, "to_bar": 100000 }),
        (None, Some(b)) => json!({ "from_bar": 1, "to_bar": b }),
        _ => Value::Null,
    }
}

fn project_dir(c: &Common) -> PathBuf {
    c.project.clone().unwrap_or_else(|| {
        if PathBuf::from("project.json").exists() {
            PathBuf::from(".")
        } else {
            PathBuf::from("patternhaus-project")
        }
    })
}

fn endpoint(c: &Common) -> String {
    c.endpoint.clone().unwrap_or_else(default_endpoint)
}

/// Where tool calls go.
enum Backend {
    Local {
        agent: std::sync::Arc<ph_control::Agent>,
        _studio: ph_control::Studio,
    },
    Remote(String),
}

impl Backend {
    fn open(c: &Common) -> Result<Backend, String> {
        if c.connect {
            return Ok(Backend::Remote(endpoint(c)));
        }
        let grant = parse_grant(&c.grant)?;
        let (studio, agent) = start_headless(HeadlessOptions {
            project_dir: project_dir(c),
            assets_dir: c.assets.clone(),
            audio: if c.audio {
                AudioMode::Device
            } else {
                AudioMode::Silent
            },
            projects_dir: ph_control::default_projects_dir(),
            grant,
        })?;
        Ok(Backend::Local {
            agent,
            _studio: studio,
        })
    }

    async fn call(&self, tool: &str, args: Value) -> Result<Value, String> {
        match self {
            Backend::Local { agent, .. } => {
                let (agent, tool) = (agent.clone(), tool.to_string());
                tokio::task::spawn_blocking(move || {
                    agent.call("patternhaus-cli", &tool, args).to_json()
                })
                .await
                .map_err(|e| e.to_string())
            }
            Backend::Remote(ep) => remote_call(ep, tool, args).await,
        }
    }
}

/// One JSON-RPC request to a running app.
async fn remote_call(endpoint: &str, tool: &str, args: Value) -> Result<Value, String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (r, mut w) = ph_control::connect(endpoint).await?;
    let mut r = BufReader::new(r);
    let hello = json!({ "jsonrpc": "2.0", "id": 0, "method": "rpc.hello", "params": { "client": "patternhaus-cli" } });
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": tool, "params": args });
    w.write_all(format!("{hello}\n{req}\n").as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    w.flush().await.map_err(|e| e.to_string())?;
    for _ in 0..2 {
        let mut line = String::new();
        r.read_line(&mut line).await.map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(line.trim()).map_err(|e| e.to_string())?;
        if v["id"] == json!(1) {
            return match v.get("error") {
                Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                None => Ok(v["result"].clone()),
            };
        }
    }
    Err("no response from the app".into())
}

/// Prints a tool result; returns whether it was an error. Images go to `image_out`.
fn print_result(res: &Value, image_out: Option<&PathBuf>) -> bool {
    use base64::Engine as _;
    let is_error = res["isError"].as_bool().unwrap_or(false);
    if let Some(blocks) = res["content"].as_array() {
        let mut printed = false;
        for b in blocks {
            match b["type"].as_str() {
                Some("text") => {
                    let t = b["text"].as_str().unwrap_or_default();
                    match serde_json::from_str::<Value>(t) {
                        Ok(v) => {
                            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default())
                        }
                        Err(_) => println!("{t}"),
                    }
                    printed = true;
                }
                Some("image") => {
                    let data = b["data"].as_str().unwrap_or_default();
                    match (
                        image_out,
                        base64::engine::general_purpose::STANDARD.decode(data),
                    ) {
                        (Some(p), Ok(bytes)) => match std::fs::write(p, &bytes) {
                            Ok(()) => println!("wrote {} ({} bytes)", p.display(), bytes.len()),
                            Err(e) => eprintln!("could not write {}: {e}", p.display()),
                        },
                        (None, Ok(bytes)) => {
                            println!("(image, {} bytes; use --out to save it)", bytes.len())
                        }
                        (_, Err(e)) => eprintln!("bad image data: {e}"),
                    }
                }
                _ => {}
            }
        }
        if !printed && !is_error {
            println!("ok");
        }
    } else {
        println!("{}", serde_json::to_string_pretty(res).unwrap_or_default());
    }
    is_error
}

fn script_commands(text: &str) -> Result<(Vec<Value>, bool), String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("script is not JSON: {e}"))?;
    let (list, one) = match &v {
        Value::Array(a) => (a.clone(), true),
        Value::Object(o) => (
            o.get("requests")
                .or_else(|| o.get("commands"))
                .and_then(Value::as_array)
                .cloned()
                .ok_or("script object needs a `requests` array")?,
            o.get("as_one_undo")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        ),
        _ => return Err("script must be an array of requests".into()),
    };
    let cmds = list
        .into_iter()
        .map(|r| {
            let id = r["command"].as_str().or_else(|| r["id"].as_str()).ok_or("each request needs a `command`")?;
            Ok(json!({ "id": id, "params": r.get("params").cloned().unwrap_or(json!({})), "confirm": r["confirm"].as_bool().unwrap_or(false) }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((cmds, one))
}

async fn run(cmd: Cmd) -> Result<bool, String> {
    match cmd {
        Cmd::Run {
            command,
            params,
            confirm,
            common,
        } => {
            let params: Value =
                serde_json::from_str(&params).map_err(|e| format!("--params is not JSON: {e}"))?;
            let b = Backend::open(&common)?;
            let r = b
                .call(
                    "run",
                    json!({ "id": command, "params": params, "confirm": confirm }),
                )
                .await?;
            Ok(print_result(&r, None))
        }
        Cmd::Script {
            file,
            separate,
            common,
        } => {
            let text =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let (cmds, one) = script_commands(&text)?;
            let b = Backend::open(&common)?;
            let r = b
                .call(
                    "run_batch",
                    json!({ "commands": cmds, "as_one_undo": one && !separate }),
                )
                .await?;
            Ok(print_result(&r, None))
        }
        Cmd::Render {
            name,
            format,
            bit_depth,
            from_bar,
            to_bar,
            common,
        } => {
            let b = Backend::open(&common)?;
            let mut args =
                json!({ "name": name, "format": format, "range": range(from_bar, to_bar) });
            if let Some(d) = bit_depth {
                args["bit_depth"] = json!(d);
            }
            let r = b.call("render", args).await?;
            Ok(print_result(&r, None))
        }
        Cmd::Analyze {
            what,
            from_bar,
            to_bar,
            common,
        } => {
            let b = Backend::open(&common)?;
            let what: Vec<&str> = what
                .as_deref()
                .map(|w| w.split(',').collect())
                .unwrap_or_default();
            let r = b
                .call(
                    "analyze",
                    json!({ "what": what, "range": range(from_bar, to_bar) }),
                )
                .await?;
            Ok(print_result(&r, None))
        }
        Cmd::Snapshot {
            kind,
            out,
            pattern,
            from_bar,
            to_bar,
            common,
        } => {
            let b = Backend::open(&common)?;
            let mut args = json!({ "kind": kind, "range": range(from_bar, to_bar) });
            if let Some(p) = pattern {
                args["pattern"] = json!(p);
            }
            let r = b.call("snapshot", args).await?;
            Ok(print_result(&r, Some(&out)))
        }
        Cmd::Commands { filter, common } => {
            let b = Backend::open(&common)?;
            let r = b.call("list_commands", json!({ "filter": filter })).await?;
            if let Some(lines) = r["value"]["commands"].as_array() {
                for l in lines {
                    println!("{}", l.as_str().unwrap_or_default());
                }
                Ok(false)
            } else {
                Ok(print_result(&r, None))
            }
        }
        Cmd::Mcp { headless, common } => {
            let ep = endpoint(&common);
            let attach = if common.connect {
                true
            } else if headless {
                false
            } else {
                ph_control::connect(&ep).await.is_ok()
            };
            if attach {
                proxy_stdio(&ep).await?;
            } else {
                let grant = parse_grant(&common.grant)?;
                let (studio, agent) = start_headless(HeadlessOptions {
                    project_dir: project_dir(&common),
                    assets_dir: common.assets.clone(),
                    audio: if common.audio {
                        AudioMode::Device
                    } else {
                        AudioMode::Silent
                    },
                    projects_dir: ph_control::default_projects_dir(),
                    grant,
                })?;
                eprintln!(
                    "patternhaus: headless MCP on stdio, project {}",
                    project_dir(&common).display()
                );
                serve_stdio(agent).await?;
                drop(studio);
            }
            Ok(false)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    match rt.block_on(run(cli.cmd)) {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
