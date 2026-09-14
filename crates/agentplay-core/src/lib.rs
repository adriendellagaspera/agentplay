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
pub enum PlayerAction {
    KeyPress(Key),
    Wait,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionPlan {
    pub actions: Vec<PlayerAction>,
    pub inter_action_delay_millis: u64,
}

impl ActionPlan {
    pub fn new(actions: Vec<PlayerAction>) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !actions.is_empty(),
            "action plan must contain at least one player action"
        );
        Ok(Self {
            actions,
            inter_action_delay_millis: 0,
        })
    }

    pub fn with_inter_action_delay(mut self, duration: Duration) -> Self {
        self.inter_action_delay_millis = duration.as_millis() as u64;
        self
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.actions.is_empty(),
            "action plan must contain at least one player action"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Plan(ActionPlan),
}

impl Action {
    pub fn plan(actions: Vec<PlayerAction>) -> anyhow::Result<Self> {
        Ok(Self::Plan(ActionPlan::new(actions)?))
    }

    pub fn single(action: PlayerAction) -> Self {
        Self::Plan(ActionPlan {
            actions: vec![action],
            inter_action_delay_millis: 0,
        })
    }

    pub fn press(key: Key) -> Self {
        Self::single(PlayerAction::KeyPress(key))
    }

    pub fn wait() -> Self {
        Self::single(PlayerAction::Wait)
    }

    pub fn repeat(action: PlayerAction, count: usize) -> anyhow::Result<Self> {
        anyhow::ensure!(count > 0, "action repetition count must be greater than zero");
        Self::plan(vec![action; count])
    }

    pub fn with_inter_action_delay(self, duration: Duration) -> Self {
        match self {
            Self::Plan(plan) => Self::Plan(plan.with_inter_action_delay(duration)),
        }
    }

    pub fn plan_ref(&self) -> &ActionPlan {
        match self {
            Self::Plan(plan) => plan,
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.plan_ref().validate()
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
    async fn step(&mut self, action: Action) -> anyhow::Result<Step>;
}

#[async_trait]
pub trait Agent: Send {
    async fn act(&mut self, observation: &Observation) -> anyhow::Result<Action>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_player_action_is_a_one_action_plan() {
        let action = Action::press(Key::Right);
        assert_eq!(
            action.plan_ref().actions,
            vec![PlayerAction::KeyPress(Key::Right)]
        );
    }

    #[test]
    fn wait_is_a_one_action_plan() {
        let action = Action::wait();
        assert_eq!(action.plan_ref().actions, vec![PlayerAction::Wait]);
    }

    #[test]
    fn repeated_waits_remain_one_agent_action() {
        let action = Action::repeat(PlayerAction::Wait, 10).unwrap();
        assert_eq!(action.plan_ref().actions, vec![PlayerAction::Wait; 10]);
    }

    #[test]
    fn empty_action_plan_is_rejected() {
        assert!(Action::plan(Vec::new()).is_err());
    }

    #[test]
    fn zero_repetition_is_rejected() {
        assert!(Action::repeat(PlayerAction::Wait, 0).is_err());
    }

    #[test]
    fn pacing_is_explicit_and_separate_from_settle() {
        let action = Action::plan(vec![
            PlayerAction::KeyPress(Key::Right),
            PlayerAction::KeyPress(Key::Right),
        ])
        .unwrap()
        .with_inter_action_delay(Duration::from_millis(25));

        assert_eq!(action.plan_ref().inter_action_delay_millis, 25);
    }
}
