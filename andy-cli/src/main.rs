use anyhow::{Result, bail};
use argh::FromArgs;
use std::fs;
use std::path::{Path, PathBuf};

use crate::client::Client;

mod a11y;
mod assets;
mod client;
mod runner;
mod types;

/// Android coordinator CLI
#[derive(FromArgs)]
struct Cli {
    /// screen name
    #[argh(option, default = "default_screen_from_env()")]
    screen: String,
    /// bind package or prefix at screen creation, e.g. com.fedi.dev or com.fedi.dev17
    #[argh(option, default = "default_package_from_env()")]
    package: String,

    #[argh(subcommand)]
    command: Command,
}

fn socket_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(format!("{home}/.local/state/andy.sock"))
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum Command {
    Info(InfoCmd),
    Screenshot(ScreenshotCmd),
    A11y(A11yCmd),
    Tap(TapCmd),
    Swipe(SwipeCmd),
    Type(TypeCmd),
    Key(KeyCmd),
    Launch(LaunchCmd),
    Stop(StopCmd),
    Reset(ResetCmd),
    OpenUrl(OpenUrlCmd),
    WaitForIdle(WaitForIdleCmd),
    Start(StartCmd),
    Install(InstallCmd),
    Version(VersionCmd),
}

/// show screen info
#[derive(FromArgs)]
#[argh(subcommand, name = "info")]
struct InfoCmd {}

/// take a screenshot and save to path
#[derive(FromArgs)]
#[argh(subcommand, name = "screenshot")]
struct ScreenshotCmd {
    #[argh(positional)]
    path: String,
    /// skip waiting for idle before screenshot
    #[argh(switch)]
    no_wait: bool,
}

/// print human-readable accessibility tree
#[derive(FromArgs)]
#[argh(subcommand, name = "a11y")]
struct A11yCmd {
    /// skip waiting for idle before fetching tree
    #[argh(switch)]
    no_wait: bool,
}

/// tap at coordinates (x,y) or by accessibility text
#[derive(FromArgs)]
#[argh(subcommand, name = "tap")]
struct TapCmd {
    #[argh(positional)]
    target: String,
    /// skip waiting for idle after tap
    #[argh(switch)]
    no_wait: bool,
    /// retry finding the node up to N times (waiting for idle between attempts)
    #[argh(option, default = "3")]
    tries: u32,
}

/// swipe gesture
#[derive(FromArgs)]
#[argh(subcommand, name = "swipe")]
struct SwipeCmd {
    #[argh(positional)]
    x1: f32,
    #[argh(positional)]
    y1: f32,
    #[argh(positional)]
    x2: f32,
    #[argh(positional)]
    y2: f32,
    /// swipe duration in milliseconds
    #[argh(positional, default = "300")]
    duration_ms: i64,
}

/// type text
#[derive(FromArgs)]
#[argh(subcommand, name = "type")]
struct TypeCmd {
    #[argh(positional)]
    text: String,
}

/// send keycode
#[derive(FromArgs)]
#[argh(subcommand, name = "key")]
struct KeyCmd {
    #[argh(positional)]
    keycode: i32,
}

/// launch the bound package
#[derive(FromArgs)]
#[argh(subcommand, name = "launch")]
struct LaunchCmd {
    /// skip waiting for idle after launch
    #[argh(switch)]
    no_wait: bool,
}

/// stop package
#[derive(FromArgs)]
#[argh(subcommand, name = "stop")]
struct StopCmd {
}

/// clear app data (pm clear)
#[derive(FromArgs)]
#[argh(subcommand, name = "reset")]
struct ResetCmd {
}

/// open URL in package
#[derive(FromArgs)]
#[argh(subcommand, name = "open-url")]
struct OpenUrlCmd {
    #[argh(positional)]
    url: String,
}

/// wait for UI to become idle
#[derive(FromArgs)]
#[argh(subcommand, name = "wait-for-idle")]
struct WaitForIdleCmd {
    /// idle timeout in milliseconds
    #[argh(option, default = "500")]
    idle_timeout_ms: i64,
    /// global timeout in milliseconds
    #[argh(option, default = "5000")]
    global_timeout_ms: i64,
}

/// deploy and start the coordinator on device
#[derive(FromArgs)]
#[argh(subcommand, name = "start")]
struct StartCmd {}

/// install agent skill file into $PWD/.agents/skills/android-emulator/
#[derive(FromArgs)]
#[argh(subcommand, name = "install")]
struct InstallCmd {}

/// print version
#[derive(FromArgs)]
#[argh(subcommand, name = "version")]
struct VersionCmd {}

/// Check if the server is reachable; if not, auto-start it.
/// Also ensures the screen exists (saving a round-trip).
async fn ensure_server(socket: &Path, screen: &str, package: &str) -> Result<Client> {
    if socket.exists() {
        let client = Client::new(socket.to_path_buf());
        if client.ensure_screen(screen, package).await.is_ok() {
            return Ok(client);
        }
        eprintln!("debug: socket exists but server is not responding, restarting...");
    } else {
        eprintln!("debug: socket not found, starting server...");
    }

    runner::start(socket)?;

    // Daemon was spawned on device — poll until it's ready
    let client = Client::new(socket.to_path_buf());
    let mut delay_ms = 1u64;
    let mut total_ms = 0u64;
    loop {
        if client.ensure_screen(screen, package).await.is_ok() {
            eprintln!("debug: server ready after {total_ms}ms");
            return Ok(client);
        }
        if total_ms >= 30000 {
            bail!("server did not become ready after 30s");
        }
        delay_ms = (delay_ms * 2).min(1000);
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        total_ms += delay_ms;
    }
}

