use crate::{CaptureBackend, InputBackend};
use agentplay_core::{Frame, Key};
use anyhow::{Context, Result, bail, ensure};
use png::{BitDepth, ColorType, Decoder, Transformations};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowSelector {
    pub window_id: Option<u32>,
    pub pid: Option<i32>,
    pub bundle_identifier: Option<String>,
    pub title_contains: Option<String>,
}

impl WindowSelector {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.window_id.is_some()
                || self.pid.is_some()
                || self.bundle_identifier.is_some()
                || self.title_contains.is_some(),
            "window selector must specify at least one constraint"
        );
        if let Some(bundle) = &self.bundle_identifier {
            ensure!(!bundle.is_empty(), "bundle identifier must not be empty");
        }
        if let Some(title) = &self.title_contains {
            ensure!(!title.is_empty(), "title selector must not be empty");
        }
        Ok(())
    }

    fn matches(&self, window: &WindowInfo) -> bool {
        self.window_id
            .is_none_or(|window_id| window.window_id == window_id)
            && self.pid.is_none_or(|pid| window.pid == pid)
            && self
                .bundle_identifier
                .as_deref()
                .is_none_or(|bundle| window.bundle_identifier == bundle)
            && self.title_contains.as_deref().is_none_or(|needle| {
                window
                    .title
                    .as_deref()
                    .is_some_and(|title| title.contains(needle))
            })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct WindowInfo {
    pub window_id: u32,
    pub pid: i32,
    pub application_name: String,
    pub bundle_identifier: String,
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TargetIdentity {
    window_id: u32,
    pid: i32,
    bundle_identifier: String,
}

#[derive(Clone, Debug)]
pub struct InputPolicy {
    allowed_keys: Vec<Key>,
    key_hold: Duration,
}

impl InputPolicy {
    pub fn new(allowed_keys: Vec<Key>, key_hold: Duration) -> Result<Self> {
        ensure!(
            key_hold <= Duration::from_secs(1),
            "key hold duration must be at most one second"
        );
        Ok(Self {
            allowed_keys,
            key_hold,
        })
    }

    pub fn capture_only() -> Self {
        Self {
            allowed_keys: Vec::new(),
            key_hold: Duration::from_millis(20),
        }
    }

    pub fn allows(&self, key: &Key) -> bool {
        self.allowed_keys.contains(key)
    }
}

struct BridgeSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl BridgeSession {
    fn start(target: &TargetIdentity) -> Result<Self> {
        let mut child = Command::new(env!("AGENTPLAY_MACOS_BRIDGE"))
            .args([
                "session",
                &target.window_id.to_string(),
                &target.pid.to_string(),
                &target.bundle_identifier,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("starting AgentPlay macOS native session")?;
        let stdin = child
            .stdin
            .take()
            .context("opening macOS native session stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("opening macOS native session stdout")?;
        let mut session = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        };
        let ready = session.read_line()?;
        ensure!(
            ready == "READY",
            "unexpected macOS native session handshake {ready:?}"
        );
        Ok(session)
    }

    fn capture(&mut self) -> Result<Frame> {
        let response = self.request("capture")?;
        if let Some(message) = response.strip_prefix("ERR ") {
            bail!("macOS native session: {message}");
        }
        let length = response
            .strip_prefix("OK ")
            .context("invalid macOS capture response")?
            .parse::<usize>()
            .context("invalid macOS capture payload length")?;
        let mut png = vec![0_u8; length];
        self.stdout
            .read_exact(&mut png)
            .context("reading macOS capture payload")?;
        decode_png(&png)
    }

    fn press(&mut self, key_code: u16, hold_millis: u128) -> Result<()> {
        let response = self.request(&format!("press {key_code} {hold_millis}"))?;
        if let Some(message) = response.strip_prefix("ERR ") {
            bail!("macOS native session: {message}");
        }
        ensure!(
            response == "OK",
            "invalid macOS input response {response:?}"
        );
        Ok(())
    }

    fn request(&mut self, command: &str) -> Result<String> {
        writeln!(self.stdin, "{command}").context("writing macOS native session command")?;
        self.stdin
            .flush()
            .context("flushing macOS native session command")?;
        self.read_line()
    }

    fn read_line(&mut self) -> Result<String> {
        let mut line = String::new();
        let bytes = self
            .stdout
            .read_line(&mut line)
            .context("reading macOS native session response")?;
        ensure!(bytes > 0, "macOS native session exited unexpectedly");
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }
}

impl Drop for BridgeSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct MacOsBackend {
    target: TargetIdentity,
    input_policy: InputPolicy,
    session: BridgeSession,
}

impl MacOsBackend {
    pub fn attach(selector: &WindowSelector, input_policy: InputPolicy) -> Result<Self> {
        selector.validate()?;
        let matches = list_windows()?
            .into_iter()
            .filter(|window| selector.matches(window))
            .collect::<Vec<_>>();

        let target = match matches.as_slice() {
            [] => bail!("no on-screen window matched selector {selector:?}"),
            [window] => TargetIdentity {
                window_id: window.window_id,
                pid: window.pid,
                bundle_identifier: window.bundle_identifier.clone(),
            },
            _ => {
                let candidates = matches
                    .iter()
                    .map(|window| {
                        format!(
                            "window={} pid={} app={:?} bundle={:?} title={:?}",
                            window.window_id,
                            window.pid,
                            window.application_name,
                            window.bundle_identifier,
                            window.title
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!("window selector is ambiguous; matched: {candidates}")
            }
        };
        let session = BridgeSession::start(&target)?;

        Ok(Self {
            target,
            input_policy,
            session,
        })
    }

    pub fn target(&self) -> Result<WindowInfo> {
        list_windows()?
            .into_iter()
            .find(|window| self.target_matches(window))
            .with_context(|| {
                format!(
                    "target window {} disappeared or changed identity",
                    self.target.window_id
                )
            })
    }

    fn target_matches(&self, window: &WindowInfo) -> bool {
        window.window_id == self.target.window_id
            && window.pid == self.target.pid
            && window.bundle_identifier == self.target.bundle_identifier
    }
}

impl CaptureBackend for MacOsBackend {
    fn capture(&mut self) -> Result<Frame> {
        self.session.capture()
    }
}

impl InputBackend for MacOsBackend {
    fn press(&mut self, key: &Key) -> Result<()> {
        ensure!(
            self.input_policy.allows(key),
            "key {key:?} is not present in the input allowlist"
        );
        let key_code = key_code(key)?;
        self.session
            .press(key_code, self.input_policy.key_hold.as_millis())
    }
}

pub fn list_windows() -> Result<Vec<WindowInfo>> {
    let output = run_bridge(["list"])?;
    serde_json::from_slice(&output.stdout).context("decoding macOS window list")
}

fn run_bridge<const N: usize>(args: [&str; N]) -> Result<Output> {
    let output = Command::new(env!("AGENTPLAY_MACOS_BRIDGE"))
        .args(args)
        .output()
        .context("running AgentPlay macOS native bridge")?;
    if output.status.success() {
        return Ok(output);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    bail!(
        "macOS bridge failed with status {}: {}",
        output.status,
        if stderr.is_empty() {
            "no diagnostic output"
        } else {
            &stderr
        }
    )
}

fn decode_png(bytes: &[u8]) -> Result<Frame> {
    let mut decoder = Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .context("reading screenshot PNG header")?;
    let buffer_size = reader
        .output_buffer_size()
        .context("screenshot PNG output size is unknown")?;
    let mut buffer = vec![0; buffer_size];
    let info = reader
        .next_frame(&mut buffer)
        .context("decoding screenshot PNG")?;
    ensure!(
        info.bit_depth == BitDepth::Eight,
        "unsupported screenshot bit depth {:?}",
        info.bit_depth
    );
    let source = &buffer[..info.buffer_size()];
    let rgba = to_rgba(source, info.color_type)?;

    Ok(Frame {
        width: info.width,
        height: info.height,
        rgba,
    })
}

fn to_rgba(source: &[u8], color_type: ColorType) -> Result<Vec<u8>> {
    match color_type {
        ColorType::Rgba => Ok(source.to_vec()),
        ColorType::Rgb => Ok(source
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect()),
        ColorType::Grayscale => Ok(source
            .iter()
            .flat_map(|value| [*value, *value, *value, 255])
            .collect()),
        ColorType::GrayscaleAlpha => Ok(source
            .chunks_exact(2)
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect()),
        other => bail!("unsupported screenshot color type {other:?}"),
    }
}

fn key_code(key: &Key) -> Result<u16> {
    let code = match key {
        Key::Up => 0x7E,
        Key::Down => 0x7D,
        Key::Left => 0x7B,
        Key::Right => 0x7C,
        Key::Enter => 0x24,
        Key::Space => 0x31,
        Key::Character(character) => character_key_code(*character)?,
    };
    Ok(code)
}

fn character_key_code(character: char) -> Result<u16> {
    let code = match character.to_ascii_lowercase() {
        'a' => 0x00,
        's' => 0x01,
        'd' => 0x02,
        'f' => 0x03,
        'h' => 0x04,
        'g' => 0x05,
        'z' => 0x06,
        'x' => 0x07,
        'c' => 0x08,
        'v' => 0x09,
        'b' => 0x0B,
        'q' => 0x0C,
        'w' => 0x0D,
        'e' => 0x0E,
        'r' => 0x0F,
        'y' => 0x10,
        't' => 0x11,
        '1' => 0x12,
        '2' => 0x13,
        '3' => 0x14,
        '4' => 0x15,
        '6' => 0x16,
        '5' => 0x17,
        '=' => 0x18,
        '9' => 0x19,
        '7' => 0x1A,
        '-' => 0x1B,
        '8' => 0x1C,
        '0' => 0x1D,
        ']' => 0x1E,
        'o' => 0x1F,
        'u' => 0x20,
        '[' => 0x21,
        'i' => 0x22,
        'p' => 0x23,
        'l' => 0x25,
        'j' => 0x26,
        '\'' => 0x27,
        'k' => 0x28,
        ';' => 0x29,
        '\\' => 0x2A,
        ',' => 0x2B,
        '/' => 0x2C,
        'n' => 0x2D,
        'm' => 0x2E,
        '.' => 0x2F,
        '`' => 0x32,
        other => bail!("character {other:?} has no safe physical-key mapping yet"),
    };
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(pid: i32, bundle: &str, title: Option<&str>) -> WindowInfo {
        WindowInfo {
            window_id: 42,
            pid,
            application_name: "Game".into(),
            bundle_identifier: bundle.into(),
            title: title.map(str::to_owned),
        }
    }

    #[test]
    fn selector_requires_a_constraint() {
        assert!(WindowSelector::default().validate().is_err());
    }

    #[test]
    fn selector_matches_all_supplied_constraints() {
        let selector = WindowSelector {
            window_id: Some(42),
            pid: Some(7),
            bundle_identifier: Some("org.example.game".into()),
            title_contains: Some("Baba".into()),
        };
        assert!(selector.matches(&window(7, "org.example.game", Some("Baba Is You"))));
        assert!(!selector.matches(&window(8, "org.example.game", Some("Baba Is You"))));
    }

    #[test]
    fn selector_can_target_exact_window_id() {
        let selector = WindowSelector {
            window_id: Some(42),
            ..WindowSelector::default()
        };
        assert!(selector.matches(&window(7, "org.example.game", Some("Baba Is You"))));
        let mut other = window(7, "org.example.game", Some("Baba Is You"));
        other.window_id = 43;
        assert!(!selector.matches(&other));
    }

    #[test]
    fn policy_rejects_unlisted_keys() {
        let policy = InputPolicy::new(vec![Key::Left, Key::Character('z')], Duration::ZERO)
            .expect("valid policy");
        assert!(policy.allows(&Key::Left));
        assert!(policy.allows(&Key::Character('z')));
        assert!(!policy.allows(&Key::Right));
    }

    #[test]
    fn maps_baba_keys() {
        assert_eq!(key_code(&Key::Left).unwrap(), 0x7B);
        assert_eq!(key_code(&Key::Character('z')).unwrap(), 0x06);
        assert_eq!(key_code(&Key::Character('r')).unwrap(), 0x0F);
    }
}
