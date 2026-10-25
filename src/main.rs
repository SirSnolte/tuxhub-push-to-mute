use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Write};
use std::time::Duration;
use tuxhub_push_to_mute::config::{Mode, parse_conditions, parse_key};
use tuxhub_push_to_mute::{client, daemon, integration, window};

/// Mutes voice chat applications such as Discord, TeamSpeak or Mumble while
/// an in-game push-to-talk key is held. Without a command, the window opens.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the background service in the foreground
    Daemon,
    /// Show the current state
    Status,
    /// List the applications that are recording from a microphone
    Streams,
    /// Wait for a key press and add that key as push-to-talk key
    Learn,
    /// Replace the push-to-talk keys, for example BTN_EXTRA or KEY_V
    Keys {
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Replace the applications to mute; each term is matched against the
    /// application name, the process name and the Flatpak app id
    Apps {
        #[arg(required = true)]
        apps: Vec<String>,
    },
    /// Mute only while one of these applications runs or is in the
    /// foreground: `cs2` or `running:cs2` while it runs, `focused:cs2` while
    /// it is in the foreground. Without arguments, muting works everywhere
    Only { apps: Vec<String> },
    /// List the open windows, as candidates for `only focused:...`
    Windows,
    /// Mute while the key is held, or switch with each press
    Mode {
        #[arg(value_enum)]
        mode: Mode,
    },
    /// Resume muting
    Enable,
    /// Pause muting
    Disable,
    /// Install for the current user from the extracted release folder
    Install {
        /// Install even if tuxhub already manages an installation
        #[arg(long)]
        force: bool,
    },
    /// Remove the installation, the launcher and the login entry
    Uninstall {
        /// Also remove the settings
        #[arg(long)]
        purge: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        None => window::run(),
        Some(cmd) => run(cmd),
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run(cmd: Command) -> io::Result<()> {
    match cmd {
        Command::Daemon => {
            if !daemon::run()? {
                eprintln!("The service is already running.");
            }
            Ok(())
        }
        Command::Status => print_status(&client::request("status", Value::Null)?),
        Command::Streams => {
            let list = client::request("streams", Value::Null)?;
            let list = list.as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!("No application is recording from a microphone.");
            }
            for s in list {
                let text = |k: &str| {
                    s.get(k)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                let mark = if s.get("matched") == Some(&json!(true)) {
                    "muted on key"
                } else {
                    ""
                };
                println!(
                    "{:<28} {:<28} {:<28} {mark}",
                    text("name"),
                    text("binary"),
                    text("app_id")
                );
            }
            Ok(())
        }
        Command::Learn => learn(),
        Command::Keys { keys } => {
            let keys: Vec<String> = keys.iter().map(|k| k.trim().to_uppercase()).collect();
            if let Some(bad) = keys.iter().find(|k| parse_key(k).is_none()) {
                return Err(io::Error::other(format!(
                    "unknown or unsupported key {bad:?}"
                )));
            }
            set(json!({ "keys": keys }))
        }
        Command::Apps { apps } => set(json!({ "apps": apps })),
        Command::Only { apps } => {
            let conditions = parse_conditions(&apps).map_err(io::Error::other)?;
            set(json!({ "only_when": conditions }))
        }
        Command::Windows => {
            let status = client::request("status", Value::Null)?;
            let list = client::request("windows", Value::Null)?;
            let list = list.as_array().cloned().unwrap_or_default();
            if status["focus_available"] != json!(true) {
                println!("Open windows are only detected on KDE Plasma.");
            } else if list.is_empty() {
                println!("No window is open.");
            }
            for w in list {
                let text = |k: &str| w[k].as_str().unwrap_or_default().to_string();
                let marks: Vec<&str> = [("active", "in foreground"), ("matched", "selected")]
                    .into_iter()
                    .filter(|(k, _)| w[*k] == json!(true))
                    .map(|(_, mark)| mark)
                    .collect();
                println!(
                    "{:<28} {:<36} {}",
                    text("term"),
                    text("class"),
                    marks.join(", ")
                );
            }
            Ok(())
        }
        Command::Mode { mode } => set(json!({ "mode": mode })),
        Command::Enable => set(json!({ "enabled": true })),
        Command::Disable => set(json!({ "enabled": false })),
        Command::Install { force } => {
            let dir = integration::install(force)?;
            println!("Installed to {}.", dir.display());
            println!("Push-to-Mute is in the application menu and starts at login.");
            println!("tuxhub detects this installation automatically.");
            Ok(())
        }
        Command::Uninstall { purge } => {
            for path in integration::uninstall(purge)? {
                println!("Removed {}", path.display());
            }
            Ok(())
        }
    }
}

fn set(patch: Value) -> io::Result<()> {
    client::request("config.set", patch)?;
    print_status(&client::request("status", Value::Null)?)
}

fn print_status(s: &Value) -> io::Result<()> {
    let cfg = client::request("config.get", Value::Null)?;
    let list = |v: &Value| {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    };
    println!(
        "State:     {}",
        s.get("text").and_then(Value::as_str).unwrap_or("unknown")
    );
    println!(
        "Keys:      {}",
        Some(list(&cfg["keys"]))
            .filter(|s| !s.is_empty())
            .unwrap_or("none".into())
    );
    println!("Mode:      {}", cfg["mode"].as_str().unwrap_or_default());
    println!("Apps:      {}", list(&cfg["apps"]));
    let met = &s["met"].as_array().cloned().unwrap_or_default();
    let only: Vec<String> = cfg["only_when"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let mark = if met.contains(&c["app"]) { ", met" } else { "" };
            format!(
                "{} ({}{mark})",
                c["app"].as_str().unwrap_or_default(),
                c["when"].as_str().unwrap_or_default()
            )
        })
        .collect();
    if only.is_empty() {
        println!("Only when: always");
    } else {
        println!("Only when: {}", only.join(", "));
        println!(
            "Windows:   {}",
            if s["focus_available"] == json!(true) {
                "detected through KWin"
            } else {
                "not detected; focused entries count while the application runs"
            }
        );
    }
    println!("Devices:   {}", s["devices"]);
    Ok(())
}

fn learn() -> io::Result<()> {
    let stream = client::connect_or_start()?;
    client::request_on(&stream, "learn.start", Value::Null)?;
    eprintln!("Press the push-to-talk key (Esc cancels) ...");
    stream.set_read_timeout(Some(Duration::from_secs(20)))?;
    for line in BufReader::new(&stream).lines() {
        let msg: Value = serde_json::from_str(&line?).map_err(io::Error::other)?;
        if msg.get("method").and_then(Value::as_str) != Some("learned") {
            continue;
        }
        let Some(key) = msg["params"]["key"].as_str() else {
            eprintln!("Cancelled.");
            return Ok(());
        };
        let cfg = client::request_on(&stream, "config.get", Value::Null)?;
        let mut keys: Vec<Value> = cfg["keys"].as_array().cloned().unwrap_or_default();
        if !keys.contains(&json!(key)) {
            keys.push(json!(key));
        }
        client::request_on(&stream, "config.set", json!({ "keys": keys }))?;
        let mut out = io::stdout();
        writeln!(out, "Added {key}.")?;
        return Ok(());
    }
    Err(io::Error::other("no key was pressed"))
}
