use agentplay_core::{Frame, QuiescencePolicy};
use agentplay_runner::quiescence::{
    BlockDifferenceMetric, Detection, QuiescenceDetector, SettleReason,
};

fn frame(value: u8) -> Frame {
    let mut rgba = vec![0_u8; 64 * 64 * 4];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = 255;
    }

    for y in 0..16 {
        for x in 0..16 {
            let i = (y * 64 + x) * 4;
            rgba[i..i + 3].fill(value);
        }
    }

    Frame {
        width: 64,
        height: 64,
        rgba,
    }
}

fn run(history: usize) -> Detection {
    let policy = QuiescencePolicy {
        sample_every_millis: 50,
        stable_for_millis: 200,
        max_wait_millis: 1_000,
        difference_threshold: 100,
        max_cycle_frames: history,
    };
    let phases = [frame(32), frame(128), frame(255)];
    let mut detector = QuiescenceDetector::new(policy, BlockDifferenceMetric::default()).unwrap();
    let mut result = Detection::Waiting;

    for sample in 0_usize..=20 {
        let phase = (sample / 3) % phases.len();
        result = detector
            .push(phases[phase].clone(), sample as u64 * 50)
            .unwrap();
        if matches!(result, Detection::Settled(_)) {
            break;
        }
    }

    result
}

#[test]
fn held_visual_cycle_needs_history_longer_than_phase_count() {
    assert!(matches!(
        run(4),
        Detection::Settled(diagnostics) if diagnostics.reason == SettleReason::Timeout
    ));

    assert!(matches!(
        run(QuiescencePolicy::default().max_cycle_frames),
        Detection::Settled(diagnostics) if diagnostics.reason == SettleReason::Stable
    ));
}
