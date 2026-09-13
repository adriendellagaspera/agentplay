use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub frame: Frame,
    pub sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Space,
    Character(char),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    KeyPress(Key),
    Wait,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TemporalPolicy {
    FixedDelay { millis: u64 },
    UntilQuiescent(QuiescencePolicy),
    FrozenStep { millis: u64 },
    Manual,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuiescencePolicy {
    pub sample_every_millis: u64,
    pub stable_for_millis: u64,
    pub max_wait_millis: u64,
    pub difference_threshold: u32,
}

impl Default for QuiescencePolicy {
    fn default() -> Self {
        Self {
            sample_every_millis: 50,
            stable_for_millis: 200,
            max_wait_millis: 2_000,
            difference_threshold: 0,
        }
    }
}

impl TemporalPolicy {
    pub fn fixed_delay(duration: Duration) -> Self {
        Self::FixedDelay {
            millis: duration.as_millis() as u64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub observation: Observation,
    pub settle_millis: u64,
}

#[async_trait]
pub trait Environment: Send {
    async fn observe(&mut self) -> anyhow::Result<Observation>;
    async fn step(&mut self, action: Action) -> anyhow::Result<Step>;
}

#[async_trait]
pub trait Agent: Send {
    async fn act(&mut self, observation: &Observation) -> anyhow::Result<Action>;
}
