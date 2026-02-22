//! Face alignment via 4-DOF similarity transform.
//!
//! Aligns detected faces to a canonical 112×112 position using the five
//! InsightFace reference landmarks and least-squares estimation.

/// ArcFace reference landmarks for a 112×112 output.
const REFERENCE_LANDMARKS_112: [(f32, f32); 5] = [
    (38.2946, 51.6963), // left eye
    (73.5318, 51.5014), // right eye
    (56.0252, 71.7366), // nose
    (41.5493, 92.3655), // left mouth
    (70.7299, 92.2041), // right mouth
];

const ALIGNED_SIZE: usize = 112;

/// Estimate a 2×3 similarity transform (4-DOF: scale, rotation, translation)
/// from `src` landmarks to `dst` landmarks using least-squares.
///
/// Returns [a, -b, tx, b, a, ty] representing the matrix:
/// ```text
/// | a  -b  tx |
/// | b   a  ty |
/// ```
fn estimate_similarity_transform(src: &[(f32, f32); 5], dst: &[(f32, f32); 5]) -> [f32; 6] {
    // Build overdetermined system A * [a, b, tx, ty]^T = B
    // For each point pair (sx, sy) -> (dx, dy):
    //   sx * a - sy * b + tx = dx
    //   sy * a + sx * b + ty = dy
    let mut ata = [0.0f32; 16]; // 4x4, row-major
    let mut atb = [0.0f32; 4]; // 4x1

    for i in 0..5 {
        let (sx, sy) = src[i];
        let (dx, dy) = dst[i];

        // Row 1: [sx, -sy, 1, 0] * [a, b, tx, ty]^T = dx
        let r1 = [sx, -sy, 1.0, 0.0];
        // Row 2: [sy, sx, 0, 1] * [a, b, tx, ty]^T = dy
        let r2 = [sy, sx, 0.0, 1.0];

        for j in 0..4 {
            for k in 0..4 {
                ata[j * 4 + k] += r1[j] * r1[k] + r2[j] * r2[k];
            }
            atb[j] += r1[j] * dx + r2[j] * dy;
        }
    }

    // Solve 4x4 system via Gaussian elimination with partial pivoting
    let x = solve_4x4(&ata, &atb);
    let (a, b, tx, ty) = (x[0], x[1], x[2], x[3]);

    [a, -b, tx, b, a, ty]
}

/// Solve a 4×4 linear system via Gaussian elimination with partial pivoting.
#[allow(clippy::needless_range_loop)]
fn solve_4x4(ata: &[f32; 16], atb: &[f32; 4]) -> [f32; 4] {
    // Augmented matrix [A | b] as 4x5
    let mut m = [[0.0f32; 5]; 4];
    for i in 0..4 {
        for j in 0..4 {
            m[i][j] = ata[i * 4 + j];
        }
        m[i][4] = atb[i];
    }

    // Forward elimination with partial pivoting
    for col in 0..4 {
        // Find pivot
        let mut max_row = col;
        let mut max_val = m[col][col].abs();
        for row in (col + 1)..4 {
            if m[row][col].abs() > max_val {
                max_val = m[row][col].abs();
                max_row = row;
            }
        }
        m.swap(col, max_row);

        let pivot = m[col][col];
        if pivot.abs() < 1e-12 {
            return [1.0, 0.0, 0.0, 0.0]; // fallback: identity-ish
        }

        for row in (col + 1)..4 {
            let factor = m[row][col] / pivot;
            for j in col..5 {
                m[row][j] -= factor * m[col][j];
            }
        }
    }

    // Back substitution
    let mut x = [0.0f32; 4];
    for i in (0..4).rev() {
        x[i] = m[i][4];
        for j in (i + 1)..4 {
            x[i] -= m[i][j] * x[j];
        }
        x[i] /= m[i][i];
    }

    x
}

