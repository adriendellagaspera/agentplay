use agentplay_core::{Action, Agent, Environment, Observation};

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

            let action = self.agent.act(&observation).await?;
            action.validate()?;
            let player_action_count = action.plan_ref().actions.len();
            let transition = self.environment.step(action).await?;
            observation = transition.observation;
            steps += 1;

            tracing::debug!(
                steps,
                player_action_count,
                "completed environment step"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentplay_core::{Frame, Key, PlayerAction, Step};
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

    struct FixedActionAgent {
        action: Action,
    }

    #[async_trait]
    impl Agent for FixedActionAgent {
        async fn act(&mut self, _observation: &Observation) -> anyhow::Result<Action> {
            Ok(self.action.clone())
        }
    }

    struct RecordingEnvironment {
        actions: Arc<Mutex<Vec<Action>>>,
    }

    #[async_trait]
    impl Environment for RecordingEnvironment {
        async fn observe(&mut self) -> anyhow::Result<Observation> {
            Ok(observation(0))
        }

        async fn step(&mut self, action: Action) -> anyhow::Result<Step> {
            self.actions.lock().unwrap().push(action);
            Ok(Step {
                observation: observation(1),
                settle_millis: 12,
            })
        }
    }

    #[tokio::test]
    async fn forwards_a_plan_as_one_environment_step() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let action = Action::plan(vec![
            PlayerAction::KeyPress(Key::Right),
            PlayerAction::KeyPress(Key::Right),
            PlayerAction::KeyPress(Key::Up),
        ])
        .unwrap();

        let runner = Runner::new(
            RecordingEnvironment {
                actions: Arc::clone(&actions),
            },
            FixedActionAgent {
                action: action.clone(),
            },
            RunLimits { max_steps: Some(1) },
        );

        let final_observation = runner.run().await.unwrap();

        assert_eq!(final_observation.sequence, 1);
        assert_eq!(actions.lock().unwrap().as_slice(), &[action]);
    }

    #[tokio::test]
    async fn repeated_waits_are_forwarded_as_one_environment_step() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let action = Action::repeat(PlayerAction::Wait, 10).unwrap();

        let runner = Runner::new(
            RecordingEnvironment {
                actions: Arc::clone(&actions),
            },
            FixedActionAgent {
                action: action.clone(),
            },
            RunLimits { max_steps: Some(1) },
        );

        runner.run().await.unwrap();

        assert_eq!(actions.lock().unwrap().as_slice(), &[action]);
    }

    #[tokio::test]
    async fn rejects_an_empty_plan_before_environment_execution() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let runner = Runner::new(
            RecordingEnvironment {
                actions: Arc::clone(&actions),
            },
            FixedActionAgent {
                action: Action::Plan(agentplay_core::ActionPlan {
                    actions: Vec::new(),
                    inter_action_delay_millis: 0,
                }),
            },
            RunLimits { max_steps: Some(1) },
        );

        let error = runner.run().await.unwrap_err();

        assert!(error.to_string().contains("at least one player action"));
        assert!(actions.lock().unwrap().is_empty());
    }
}
