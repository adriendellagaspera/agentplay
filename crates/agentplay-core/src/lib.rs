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
pub struct ActionPlan {
    pub actions: Vec<Action>,
    pub inter_action_delay_millis: u64,
}

impl ActionPlan {
    pub fn new(actions: Vec<Action>) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !actions.is_empty(),
            "action plan must contain at least one action"
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

    pub fn with_inter_action_delay(mut self, duration: Duration) -> Self {
        self.inter_action_delay_millis = duration.as_millis() as u64;
        self
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.actions.is_empty(),
            "action plan must contain at least one action"
        );
        Ok(())
    }
}

impl From<Action> for ActionPlan {
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
    async fn step(&mut self, plan: ActionPlan) -> anyhow::Result<Step>;
}

#[async_trait]
pub trait Agent: Send {
    async fn act(&mut self, observation: &Observation) -> anyhow::Result<ActionPlan>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_action_is_a_one_action_plan() {
        let plan = ActionPlan::single(Action::KeyPress(Key::Right));
        assert_eq!(plan.actions, vec![Action::KeyPress(Key::Right)]);
        assert_eq!(plan.inter_action_delay_millis, 0);
    }

    #[test]
    fn empty_action_plan_is_rejected() {
        assert!(ActionPlan::new(Vec::new()).is_err());
    }

    #[test]
    fn action_converts_to_single_action_plan() {
        let plan: ActionPlan = Action::Wait.into();
        assert_eq!(plan, ActionPlan::single(Action::Wait));
    }

    #[test]
    fn pacing_is_explicit_and_separate_from_settle() {
        let plan = ActionPlan::new(vec![
            Action::KeyPress(Key::Right),
            Action::KeyPress(Key::Right),
        ])
        .unwrap()
        .with_inter_action_delay(Duration::from_millis(25));

        assert_eq!(plan.inter_action_delay_millis, 25);
    }
}