/// Apply a 2×3 affine warp to produce an output image.
///
/// Uses bilinear interpolation. Out-of-bounds pixels are filled with 0 (black).
fn warp_affine(
    frame: &[u8],
    src_width: usize,
    src_height: usize,
    matrix: &[f32; 6],
    out_size: usize,
) -> Vec<u8> {
    let (a, _neg_b, tx) = (matrix[0], matrix[1], matrix[2]);
    let (b, _a2, ty) = (matrix[3], matrix[4], matrix[5]);

    // Invert the 2x2 part: M = [[a, -b], [b, a]], det = a^2 + b^2
    let det = a * a + b * b;
    if det.abs() < 1e-12 {
        return vec![0u8; out_size * out_size];
    }
    let inv_det = 1.0 / det;
    let ia = a * inv_det;
    let ib = b * inv_det;

    let mut output = vec![0u8; out_size * out_size];

    for oy in 0..out_size {
        for ox in 0..out_size {
            // Map output pixel back to source: src = M_inv * (dst - t)
            let dx = ox as f32 - tx;
            let dy = oy as f32 - ty;
            let sx = ia * dx + ib * dy;
            let sy = -ib * dx + ia * dy;

            // Bilinear interpolation
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let x1 = x0 + 1;
            let y1 = y0 + 1;
            let fx = sx - x0 as f32;
            let fy = sy - y0 as f32;

            let sample = |x: i32, y: i32| -> f32 {
                if x >= 0 && x < src_width as i32 && y >= 0 && y < src_height as i32 {
                    frame[y as usize * src_width + x as usize] as f32
                } else {
                    0.0
                }
            };

            let val = sample(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + sample(x1, y0) * fx * (1.0 - fy)
                + sample(x0, y1) * (1.0 - fx) * fy
                + sample(x1, y1) * fx * fy;

            output[oy * out_size + ox] = val.round().clamp(0.0, 255.0) as u8;
        }
    }

    output
}

/// Align a detected face to a canonical 112×112 crop.
///
/// Takes a grayscale frame and five detected facial landmarks, computes the
/// similarity transform to reference positions, and warps the face region
/// into a 112×112 aligned output suitable for ArcFace embedding extraction.
pub fn align_face(
    frame: &[u8],
    width: u32,
    height: u32,
    landmarks: &[(f32, f32); 5],
) -> Vec<u8> {
    let matrix = estimate_similarity_transform(landmarks, &REFERENCE_LANDMARKS_112);
    warp_affine(frame, width as usize, height as usize, &matrix, ALIGNED_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_transform() {
        // When src == dst, transform should be identity-like (a≈1, b≈0)
        let pts = REFERENCE_LANDMARKS_112;
        let m = estimate_similarity_transform(&pts, &pts);

        // a ≈ 1.0
        assert!((m[0] - 1.0).abs() < 1e-4, "a = {}", m[0]);
        // -b ≈ 0.0
        assert!(m[1].abs() < 1e-4, "-b = {}", m[1]);
        // tx ≈ 0.0
        assert!(m[2].abs() < 1e-3, "tx = {}", m[2]);
        // b ≈ 0.0
        assert!(m[3].abs() < 1e-4, "b = {}", m[3]);
        // a ≈ 1.0
        assert!((m[4] - 1.0).abs() < 1e-4, "a2 = {}", m[4]);
        // ty ≈ 0.0
        assert!(m[5].abs() < 1e-3, "ty = {}", m[5]);
    }

    #[test]
    fn test_scaled_transform() {
        // Source landmarks at 2x scale → transform should have a ≈ 0.5
        let src: [(f32, f32); 5] = [
            (76.5892, 103.3926),
            (147.0636, 103.0028),
            (112.0504, 143.4732),
            (83.0986, 184.7310),
            (141.4598, 184.4082),
        ];
        let m = estimate_similarity_transform(&src, &REFERENCE_LANDMARKS_112);

        // Scale factor should be ~0.5
        assert!((m[0] - 0.5).abs() < 0.05, "a = {}, expected ~0.5", m[0]);
    }

    #[test]
    fn test_warp_output_size() {
        let frame = vec![128u8; 640 * 480];
        let m = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]; // identity
        let out = warp_affine(&frame, 640, 480, &m, 112);
        assert_eq!(out.len(), 112 * 112);
    }

    #[test]
    fn test_align_face_output_size() {
        let frame = vec![128u8; 640 * 480];
        let landmarks = REFERENCE_LANDMARKS_112; // landmarks at reference positions
        let aligned = align_face(&frame, 640, 480, &landmarks);
        assert_eq!(aligned.len(), 112 * 112);
    }

    #[test]
    fn test_landmark_roundtrip() {
        // Place a bright patch at a landmark position, verify it lands near the
        // reference position after alignment.
        let w = 200usize;
        let h = 200usize;
        let mut frame = vec![0u8; w * h];

        let src_landmarks: [(f32, f32); 5] = [
            (80.0, 60.0),
            (120.0, 60.0),
            (100.0, 85.0),
            (85.0, 110.0),
            (115.0, 110.0),
        ];

        // Paint a 5x5 bright patch at the left eye position (survives bilinear interpolation)
        let lx = src_landmarks[0].0 as usize;
        let ly = src_landmarks[0].1 as usize;
        for dy in 0..5 {
            for dx in 0..5 {
                let px = lx.wrapping_sub(2) + dx;
                let py = ly.wrapping_sub(2) + dy;
                if px < w && py < h {
                    frame[py * w + px] = 255;
                }
            }
        }

        let aligned = align_face(&frame, w as u32, h as u32, &src_landmarks);

        // The reference left eye position is (38.29, 51.70).
        // Sample a small area around it and check for non-zero brightness.
        let ref_x = REFERENCE_LANDMARKS_112[0].0.round() as usize;
        let ref_y = REFERENCE_LANDMARKS_112[0].1.round() as usize;

        let mut max_val = 0u8;
        for dy in 0..3 {
            for dx in 0..3 {
                let x = ref_x.wrapping_sub(1) + dx;
                let y = ref_y.wrapping_sub(1) + dy;
                if x < 112 && y < 112 {
                    max_val = max_val.max(aligned[y * 112 + x]);
                }
            }
        }
        assert!(max_val > 100, "Expected bright patch near reference left eye ({ref_x}, {ref_y}), max={max_val}");
    }
}