fn default_screen_from_env() -> String {
    std::env::var("ANDY_SCREEN").unwrap_or_else(|_| String::from("default"))
}

fn default_package_from_env() -> String {
    std::env::var("ANDY_PACKAGE").unwrap_or_default()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli: Cli = argh::from_env();

    let socket = socket_path();

    // Handle commands that don't need a client
    if let Command::Start(_) = &cli.command {
        return runner::start(&socket);
    }
    if let Command::Version(_) = &cli.command {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if let Command::Install(_) = cli.command {
        let skill_dir = PathBuf::from(".agents/skills/android-emulator");
        fs::create_dir_all(&skill_dir)?;
        fs::write(skill_dir.join("SKILL.md"), assets::SKILL_MD)?;

        let claude_skills = PathBuf::from(".claude/skills");
        fs::create_dir_all(&claude_skills)?;
        let link = claude_skills.join("android-emulator");
        if !link.exists() {
            std::os::unix::fs::symlink("../../.agents/skills/android-emulator", &link)?;
        }

        eprintln!("installed .agents/skills/android-emulator/SKILL.md");
        return Ok(());
    }

    let screen = &cli.screen;
    let package = cli.package.clone();
    if package.is_empty() {
        bail!("--package or ANDY_PACKAGE required to bind at screen creation (full or prefix)");
    }
    let client = ensure_server(&socket, screen, &package).await?;

    match cli.command {
        Command::Info(_) => {
            let info = client.info(screen).await?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        Command::Screenshot(cmd) => {
            let (data, wait_ms) = client.screenshot(screen, cmd.no_wait).await?;
            fs::write(&cmd.path, &data)?;
            if let Some(ms) = wait_ms {
                if ms > 0 {
                    eprintln!("note: waited {ms}ms for idle");
                }
            }
            eprintln!("saved screenshot to {}", cmd.path);
        }
        Command::A11y(cmd) => {
            let (tree, wait_ms) = client.a11y(screen, cmd.no_wait).await?;
            if let Some(ms) = wait_ms {
                if ms > 0 {
                    eprintln!("note: waited {ms}ms for idle");
                }
            }
            println!("{}", a11y::render_text(&tree));
        }
        Command::Tap(cmd) => {
            let wait_ms = if let Some((x_str, y_str)) = cmd.target.split_once(',') {
                let x: f32 = x_str.parse()?;
                let y: f32 = y_str.parse()?;
                client.tap(screen, x, y, cmd.no_wait).await?
            } else {
                let tries = cmd.tries.max(1);
                let mut coords = None;
                for attempt in 1..=tries {
                    let (tree, _) = client.a11y(screen, true).await?;
                    if let Some(node) = a11y::find_node(&tree, &cmd.target) {
                        let x = (node.bounds.left + node.bounds.right) as f32 / 2.0;
                        let y = (node.bounds.top + node.bounds.bottom) as f32 / 2.0;
                        coords = Some((x, y));
                        break;
                    }
                    if attempt < tries {
                        eprintln!("note: node \"{}\" not found, retrying ({}/{})", cmd.target, attempt, tries);
                        client.wait_for_idle(screen, 500, 5000).await?;
                    }
                }
                let (x, y) = coords
                    .ok_or_else(|| anyhow::anyhow!("node not found: \"{}\"", cmd.target))?;
                client.tap(screen, x, y, cmd.no_wait).await?
            };
            if let Some(ms) = wait_ms {
                if ms > 0 {
                    eprintln!("note: waited {ms}ms for idle");
                }
            }
        }
        Command::Swipe(cmd) => {
            client
                .swipe(screen, cmd.x1, cmd.y1, cmd.x2, cmd.y2, cmd.duration_ms)
                .await?;
        }
        Command::Type(cmd) => {
            client.type_text(screen, &cmd.text).await?;
        }
        Command::Key(cmd) => {
            client.key(screen, cmd.keycode).await?;
        }
        Command::Launch(cmd) => {
            let wait_ms = client.launch(screen, cmd.no_wait).await?;
            if let Some(ms) = wait_ms {
                if ms > 0 {
                    eprintln!("note: waited {ms}ms for idle");
                }
            }
        }
        Command::Stop(_) => {
            client.stop(screen).await?;
        }
        Command::Reset(_) => {
            client.reset(screen).await?;
        }
        Command::OpenUrl(cmd) => {
            client.open_url(screen, &cmd.url).await?;
        }
        Command::WaitForIdle(cmd) => {
            client
                .wait_for_idle(screen, cmd.idle_timeout_ms, cmd.global_timeout_ms)
                .await?;
        }
        Command::Start(_) | Command::Install(_) | Command::Version(_) => unreachable!(),
    }

    Ok(())
}
