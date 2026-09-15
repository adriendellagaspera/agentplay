#[cfg(any(target_os = "macos", test))]
use agentplay_core::Action;
use agentplay_core::Key;
#[cfg(target_os = "macos")]
use anyhow::{Context, ensure};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "agentplay",
    version,
    about = "Agents as players for non-realtime games"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the current bootstrap status.
    Doctor,
    /// Play an attached native application through the AgentPlay runtime.
    Human(HumanArgs),
}

#[derive(Args)]
struct HumanArgs {
    /// Exact native window id. If no target selector is supplied, AgentPlay opens a window picker.
    #[arg(long)]
    window_id: Option<u32>,

    /// PID that must own the target window.
    #[arg(long)]
    pid: Option<i32>,

    /// Bundle identifier that must own the target window.
    #[arg(long = "bundle-id")]
    bundle_identifier: Option<String>,

    /// Substring that must match the target window title.
    #[arg(long)]
    title: Option<String>,

    /// Physical keys the runtime is allowed to send, comma-separated.
    #[arg(long = "allow", value_delimiter = ',', value_parser = parse_key)]
    allowed_keys: Vec<Key>,

    /// Delay between physical inputs inside one Decision.
    #[arg(long, default_value_t = 0)]
    inter_action_ms: u64,

    /// Directory in which to record the model-visible trajectory.
    #[arg(long)]
    record_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .ok();

    match Cli::parse().command {
        Command::Doctor => {
            println!("agentplay: core runtime installed");
        }
        Command::Human(args) => run_human(args).await?,
    }

    Ok(())
}

