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


#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame {
            width: 64,
            height: 64,
            rgba: vec![0; 64 * 64 * 4],
        }
    }

    fn fill_block(frame: &mut Frame, bx: u32, by: u32, value: u8) {
        for y in by * 8..(by + 1) * 8 {
            for x in bx * 8..(bx + 1) * 8 {
                let i = ((y * frame.width + x) * 4) as usize;
                frame.rgba[i..i + 3].fill(value);
            }
        }
    }

    fn policy() -> QuiescencePolicy {
        QuiescencePolicy {
            sample_every_millis: 50,
            stable_for_millis: 200,
            max_wait_millis: 1_000,
            difference_threshold: 500,
        }
    }

    #[test]
    fn local_idle_animation_is_below_threshold() {
        let a = frame();
        let mut b = a.clone();
        fill_block(&mut b, 0, 0, 255);
        let score = BlockDifferenceMetric::default().difference(&a, &b).unwrap();
        assert_eq!(score, 156);
        assert!(score <= policy().difference_threshold);
    }

    #[test]
    fn large_scene_change_is_above_threshold() {
        let a = frame();
        let mut b = a.clone();
        for y in 0..4 {
            for x in 0..4 {
                fill_block(&mut b, x, y, 255);
            }
        }
        let score = BlockDifferenceMetric::default().difference(&a, &b).unwrap();
        assert_eq!(score, 2_500);
        assert!(score > policy().difference_threshold);
    }

    #[test]
    fn persistent_local_animation_can_settle() {
        let mut detector =
            QuiescenceDetector::new(policy(), BlockDifferenceMetric::default()).unwrap();
        let base = frame();
        assert_eq!(detector.push(base.clone(), 0).unwrap(), Detection::Waiting);

        for elapsed in [50, 100, 150, 200] {
            let mut animated = base.clone();
            fill_block(
                &mut animated,
                0,
                0,
                if elapsed % 100 == 0 { 255 } else { 128 },
            );
            assert_eq!(detector.push(animated, elapsed).unwrap(), Detection::Waiting);
        }

        let mut animated = base;
        fill_block(&mut animated, 0, 0, 255);
        assert!(matches!(
            detector.push(animated, 250).unwrap(),
            Detection::Settled(QuiescenceDiagnostics {
                reason: SettleReason::Stable,
                ..
            })
        ));
    }

    #[test]
    fn meaningful_change_resets_stability_window() {
        let mut detector =
            QuiescenceDetector::new(policy(), BlockDifferenceMetric::default()).unwrap();
        let base = frame();
        detector.push(base.clone(), 0).unwrap();
        detector.push(base.clone(), 50).unwrap();
        detector.push(base.clone(), 100).unwrap();

        let mut changed = base.clone();
        for y in 0..4 {
            for x in 0..4 {
                fill_block(&mut changed, x, y, 255);
            }
        }
        assert_eq!(detector.push(changed, 150).unwrap(), Detection::Waiting);

        for elapsed in [200, 250, 300, 350] {
            assert_eq!(
                detector.push(base.clone(), elapsed).unwrap(),
                Detection::Waiting
            );
        }
        assert!(matches!(
            detector.push(base, 400).unwrap(),
            Detection::Settled(QuiescenceDiagnostics {
                reason: SettleReason::Stable,
                ..
            })
        ));
    }

    #[test]
    fn timeout_is_reported() {
        let mut detector =
            QuiescenceDetector::new(policy(), BlockDifferenceMetric::default()).unwrap();
        let a = frame();
        detector.push(a.clone(), 0).unwrap();
        let mut b = a;
        for y in 0..4 {
            for x in 0..4 {
                fill_block(&mut b, x, y, 255);
            }
        }
        assert!(matches!(
            detector.push(b, 1_000).unwrap(),
            Detection::Settled(QuiescenceDiagnostics {
                reason: SettleReason::Timeout,
                ..
            })
        ));
    }
}
