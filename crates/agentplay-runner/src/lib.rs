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

            let action = self.agent.act(&observation).await?;
            let transition = self.environment.step(action).await?;
            observation = transition.observation;
            steps += 1;

            tracing::debug!(steps, "completed environment step");
        }
    }
}
