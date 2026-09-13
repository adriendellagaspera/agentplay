use agentplay_core::{Frame, Key};

pub trait CaptureBackend: Send {
    fn capture(&mut self) -> anyhow::Result<Frame>;
}

pub trait InputBackend: Send {
    fn press(&mut self, key: &Key) -> anyhow::Result<()>;
}

#[cfg(target_os = "macos")]
pub mod macos;