fn parse_key(value: &str) -> Result<Key, String> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "up" => Ok(Key::Up),
        "down" => Ok(Key::Down),
        "left" => Ok(Key::Left),
        "right" => Ok(Key::Right),
        "enter" | "return" => Ok(Key::Enter),
        "space" => Ok(Key::Space),
        _ => {
            let mut chars = normalized.chars();
            match (chars.next(), chars.next()) {
                (Some(character), None) => Ok(Key::Character(character)),
                _ => Err(format!("unsupported physical key {value:?}")),
            }
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn parse_actions(line: &str) -> anyhow::Result<Vec<Action>> {
    let mut actions = Vec::new();
    for token in line.split_whitespace() {
        let (key_token, count) = match token.rsplit_once('*') {
            Some((key, count)) => {
                let count = count
                    .parse::<usize>()
                    .map_err(|_| anyhow::anyhow!("invalid repetition count in {token:?}"))?;
                anyhow::ensure!(count > 0, "repetition count must be greater than zero");
                (key, count)
            }
            None => (token, 1),
        };
        let key = parse_key(key_token).map_err(anyhow::Error::msg)?;
        actions.extend(std::iter::repeat_n(Action::KeyPress(key), count));
    }
    anyhow::ensure!(
        !actions.is_empty(),
        "Decision must contain at least one Action"
    );
    Ok(actions)
}

#[cfg(not(target_os = "macos"))]
async fn run_human(_args: HumanArgs) -> anyhow::Result<()> {
    anyhow::bail!("the native human-through-runtime path is currently implemented only on macOS")
}

#[cfg(target_os = "macos")]
async fn run_human(args: HumanArgs) -> anyhow::Result<()> {
    use agentplay_core::{Decision, QuiescencePolicy};
    use agentplay_platform_macos::{
        CaptureBackend, InputBackend, InputPolicy, MacOsBackend, WindowInfo, WindowSelector,
        list_windows,
    };
    use agentplay_runner::{
        quiescence::{BlockDifferenceMetric, QuiescenceDiagnostics, settle_until_quiescent},
        step_decision,
    };
    use dialoguer::Select;
    use serde::Serialize;
    use std::fs::{self, File};
    use std::io::{self, BufWriter, Write};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    ensure!(
        !args.allowed_keys.is_empty(),
        "at least one --allow key is required"
    );

    let selector = if args.window_id.is_some()
        || args.pid.is_some()
        || args.bundle_identifier.is_some()
        || args.title.is_some()
    {
        WindowSelector {
            window_id: args.window_id,
            pid: args.pid,
            bundle_identifier: args.bundle_identifier.clone(),
            title_contains: args.title.clone(),
        }
    } else {
        let window = choose_window()?;
        WindowSelector {
            window_id: Some(window.window_id),
            pid: Some(window.pid),
            bundle_identifier: Some(window.bundle_identifier),
            title_contains: None,
        }
    };

    let input_policy = InputPolicy::new(args.allowed_keys.clone(), Duration::from_millis(20))?;
    let mut backend = MacOsBackend::attach(&selector, input_policy)?;
    let target = backend.target()?;
    let settle_policy = QuiescencePolicy::default();
    let record_dir = args.record_dir.unwrap_or_else(default_record_dir);
    fs::create_dir_all(&record_dir)
        .with_context(|| format!("creating recording directory {}", record_dir.display()))?;

    #[derive(Serialize)]
    struct Manifest<'a> {
        window_id_selector: Option<u32>,
        pid_selector: Option<i32>,
        bundle_identifier_selector: Option<&'a str>,
        title_selector: Option<&'a str>,
        window_id: u32,
        pid: i32,
        bundle_identifier: &'a str,
        allowed_keys: &'a [Key],
        quiescence: &'a QuiescencePolicy,
    }

    write_json(
        record_dir.join("manifest.json"),
        &Manifest {
            window_id_selector: selector.window_id,
            pid_selector: selector.pid,
            bundle_identifier_selector: selector.bundle_identifier.as_deref(),
            title_selector: selector.title_contains.as_deref(),
            window_id: target.window_id,
            pid: target.pid,
            bundle_identifier: &target.bundle_identifier,
            allowed_keys: &args.allowed_keys,
            quiescence: &settle_policy,
        },
    )?;

    println!(
        "attached to {:?} (pid {}, window {}); recording to {}",
        target.title,
        target.pid,
        target.window_id,
        record_dir.display()
    );
    println!(
        "one line = one Decision; examples: `right`, `right right up`, `space*10`; `quit` exits"
    );

    let initial = settle_until_quiescent(
        settle_policy.clone(),
        BlockDifferenceMetric::default(),
        || backend.capture(),
    )
    .await?;
    record_observation(&record_dir, 0, &initial.frame, None, &initial.diagnostics)?;

    let stdin = io::stdin();
    let mut line = String::new();
    let mut sequence = 0_u64;

    loop {
        print!("> ");
        io::stdout().flush()?;
        line.clear();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if matches!(line, "quit" | "exit") {
            break;
        }

        let actions = parse_actions(line)?;
        let decision = Decision::new(actions)?
            .with_inter_action_delay(Duration::from_millis(args.inter_action_ms));
        let result = step_decision(
            &mut backend,
            &decision,
            |backend, action| match action {
                Action::KeyPress(key) => backend.press(key),
            },
            |backend| backend.capture(),
            settle_policy.clone(),
            BlockDifferenceMetric::default(),
        )
        .await?;

        sequence += 1;
        record_observation(
            &record_dir,
            sequence,
            &result.frame,
            Some(&decision),
            &result.diagnostics,
        )?;
        println!(
            "observation {sequence}: {:?} after {} ms / {} samples",
            result.diagnostics.reason,
            result.diagnostics.elapsed_millis,
            result.diagnostics.samples
        );
    }

    fn choose_window() -> anyhow::Result<WindowInfo> {
        let mut windows = list_windows()?;
        ensure!(!windows.is_empty(), "no on-screen windows are available");
        windows.sort_by(|left, right| {
            left.application_name
                .cmp(&right.application_name)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.window_id.cmp(&right.window_id))
        });

        let labels = windows.iter().map(window_label).collect::<Vec<_>>();
        let selection = Select::new()
            .with_prompt("Select target window (↑/↓, Enter; Esc/q to cancel)")
            .items(&labels)
            .default(0)
            .max_length(15)
            .interact_opt()
            .context("selecting target window")?
            .ok_or_else(|| anyhow::anyhow!("window selection cancelled"))?;
        Ok(windows.remove(selection))
    }

    fn window_label(window: &WindowInfo) -> String {
        let title = window.title.as_deref().unwrap_or("<untitled>");
        format!(
            "{} — {}  [pid {}, window {}, {}]",
            window.application_name,
            title,
            window.pid,
            window.window_id,
            window.bundle_identifier
        )
    }

    fn default_record_dir() -> PathBuf {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        PathBuf::from("agentplay-runs").join(millis.to_string())
    }

    #[derive(Serialize)]
    struct ObservationRecord<'a> {
        sequence: u64,
        timestamp_unix_millis: u64,
        decision: Option<&'a Decision>,
        settle: &'a QuiescenceDiagnostics,
        frame: String,
    }

    fn record_observation(
        directory: &std::path::Path,
        sequence: u64,
        frame: &agentplay_core::Frame,
        decision: Option<&Decision>,
        settle: &QuiescenceDiagnostics,
    ) -> anyhow::Result<()> {
        let stem = format!("{sequence:06}");
        let frame_name = format!("{stem}.png");
        write_png(directory.join(&frame_name), frame)?;
        let timestamp_unix_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        write_json(
            directory.join(format!("{stem}.json")),
            &ObservationRecord {
                sequence,
                timestamp_unix_millis,
                decision,
                settle,
                frame: frame_name,
            },
        )
    }

    fn write_png(path: PathBuf, frame: &agentplay_core::Frame) -> anyhow::Result<()> {
        let file =
            File::create(&path).with_context(|| format!("creating frame {}", path.display()))?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), frame.width, frame.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().context("writing PNG header")?;
        writer
            .write_image_data(&frame.rgba)
            .with_context(|| format!("writing frame {}", path.display()))?;
        Ok(())
    }

    fn write_json(path: PathBuf, value: &impl Serialize) -> anyhow::Result<()> {
        let file =
            File::create(&path).with_context(|| format!("creating metadata {}", path.display()))?;
        serde_json::to_writer_pretty(BufWriter::new(file), value)
            .with_context(|| format!("writing metadata {}", path.display()))
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_line_as_one_ordered_action_batch() {
        assert_eq!(
            parse_actions("right right up").unwrap(),
            vec![
                Action::KeyPress(Key::Right),
                Action::KeyPress(Key::Right),
                Action::KeyPress(Key::Up),
            ]
        );
    }

    #[test]
    fn expands_repeated_physical_input() {
        assert_eq!(
            parse_actions("space*3").unwrap(),
            vec![Action::KeyPress(Key::Space); 3]
        );
    }

    #[test]
    fn rejects_zero_repetition() {
        assert!(parse_actions("space*0").is_err());
    }
}
