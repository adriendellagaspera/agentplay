#![cfg(target_os = "macos")]

pub use agentplay_platform::{CaptureBackend, InputBackend};

mod backend;

pub use backend::*;
