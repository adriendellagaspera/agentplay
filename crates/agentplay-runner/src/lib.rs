pub mod quiescence;

use agentplay_core::{Action, Agent, Decision, Environment, Observation, QuiescencePolicy};
use quiescence::{FrameDifferenceMetric, QuiescenceResult, settle_until_quiescent};
use std::time::Duration;

/// Applies every Action in a Decision before starting one fresh post-Decision settle attempt.
pub async fn step_decision<S, M, A, C>(
    state: &mut S,
    decision: &Decision,
    mut apply: A,
    mut capture: C,
    policy: QuiescencePolicy,
    metric: M,
) -> anyhow::Result<QuiescenceResult>
where
    M: FrameDifferenceMetric,
    A: FnMut(&mut S, &Action) -> anyhow::Result<()>,
    C: FnMut(&mut S) -> anyhow::Result<agentplay_core::Frame>,
{
    decision.validate()?;

    for (index, action) in decision.actions.iter().enumerate() {
        apply(state, action)?;

        let has_next_action = index + 1 < decision.actions.len();
        if has_next_action && decision.inter_action_delay_millis > 0 {
            tokio::time::sleep(Duration::from_millis(decision.inter_action_delay_millis)).await;
        }
    }

    settle_until_quiescent(policy, metric, || capture(state)).await
}

#[derive(Clone, Debug, Default)]
pub struct RunLimits {
    pub max_steps: Option<u64>,
}

pub struct Runner<E, A> {
    environment: E,
    agent: A,
    limits: RunLimits,
}

impl<E, A> Runner<E, A>
where
    E: Environment,
    A: Agent,
{
    pub fn new(environment: E, agent: A, limits: RunLimits) -> Self {
        Self {
            environment,
            agent,
            limits,
        }
    }

    pub async fn run(mut self) -> anyhow::Result<Observation> {
        let mut observation = self.environment.observe().await?;
        let mut steps = 0_u64;

        loop {
            if self.limits.max_steps.is_some_and(|max| steps >= max) {
                return Ok(observation);
            }

            let decision = self.agent.decide(&observation).await?;
            decision.validate()?;
            let action_count = decision.actions.len();
            let transition = self.environment.step(decision).await?;
            observation = transition.observation;
            steps += 1;

            tracing::debug!(steps, action_count, "completed environment step");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentplay_core::{Frame, Key, Step};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    fn observation(sequence: u64) -> Observation {
        Observation {
            frame: Frame {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            },
            sequence,
        }
    }

    fn settle_policy() -> QuiescencePolicy {
        QuiescencePolicy {
            sample_every_millis: 1,
            stable_for_millis: 0,
            max_wait_millis: 20,
            difference_threshold: 0,
            max_cycle_frames: 1,
        }
    }

    struct FixedDecisionAgent {
        decision: Decision,
    }

    #[async_trait]
    impl Agent for FixedDecisionAgent {
        async fn decide(&mut self, _observation: &Observation) -> anyhow::Result<Decision> {
            Ok(self.decision.clone())
        }
    }

    struct RecordingEnvironment {
        decisions: Arc<Mutex<Vec<Decision>>>,
    }

    #[async_trait]
    impl Environment for RecordingEnvironment {
        async fn observe(&mut self) -> anyhow::Result<Observation> {
            Ok(observation(0))
        }

        async fn step(&mut self, decision: Decision) -> anyhow::Result<Step> {
            self.decisions.lock().unwrap().push(decision);
            Ok(Step {
                observation: observation(1),
                settle_millis: 12,
            })
        }
    }

    #[tokio::test]
    async fn forwards_multiple_actions_as_one_environment_step() {
        let decisions = Arc::new(Mutex::new(Vec::new()));
        let decision = Decision::new(vec![
            Action::KeyPress(Key::Right),
            Action::KeyPress(Key::Right),
            Action::KeyPress(Key::Up),
        ])
        .unwrap();

        let runner = Runner::new(
            RecordingEnvironment {
                decisions: Arc::clone(&decisions),
            },
            FixedDecisionAgent {
                decision: decision.clone(),
            },
            RunLimits { max_steps: Some(1) },
        );

        let final_observation = runner.run().await.unwrap();

        assert_eq!(final_observation.sequence, 1);
        assert_eq!(decisions.lock().unwrap().as_slice(), &[decision]);
    }

    #[tokio::test]
    async fn repeated_waits_are_forwarded_as_one_environment_step() {
        let decisions = Arc::new(Mutex::new(Vec::new()));
        let decision = Decision::repeat(Action::Wait, 10).unwrap();

        let runner = Runner::new(
            RecordingEnvironment {
                decisions: Arc::clone(&decisions),
            },
            FixedDecisionAgent {
                decision: decision.clone(),
            },
            RunLimits { max_steps: Some(1) },
        );

        runner.run().await.unwrap();

        assert_eq!(decisions.lock().unwrap().as_slice(), &[decision]);
    }

    #[tokio::test]
    async fn rejects_an_empty_decision_before_environment_execution() {
        let decisions = Arc::new(Mutex::new(Vec::new()));
        let runner = Runner::new(
            RecordingEnvironment {
                decisions: Arc::clone(&decisions),
            },
            FixedDecisionAgent {
                decision: Decision {
                    actions: Vec::new(),
                    inter_action_delay_millis: 0,
                },
            },
            RunLimits { max_steps: Some(1) },
        );

        let error = runner.run().await.unwrap_err();

        assert!(error.to_string().contains("at least one action"));
        assert!(decisions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn step_decision_applies_every_action_before_first_settle_capture() {
        #[derive(Default)]
        struct State {
            events: Vec<&'static str>,
        }

        let mut state = State::default();
        let decision = Decision::repeat(Action::Wait, 3).unwrap();

        let result = step_decision(
            &mut state,
            &decision,
            |state, action| {
                assert_eq!(action, &Action::Wait);
                state.events.push("action");
                Ok(())
            },
            |state| {
                state.events.push("capture");
                Ok(observation(0).frame)
            },
            settle_policy(),
            quiescence::BlockDifferenceMetric::default(),
        )
        .await
        .unwrap();

        assert_eq!(
            state.events,
            vec!["action", "action", "action", "capture", "capture"]
        );
        assert_eq!(result.diagnostics.samples, 2);
    }

    #[tokio::test]
    async fn each_step_decision_starts_with_fresh_settle_history() {
        #[derive(Default)]
        struct State {
            captures: usize,
        }

        let mut state = State::default();
        let decision = Decision::wait();

        for _ in 0..2 {
            let result = step_decision(
                &mut state,
                &decision,
                |_state, _action| Ok(()),
                |state| {
                    state.captures += 1;
                    Ok(observation(0).frame)
                },
                settle_policy(),
                quiescence::BlockDifferenceMetric::default(),
            )
            .await
            .unwrap();

            assert_eq!(result.diagnostics.samples, 2);
        }

        assert_eq!(state.captures, 4);
    }
}
