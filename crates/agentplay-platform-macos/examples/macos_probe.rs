#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos_probe is only available on macOS");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use agentplay_core::Key;
    use agentplay_platform::{CaptureBackend, InputBackend};
    use agentplay_platform_macos::{InputPolicy, MacOsBackend, WindowSelector, list_windows};
    use std::time::{Duration, Instant};

    fn exact_selector(window_id: &str, pid: &str, bundle: &str) -> anyhow::Result<WindowSelector> {
        Ok(WindowSelector {
            window_id: Some(window_id.parse()?),
            pid: Some(pid.parse()?),
            bundle_identifier: Some(bundle.to_owned()),
            title_contains: None,
        })
    }

    fn parse_key(value: &str) -> anyhow::Result<Key> {
        Ok(match value.to_ascii_lowercase().as_str() {
            "up" => Key::Up,
            "down" => Key::Down,
            "left" => Key::Left,
            "right" => Key::Right,
            "enter" => Key::Enter,
            "space" => Key::Space,
            value if value.chars().count() == 1 => {
                Key::Character(value.chars().next().expect("one-character key"))
            }
            _ => anyhow::bail!("key must be a direction, enter, space, or one character"),
        })
    }

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
        [command, window_id, pid, bundle, count] if command == "burst" => {
            let count: usize = count.parse()?;
            anyhow::ensure!(count > 0, "capture count must be positive");
            let selector = exact_selector(window_id, pid, bundle)?;
            let mut backend = MacOsBackend::attach(&selector, InputPolicy::capture_only())?;
            let started = Instant::now();
            let mut last = None;
            for _ in 0..count {
                last = Some(backend.capture()?);
            }
            let elapsed = started.elapsed();
            let frame = last.expect("positive capture count");
            println!(
                "captured {count} frames at {}x{} in {} ms ({:.1} ms/frame)",
                frame.width,
                frame.height,
                elapsed.as_millis(),
                elapsed.as_secs_f64() * 1000.0 / count as f64
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
            "usage:\n  macos_probe list\n  macos_probe capture <window-id> <pid> <bundle-id>\n  macos_probe burst <window-id> <pid> <bundle-id> <count>\n  macos_probe press <window-id> <pid> <bundle-id> <key>"
        ),
    }
    Ok(())
}
