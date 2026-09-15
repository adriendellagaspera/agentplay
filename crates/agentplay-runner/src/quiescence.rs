use agentplay_core::{Frame, QuiescencePolicy};
use anyhow::ensure;
use serde::Serialize;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

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
            block_size: 16,
            mean_channel_delta_threshold: 5,
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
        let expected_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| anyhow::anyhow!("frame dimensions overflow"))?;
        ensure!(
            previous.rgba.len() == expected_len && current.rgba.len() == expected_len,
            "invalid RGBA buffer length"
        );

        let block = self.block_size as usize;
        let mut changed = 0_u64;
        let mut total = 0_u64;

        for y0 in (0..height).step_by(block) {
            for x0 in (0..width).step_by(block) {
                total += 1;
                let mut previous_sum = [0_u64; 3];
                let mut current_sum = [0_u64; 3];
                let mut pixels = 0_u64;

                for y in y0..(y0 + block).min(height) {
                    for x in x0..(x0 + block).min(width) {
                        let i = (y * width + x) * 4;
                        for channel in 0..3 {
                            previous_sum[channel] += previous.rgba[i + channel] as u64;
                            current_sum[channel] += current.rgba[i + channel] as u64;
                        }
                        pixels += 1;
                    }
                }

                let mean_channel_delta = (0..3)
                    .map(|channel| previous_sum[channel].abs_diff(current_sum[channel]) / pixels)
                    .sum::<u64>()
                    / 3;

                if mean_channel_delta > self.mean_channel_delta_threshold as u64 {
                    changed += 1;
                }
            }
        }

        ensure!(total > 0, "frame must not be empty");
        Ok(((changed * 10_000) / total) as u32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SettleReason {
    Stable,
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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

/// Detects a stable or recurrent visual regime using only frames supplied to this instance.
///
/// Create a fresh detector after a complete `Decision` has executed. Frames captured before or
/// between the `Action`s of that `Decision` must never be pushed into this detector.
pub struct QuiescenceDetector<M> {
    policy: QuiescencePolicy,
    metric: M,
    recent: VecDeque<Frame>,
    stable_since_millis: Option<u64>,
    samples: u64,
    last_difference: Option<u32>,
}

impl<M: FrameDifferenceMetric> QuiescenceDetector<M> {
    pub fn new(policy: QuiescencePolicy, metric: M) -> anyhow::Result<Self> {
        ensure!(
            policy.sample_every_millis > 0,
            "sample interval must be positive"
        );
        ensure!(
            policy.stable_for_millis <= policy.max_wait_millis,
            "stable window must not exceed max wait"
        );
        ensure!(
            policy.max_cycle_frames > 0,
            "maximum cycle length must be greater than zero"
        );

        let max_cycle_frames = policy.max_cycle_frames;
        Ok(Self {
            policy,
            metric,
            recent: VecDeque::with_capacity(max_cycle_frames),
            stable_since_millis: None,
            samples: 0,
            last_difference: None,
        })
    }

    pub fn push(&mut self, frame: Frame, elapsed_millis: u64) -> anyhow::Result<Detection> {
        self.samples += 1;

        let residual = self
            .recent
            .iter()
            .map(|reference| self.metric.difference(reference, &frame))
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .min();

        if let Some(residual) = residual {
            self.last_difference = Some(residual);
            if residual <= self.policy.difference_threshold {
                let stable_since = self.stable_since_millis.get_or_insert(elapsed_millis);
                if elapsed_millis.saturating_sub(*stable_since) >= self.policy.stable_for_millis {
                    return Ok(Detection::Settled(
                        self.diagnostics(SettleReason::Stable, elapsed_millis),
                    ));
                }
            } else {
                self.stable_since_millis = None;
            }
        }

        self.recent.push_back(frame);
        while self.recent.len() > self.policy.max_cycle_frames {
            self.recent.pop_front();
        }

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

#[derive(Clone, Debug)]
pub struct QuiescenceResult {
    pub frame: Frame,
    pub diagnostics: QuiescenceDiagnostics,
}

/// Samples a fresh post-Decision visual history until it becomes recurrent/stable or times out.
pub async fn settle_until_quiescent<M, C>(
    policy: QuiescencePolicy,
    metric: M,
    mut capture: C,
) -> anyhow::Result<QuiescenceResult>
where
    M: FrameDifferenceMetric,
    C: FnMut() -> anyhow::Result<Frame>,
{
    let sample_every = Duration::from_millis(policy.sample_every_millis);
    let started = Instant::now();
    let mut detector = QuiescenceDetector::new(policy, metric)?;

    loop {
        let frame = capture()?;
        let elapsed_millis = started.elapsed().as_millis() as u64;

        match detector.push(frame.clone(), elapsed_millis)? {
            Detection::Waiting => tokio::time::sleep(sample_every).await,
            Detection::Settled(diagnostics) => {
                return Ok(QuiescenceResult { frame, diagnostics });
            }
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
            difference_threshold: 100,
            max_cycle_frames: 4,
        }
    }

    fn three_phase_state(tile_x: u32) -> [Frame; 3] {
        let mut phases = [frame(), frame(), frame()];
        for (phase, value) in phases.iter_mut().zip([32, 128, 255]) {
            fill_block(phase, tile_x, 1, 255);
            fill_block(phase, 0, 0, value);
        }
        phases
    }

    #[test]
    fn short_post_decision_cycle_settles_without_consecutive_frame_stability() {
        let mut detector =
            QuiescenceDetector::new(policy(), BlockDifferenceMetric::default()).unwrap();
        let phases = three_phase_state(2);
        let mut result = Detection::Waiting;

        for (sample, elapsed) in (0_usize..).zip((0_u64..=450).step_by(50)) {
            result = detector
                .push(phases[sample % phases.len()].clone(), elapsed)
                .unwrap();
            if matches!(result, Detection::Settled(_)) {
                break;
            }
        }

        assert!(matches!(
            result,
            Detection::Settled(QuiescenceDiagnostics {
                reason: SettleReason::Stable,
                ..
            })
        ));
    }

    #[test]
    fn fresh_detector_does_not_reuse_previous_decision_history() {
        let old_phases = three_phase_state(1);
        let new_phases = three_phase_state(3);

        let mut old_detector =
            QuiescenceDetector::new(policy(), BlockDifferenceMetric::default()).unwrap();
        for (sample, elapsed) in (0_usize..3).zip((0_u64..).step_by(50)) {
            old_detector
                .push(old_phases[sample].clone(), elapsed)
                .unwrap();
        }

        let mut new_detector =
            QuiescenceDetector::new(policy(), BlockDifferenceMetric::default()).unwrap();
        assert_eq!(
            new_detector.push(new_phases[0].clone(), 0).unwrap(),
            Detection::Waiting
        );
        assert_eq!(
            new_detector.push(new_phases[1].clone(), 50).unwrap(),
            Detection::Waiting
        );
        assert_eq!(
            new_detector.push(new_phases[2].clone(), 100).unwrap(),
            Detection::Waiting
        );
    }

    #[test]
    fn moving_automaton_can_settle_into_new_post_decision_cycle() {
        let mut detector =
            QuiescenceDetector::new(policy(), BlockDifferenceMetric::default()).unwrap();

        let mut transition = frame();
        fill_block(&mut transition, 1, 1, 255);
        assert_eq!(detector.push(transition, 0).unwrap(), Detection::Waiting);

        let phases = three_phase_state(2);
        let mut result = Detection::Waiting;
        for (sample, elapsed) in (0_usize..).zip((50_u64..=500).step_by(50)) {
            result = detector
                .push(phases[sample % phases.len()].clone(), elapsed)
                .unwrap();
            if matches!(result, Detection::Settled(_)) {
                break;
            }
        }

        assert!(matches!(
            result,
            Detection::Settled(QuiescenceDiagnostics {
                reason: SettleReason::Stable,
                ..
            })
        ));
    }

    #[test]
    fn idle_phases_may_differ_above_threshold_and_still_settle_by_recurrence() {
        let phases = three_phase_state(2);
        let adjacent_score = BlockDifferenceMetric::default()
            .difference(&phases[0], &phases[1])
            .unwrap();
        assert!(adjacent_score > policy().difference_threshold);

        let mut detector =
            QuiescenceDetector::new(policy(), BlockDifferenceMetric::default()).unwrap();
        let mut result = Detection::Waiting;
        for (sample, elapsed) in (0_usize..).zip((0_u64..=450).step_by(50)) {
            result = detector
                .push(phases[sample % phases.len()].clone(), elapsed)
                .unwrap();
            if matches!(result, Detection::Settled(_)) {
                break;
            }
        }

        assert!(matches!(
            result,
            Detection::Settled(QuiescenceDiagnostics {
                reason: SettleReason::Stable,
                ..
            })
        ));
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

        assert!(score >= 1_000);
        assert!(score > policy().difference_threshold);
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
