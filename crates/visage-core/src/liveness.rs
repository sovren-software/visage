//! Passive liveness detection via landmark stability analysis.
//!
//! A static photograph or printed image produces near-identical facial landmark
//! positions across consecutive frames. A live person exhibits involuntary
//! micro-saccades and natural eye drift that cause measurable landmark
//! displacement between frames — even when staring directly at the camera.
//!
//! This module provides a lightweight, zero-model liveness gate that operates
//! on SCRFD landmark data already produced by the detection pipeline. It adds
//! no additional inference, no extra frames, and no user interaction.
//!
//! # Threat Coverage
//!
//! - **Blocks:** Printed photographs, static IR images held in front of camera.
//! - **Does not block:** Video replay attacks (landmarks move in video),
//!   high-quality 3D masks, or adversarial displays.

/// Result of a landmark stability liveness check.
#[derive(Debug, Clone)]
pub struct LivenessResult {
    /// Whether the frames passed the liveness check (true = likely live).
    pub is_live: bool,
    /// Mean Euclidean displacement of eye landmarks across consecutive frame pairs.
    pub mean_eye_displacement: f32,
    /// Number of frame pairs analysed.
    pub frame_pairs_analysed: usize,
}

/// Default minimum eye displacement (in pixels) below which frames are
/// considered suspiciously static. Empirically, even a steady gaze at a
/// fixed point produces >1.0 px of involuntary eye movement between frames
/// at 30 fps on a 640×480 sensor. A printed photo produces <0.3 px (sensor
/// noise only).
const DEFAULT_MIN_EYE_DISPLACEMENT: f32 = 0.8;

