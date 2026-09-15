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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub actions: Vec<Action>,
    pub inter_action_delay_millis: u64,
}

impl Decision {
    pub fn new(actions: Vec<Action>) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !actions.is_empty(),
            "decision must contain at least one action"
        );
        Ok(Self {
            actions,
            inter_action_delay_millis: 0,
        })
    }

    pub fn single(action: Action) -> Self {
        Self {
            actions: vec![action],
            inter_action_delay_millis: 0,
        }
    }

    pub fn press(key: Key) -> Self {
        Self::single(Action::KeyPress(key))
    }

    pub fn repeat(action: Action, count: usize) -> anyhow::Result<Self> {
        anyhow::ensure!(
            count > 0,
            "action repetition count must be greater than zero"
        );
        Self::new(vec![action; count])
    }

    pub fn with_inter_action_delay(mut self, duration: Duration) -> Self {
        self.inter_action_delay_millis = duration.as_millis() as u64;
        self
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.actions.is_empty(),
            "decision must contain at least one action"
        );
        Ok(())
    }
}

impl From<Action> for Decision {
    fn from(action: Action) -> Self {
        Self::single(action)
    }
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
    pub max_cycle_frames: usize,
}

impl Default for QuiescencePolicy {
    fn default() -> Self {
        Self {
            sample_every_millis: 50,
            stable_for_millis: 200,
            max_wait_millis: 2_000,
            difference_threshold: 100,
            max_cycle_frames: 8,
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
    async fn step(&mut self, decision: Decision) -> anyhow::Result<Step>;
}

#[async_trait]
pub trait Agent: Send {
    async fn decide(&mut self, observation: &Observation) -> anyhow::Result<Decision>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_converts_to_single_action_decision() {
        let decision: Decision = Action::KeyPress(Key::Right).into();
        assert_eq!(decision.actions, vec![Action::KeyPress(Key::Right)]);
    }

    #[test]
    fn repeated_key_presses_remain_one_decision() {
        let action = Action::KeyPress(Key::Space);
        let decision = Decision::repeat(action.clone(), 10).unwrap();
        assert_eq!(decision.actions, vec![action; 10]);
    }

    #[test]
    fn empty_decision_is_rejected() {
        assert!(Decision::new(Vec::new()).is_err());
    }

    #[test]
    fn zero_repetition_is_rejected() {
        assert!(Decision::repeat(Action::KeyPress(Key::Space), 0).is_err());
    }

    #[test]
    fn pacing_is_explicit_and_separate_from_settle() {
        let decision = Decision::new(vec![
            Action::KeyPress(Key::Right),
            Action::KeyPress(Key::Right),
        ])
        .unwrap()
        .with_inter_action_delay(Duration::from_millis(25));

        assert_eq!(decision.inter_action_delay_millis, 25);
    }

    #[test]
    fn quiescence_default_keeps_multi_sample_cycle_history() {
        assert_eq!(QuiescencePolicy::default().max_cycle_frames, 8);
    }
}
