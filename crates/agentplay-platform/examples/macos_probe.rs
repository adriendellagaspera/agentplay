#[cfg(target_os = "macos")]
use agentplay_core::Key;
#[cfg(target_os = "macos")]
use agentplay_platform::{CaptureBackend, InputBackend};
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos_probe is only available on macOS");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use agentplay_platform::macos::{InputPolicy, MacOsBackend, list_windows};

    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [command] if command == "list" => {
            for window in list_windows()? {
                println!(
                    "window={} pid={} bundle={} app={:?} title={:?}",
                    window.window_id,
                    window.pid,
                    window.bundle_identifier,
                    window.application_name,
                    window.title
                );
            }
        }
        [command, window_id, pid, bundle] if command == "capture" => {
            let selector = exact_selector(window_id, pid, bundle)?;
            let mut backend = MacOsBackend::attach(&selector, InputPolicy::capture_only())?;
            let frame = backend.capture()?;
            std::fs::write("frame.rgba", &frame.rgba)?;
            println!(
                "captured {}x{} RGBA frame to frame.rgba",
                frame.width, frame.height
            );
        }
        [command, window_id, pid, bundle, key] if command == "press" => {
            let key = parse_key(key)?;
            let selector = exact_selector(window_id, pid, bundle)?;
            let policy = InputPolicy::new(vec![key.clone()], Duration::from_millis(20))?;
            let mut backend = MacOsBackend::attach(&selector, policy)?;
            backend.press(&key)?;
            println!("sent {key:?} to validated target");
        }
        _ => anyhow::bail!(
            "usage:\n  macos_probe list\n  macos_probe capture <window-id> <pid> <bundle-id>\n  macos_probe press <window-id> <pid> <bundle-id> <key>"
        ),
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn exact_selector(
    window_id: &str,
    pid: &str,
    bundle: &str,
) -> anyhow::Result<agentplay_platform::macos::WindowSelector> {
    let window_id: u32 = window_id.parse()?;
    let pid: i32 = pid.parse()?;
    let matched = agentplay_platform::macos::list_windows()?
        .into_iter()
        .find(|window| {
            window.window_id == window_id && window.pid == pid && window.bundle_identifier == bundle
        })
        .ok_or_else(|| anyhow::anyhow!("exact target window was not found"))?;
    Ok(agentplay_platform::macos::WindowSelector {
        pid: Some(matched.pid),
        bundle_identifier: Some(matched.bundle_identifier),
        title_contains: matched.title,
    })
}

#[cfg(target_os = "macos")]
fn parse_key(value: &str) -> anyhow::Result<Key> {
    Ok(match value.to_ascii_lowercase().as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "enter" => Key::Enter,
        "space" => Key::Space,
        value if value.chars().count() == 1 => Key::Character(value.chars().next().unwrap()),
        _ => anyhow::bail!("key must be a direction, enter, space, or one character"),
    })
}
