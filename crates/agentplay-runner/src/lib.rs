use agentplay_core::{Agent, Environment, Observation};

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

            let plan = self.agent.act(&observation).await?;
            plan.validate()?;
            let action_count = plan.actions.len();
            let transition = self.environment.step(plan).await?;
            observation = transition.observation;
            steps += 1;

            tracing::debug!(steps, action_count, "completed environment step");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentplay_core::{Action, ActionPlan, Frame, Key, Step};
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

    struct FixedPlanAgent {
        plan: ActionPlan,
    }

    #[async_trait]
    impl Agent for FixedPlanAgent {
        async fn act(&mut self, _observation: &Observation) -> anyhow::Result<ActionPlan> {
            Ok(self.plan.clone())
        }
    }

    struct RecordingEnvironment {
        plans: Arc<Mutex<Vec<ActionPlan>>>,
    }

    #[async_trait]
    impl Environment for RecordingEnvironment {
        async fn observe(&mut self) -> anyhow::Result<Observation> {
            Ok(observation(0))
        }

        async fn step(&mut self, plan: ActionPlan) -> anyhow::Result<Step> {
            self.plans.lock().unwrap().push(plan);
            Ok(Step {
                observation: observation(1),
                settle_millis: 12,
            })
        }
    }

    #[tokio::test]
    async fn forwards_a_batch_as_one_environment_step() {
        let plans = Arc::new(Mutex::new(Vec::new()));
        let plan = ActionPlan::new(vec![
            Action::KeyPress(Key::Right),
            Action::KeyPress(Key::Right),
            Action::KeyPress(Key::Up),
        ])
        .unwrap();

        let runner = Runner::new(
            RecordingEnvironment {
                plans: Arc::clone(&plans),
            },
            FixedPlanAgent { plan: plan.clone() },
            RunLimits { max_steps: Some(1) },
        );

        let final_observation = runner.run().await.unwrap();

        assert_eq!(final_observation.sequence, 1);
        assert_eq!(plans.lock().unwrap().as_slice(), &[plan]);
    }

    #[tokio::test]
    async fn rejects_an_empty_plan_before_environment_execution() {
        let plans = Arc::new(Mutex::new(Vec::new()));
        let runner = Runner::new(
            RecordingEnvironment {
                plans: Arc::clone(&plans),
            },
            FixedPlanAgent {
                plan: ActionPlan {
                    actions: Vec::new(),
                    inter_action_delay_millis: 0,
                },
            },
            RunLimits { max_steps: Some(1) },
        );

        let error = runner.run().await.unwrap_err();

        assert!(error.to_string().contains("at least one action"));
        assert!(plans.lock().unwrap().is_empty());
    }
}
