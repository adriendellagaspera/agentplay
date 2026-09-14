use agentplay_core::{Frame, QuiescencePolicy};
use anyhow::ensure;

pub trait FrameDifferenceMetric: Send {
    fn difference(&mut self, previous: &Frame, current: &Frame) -> anyhow::Result<u32>;
}

#[derive(Clone, Debug)]
pub struct BlockDifferenceMetric {
    pub block_size: u32,
    pub mean_channel_delta_threshold: u8,
}

impl Default for BlockDifferenceMetric {
    fn default() -> Self {
        Self {
            block_size: 8,
            mean_channel_delta_threshold: 8,
        }
    }
}

impl FrameDifferenceMetric for BlockDifferenceMetric {
    fn difference(&mut self, previous: &Frame, current: &Frame) -> anyhow::Result<u32> {
        ensure!(
            previous.width == current.width && previous.height == current.height,
            "frame dimensions changed during settling"
        );
        ensure!(self.block_size > 0, "block size must be greater than zero");

        let width = previous.width as usize;
        let height = previous.height as usize;
        let block = self.block_size as usize;
        let mut changed = 0_u64;
        let mut total = 0_u64;

        for y0 in (0..height).step_by(block) {
            for x0 in (0..width).step_by(block) {
                total += 1;
                let mut delta = 0_u64;
                let mut channels = 0_u64;
                for y in y0..(y0 + block).min(height) {
                    for x in x0..(x0 + block).min(width) {
                        let i = (y * width + x) * 4;
                        for channel in 0..3 {
                            delta += previous.rgba[i + channel]
                                .abs_diff(current.rgba[i + channel]) as u64;
                            channels += 1;
                        }
                    }
                }
                if delta / channels > self.mean_channel_delta_threshold as u64 {
                    changed += 1;
                }
            }
        }

        ensure!(total > 0, "frame must not be empty");
        Ok(((changed * 10_000) / total) as u32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettleReason {
    Stable,
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuiescenceDiagnostics {
    pub reason: SettleReason,
    pub elapsed_millis: u64,
    pub samples: u64,
    pub last_difference: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detection {
    Waiting,
    Settled(QuiescenceDiagnostics),
}

pub struct QuiescenceDetector<M> {
    policy: QuiescencePolicy,
    metric: M,
    previous: Option<Frame>,
    stable_since_millis: Option<u64>,
    samples: u64,
    last_difference: Option<u32>,
}

impl<M: FrameDifferenceMetric> QuiescenceDetector<M> {
    pub fn new(policy: QuiescencePolicy, metric: M) -> anyhow::Result<Self> {
        ensure!(policy.sample_every_millis > 0, "sample interval must be positive");
        ensure!(
            policy.stable_for_millis <= policy.max_wait_millis,
            "stable window must not exceed max wait"
        );
        Ok(Self {
            policy,
            metric,
            previous: None,
            stable_since_millis: None,
            samples: 0,
            last_difference: None,
        })
    }

    pub fn push(&mut self, frame: Frame, elapsed_millis: u64) -> anyhow::Result<Detection> {
        self.samples += 1;
        if let Some(previous) = &self.previous {
            let difference = self.metric.difference(previous, &frame)?;
            self.last_difference = Some(difference);
            if difference <= self.policy.difference_threshold {
                let stable_since = self.stable_since_millis.get_or_insert(elapsed_millis);
                if elapsed_millis.saturating_sub(*stable_since) >= self.policy.stable_for_millis {
                    self.previous = Some(frame);
                    return Ok(Detection::Settled(self.diagnostics(
                        SettleReason::Stable,
                        elapsed_millis,
                    )));
                }
            } else {
                self.stable_since_millis = None;
            }
        }
        self.previous = Some(frame);
        if elapsed_millis >= self.policy.max_wait_millis {
            return Ok(Detection::Settled(
                self.diagnostics(SettleReason::Timeout, elapsed_millis),
            ));
        }
        Ok(Detection::Waiting)
    }

    fn diagnostics(&self, reason: SettleReason, elapsed_millis: u64) -> QuiescenceDiagnostics {
        QuiescenceDiagnostics {
            reason,
            elapsed_millis,
            samples: self.samples,
            last_difference: self.last_difference,
        }
    }
}