/// Check whether a sequence of detected facial landmarks exhibits sufficient
/// eye movement to indicate a live subject.
///
/// # Arguments
///
/// * `landmark_sequence` — Landmarks from consecutive frames. Each entry is
///   the 5-point landmark array `[(f32, f32); 5]` where indices 0 and 1 are
///   the left and right eye centres respectively (SCRFD convention).
/// * `min_displacement` — Minimum mean eye displacement threshold. If `None`,
///   uses [`DEFAULT_MIN_EYE_DISPLACEMENT`].
///
/// # Returns
///
/// A [`LivenessResult`] indicating whether the landmark sequence passes the
/// liveness check. **Fails closed:** if fewer than 2 landmark frames are
/// provided there is no frame pair to compare, so the check cannot gather any
/// eye-movement evidence and returns `is_live = false`. A liveness gate must
/// never vouch for a subject it could not actually assess — reporting "live" on
/// missing evidence would let a spoof that yields only a single detectable
/// landmark frame bypass the check entirely. The engine invokes this only when
/// liveness is enabled *and* a match otherwise succeeded, where a fail-closed
/// result is surfaced as a (rate-limited) non-match and the user simply retries.
pub fn check_landmark_stability(
    landmark_sequence: &[[(f32, f32); 5]],
    min_displacement: Option<f32>,
) -> LivenessResult {
    let threshold = min_displacement.unwrap_or(DEFAULT_MIN_EYE_DISPLACEMENT);

    // Fewer than 2 frames → no frame pair → no eye-movement evidence.
    // Fail closed: a liveness check that cannot gather evidence must NOT report live.
    if landmark_sequence.len() < 2 {
        return LivenessResult {
            is_live: false,
            mean_eye_displacement: 0.0,
            frame_pairs_analysed: 0,
        };
    }

    let mut total_displacement = 0.0f32;
    let mut pair_count = 0usize;

    for pair in landmark_sequence.windows(2) {
        let prev = &pair[0];
        let curr = &pair[1];

        // Left eye (index 0) displacement
        let left_dx = curr[0].0 - prev[0].0;
        let left_dy = curr[0].1 - prev[0].1;
        let left_disp = (left_dx * left_dx + left_dy * left_dy).sqrt();

        // Right eye (index 1) displacement
        let right_dx = curr[1].0 - prev[1].0;
        let right_dy = curr[1].1 - prev[1].1;
        let right_disp = (right_dx * right_dx + right_dy * right_dy).sqrt();

        // Average displacement of both eyes for this frame pair
        total_displacement += (left_disp + right_disp) / 2.0;
        pair_count += 1;
    }

    let mean_displacement = if pair_count > 0 {
        total_displacement / pair_count as f32
    } else {
        0.0
    };

    LivenessResult {
        is_live: mean_displacement >= threshold,
        mean_eye_displacement: mean_displacement,
        frame_pairs_analysed: pair_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create landmarks with specified eye positions, other landmarks at origin.
    fn landmarks_with_eyes(left: (f32, f32), right: (f32, f32)) -> [(f32, f32); 5] {
        [
            left,
            right,
            (0.0, 0.0), // nose
            (0.0, 0.0), // left mouth
            (0.0, 0.0), // right mouth
        ]
    }

    #[test]
    fn test_single_frame_fails_closed() {
        // Cannot determine liveness with 1 frame (no pair to compare) — must
        // fail closed (reject) rather than vouch for an unassessed subject.
        let seq = vec![landmarks_with_eyes((100.0, 50.0), (140.0, 50.0))];
        let result = check_landmark_stability(&seq, None);
        assert!(!result.is_live);
        assert_eq!(result.frame_pairs_analysed, 0);
    }

    #[test]
    fn test_empty_sequence_fails_closed() {
        // No landmark data at all — must fail closed.
        let result = check_landmark_stability(&[], None);
        assert!(!result.is_live);
        assert_eq!(result.frame_pairs_analysed, 0);
    }

    #[test]
    fn test_identical_landmarks_rejected() {
        // Perfectly identical landmarks across 3 frames = static image
        let lm = landmarks_with_eyes((100.0, 50.0), (140.0, 50.0));
        let seq = vec![lm, lm, lm];
        let result = check_landmark_stability(&seq, None);
        assert!(!result.is_live);
        assert_eq!(result.frame_pairs_analysed, 2);
        assert!(result.mean_eye_displacement < 1e-6);
    }

    #[test]
    fn test_near_identical_landmarks_rejected() {
        // Tiny displacement (sensor noise level: ~0.2 px) = still static
        let seq = vec![
            landmarks_with_eyes((100.0, 50.0), (140.0, 50.0)),
            landmarks_with_eyes((100.1, 50.1), (140.1, 50.1)),
            landmarks_with_eyes((100.0, 50.0), (140.0, 50.0)),
        ];
        let result = check_landmark_stability(&seq, None);
        assert!(!result.is_live);
        assert!(result.mean_eye_displacement < DEFAULT_MIN_EYE_DISPLACEMENT);
    }

    #[test]
    fn test_natural_movement_passes() {
        // Simulated natural micro-saccade movement (~1.5 px between frames)
        let seq = vec![
            landmarks_with_eyes((100.0, 50.0), (140.0, 50.0)),
            landmarks_with_eyes((101.2, 50.8), (141.0, 50.6)),
            landmarks_with_eyes((100.5, 49.5), (140.3, 49.8)),
        ];
        let result = check_landmark_stability(&seq, None);
        assert!(result.is_live);
        assert!(result.mean_eye_displacement >= DEFAULT_MIN_EYE_DISPLACEMENT);
        assert_eq!(result.frame_pairs_analysed, 2);
    }

    #[test]
    fn test_large_movement_passes() {
        // Deliberate head movement — clearly live
        let seq = vec![
            landmarks_with_eyes((100.0, 50.0), (140.0, 50.0)),
            landmarks_with_eyes((105.0, 52.0), (145.0, 52.0)),
        ];
        let result = check_landmark_stability(&seq, None);
        assert!(result.is_live);
        assert!(result.mean_eye_displacement > 5.0);
    }

    #[test]
    fn test_custom_threshold() {
        // Use a very low threshold — even tiny movement passes
        let seq = vec![
            landmarks_with_eyes((100.0, 50.0), (140.0, 50.0)),
            landmarks_with_eyes((100.1, 50.1), (140.1, 50.1)),
        ];
        let result = check_landmark_stability(&seq, Some(0.1));
        assert!(result.is_live);
    }

    #[test]
    fn test_custom_high_threshold() {
        // Use a very high threshold — moderate movement fails
        let seq = vec![
            landmarks_with_eyes((100.0, 50.0), (140.0, 50.0)),
            landmarks_with_eyes((101.0, 50.5), (141.0, 50.5)),
        ];
        let result = check_landmark_stability(&seq, Some(5.0));
        assert!(!result.is_live);
    }

    #[test]
    fn test_two_frames_minimum() {
        // Exactly 2 frames = 1 pair, should work
        let seq = vec![
            landmarks_with_eyes((100.0, 50.0), (140.0, 50.0)),
            landmarks_with_eyes((102.0, 51.0), (142.0, 51.0)),
        ];
        let result = check_landmark_stability(&seq, None);
        assert_eq!(result.frame_pairs_analysed, 1);
        // displacement = sqrt(4+1) ≈ 2.236 for each eye
        assert!(result.mean_eye_displacement > 2.0);
        assert!(result.is_live);
    }

    #[test]
    fn test_displacement_calculation_accuracy() {
        // Known geometry: move right eye 3px right, 4px down → displacement = 5.0
        let seq = vec![
            landmarks_with_eyes((100.0, 50.0), (140.0, 50.0)),
            landmarks_with_eyes((100.0, 50.0), (143.0, 54.0)),
        ];
        let result = check_landmark_stability(&seq, None);
        // Left eye: 0.0, Right eye: 5.0, mean: 2.5
        assert!((result.mean_eye_displacement - 2.5).abs() < 1e-6);
    }

    #[test]
    fn test_mean_across_multiple_pairs() {
        // 3 frames = 2 pairs
        // Pair 1: left_disp=1.0, right_disp=1.0, avg=1.0
        // Pair 2: left_disp=0.0, right_disp=0.0, avg=0.0
        // Mean across pairs: 0.5
        let seq = vec![
            landmarks_with_eyes((100.0, 50.0), (140.0, 50.0)),
            landmarks_with_eyes((101.0, 50.0), (141.0, 50.0)),
            landmarks_with_eyes((101.0, 50.0), (141.0, 50.0)),
        ];
        let result = check_landmark_stability(&seq, None);
        assert_eq!(result.frame_pairs_analysed, 2);
        assert!((result.mean_eye_displacement - 0.5).abs() < 1e-6);
    }
}
