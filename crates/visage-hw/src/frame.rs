//! Frame type and image processing — YUYV conversion, dark detection, CLAHE.

/// A captured grayscale camera frame.
#[derive(Clone)]
pub struct Frame {
    /// Grayscale pixel data (width * height bytes).
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp: std::time::Instant,
    pub sequence: u32,
    pub is_dark: bool,
}

impl Frame {
    /// Average pixel brightness (0.0–255.0).
    pub fn avg_brightness(&self) -> f32 {
        if self.data.is_empty() {
            return 0.0;
        }
        self.data.iter().map(|&b| b as f32).sum::<f32>() / self.data.len() as f32
    }
}

/// Convert packed YUYV (4:2:2) to grayscale by extracting the Y channel.
///
/// YUYV packs two pixels per 4 bytes: [Y0, U, Y1, V].
/// Grayscale = every even-indexed byte.
pub fn yuyv_to_grayscale(yuyv: &[u8], width: u32, height: u32) -> Result<Vec<u8>, FrameError> {
    let expected = (width * height * 2) as usize;
    if yuyv.len() < expected {
        return Err(FrameError::InvalidLength {
            expected,
            actual: yuyv.len(),
        });
    }
    Ok(yuyv[..expected].iter().step_by(2).copied().collect())
}

/// Check if a frame is dark using an 8-bucket histogram.
///
/// Returns true if >95% of pixels fall in the darkest bucket (0–31).
pub fn is_dark_frame(gray: &[u8], threshold_pct: f32) -> bool {
    if gray.is_empty() {
        return true;
    }
    let dark_count = gray.iter().filter(|&&p| p < 32).count();
    (dark_count as f32 / gray.len() as f32) > threshold_pct
}

/// Apply Contrast-Limited Adaptive Histogram Equalization (CLAHE) in-place.
///
/// Divides the image into a grid of tiles, computes a clipped histogram
/// per tile, builds CDFs, and uses bilinear interpolation between tile
/// CDFs for smooth output.
pub fn clahe_enhance(gray: &mut [u8], width: u32, height: u32, tiles_x: u32, clip_limit: f32) {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || gray.len() < w * h {
        return;
    }

    let tiles_y = tiles_x; // square grid
    let tx = tiles_x as usize;
    let ty = tiles_y as usize;
    let tile_w = w / tx;
    let tile_h = h / ty;
    if tile_w == 0 || tile_h == 0 {
        return;
    }
    let tile_pixels = tile_w * tile_h;

    // Build per-tile CDFs
    let mut cdfs: Vec<[f32; 256]> = Vec::with_capacity(tx * ty);

    for row in 0..ty {
        for col in 0..tx {
            let mut hist = [0u32; 256];
            let y0 = row * tile_h;
            let x0 = col * tile_w;

            for y in y0..y0 + tile_h {
                for x in x0..x0 + tile_w {
                    hist[gray[y * w + x] as usize] += 1;
                }
            }

            // Clip histogram
            let clip = (clip_limit * tile_pixels as f32) as u32;
            let mut excess = 0u32;
            for bin in hist.iter_mut() {
                if *bin > clip {
                    excess += *bin - clip;
                    *bin = clip;
                }
            }
            let redist = excess / 256;
            let leftover = (excess % 256) as usize;
            for (i, bin) in hist.iter_mut().enumerate() {
                *bin += redist;
                if i < leftover {
                    *bin += 1;
                }
            }

            // Build CDF
            let mut cdf = [0f32; 256];
            cdf[0] = hist[0] as f32;
            for i in 1..256 {
                cdf[i] = cdf[i - 1] + hist[i] as f32;
            }
            // Normalize to 0–255
            let cdf_min = cdf.iter().find(|&&v| v > 0.0).copied().unwrap_or(0.0);
            let denom = (tile_pixels as f32) - cdf_min;
            if denom > 0.0 {
                for v in cdf.iter_mut() {
                    *v = ((*v - cdf_min) / denom * 255.0).clamp(0.0, 255.0);
                }
            }
            cdfs.push(cdf);
        }
    }

    // Map each pixel using bilinear interpolation between tile CDFs
    for y in 0..h {
        for x in 0..w {
            let pixel = gray[y * w + x] as usize;

            // Which tile center is this pixel near?
            let fy = (y as f32 / tile_h as f32) - 0.5;
            let fx = (x as f32 / tile_w as f32) - 0.5;

            let fy = fy.clamp(0.0, (ty - 1) as f32);
            let fx = fx.clamp(0.0, (tx - 1) as f32);

            let r0 = fy as usize;
            let c0 = fx as usize;
            let r1 = (r0 + 1).min(ty - 1);
            let c1 = (c0 + 1).min(tx - 1);

            let dy = fy - r0 as f32;
            let dx = fx - c0 as f32;

            let tl = cdfs[r0 * tx + c0][pixel];
            let tr = cdfs[r0 * tx + c1][pixel];
            let bl = cdfs[r1 * tx + c0][pixel];
            let br = cdfs[r1 * tx + c1][pixel];

            let top = tl * (1.0 - dx) + tr * dx;
            let bot = bl * (1.0 - dx) + br * dx;
            let val = top * (1.0 - dy) + bot * dy;

            gray[y * w + x] = val.round().clamp(0.0, 255.0) as u8;
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("invalid YUYV length: expected {expected}, got {actual}")]
    InvalidLength { expected: usize, actual: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_yuyv_to_grayscale() {
        // 2x1 image: [Y0=100, U=128, Y1=200, V=128]
        let yuyv = vec![100, 128, 200, 128];
        let gray = yuyv_to_grayscale(&yuyv, 2, 1).unwrap();
        assert_eq!(gray, vec![100, 200]);
    }

    #[test]
    fn test_yuyv_to_grayscale_4x2() {
        // 4x2 image = 8 pixels, 16 YUYV bytes
        let yuyv: Vec<u8> = (0..16).collect();
        let gray = yuyv_to_grayscale(&yuyv, 4, 2).unwrap();
        assert_eq!(gray.len(), 8);
        // Even indices: 0, 2, 4, 6, 8, 10, 12, 14
        assert_eq!(gray, vec![0, 2, 4, 6, 8, 10, 12, 14]);
    }

    #[test]
    fn test_yuyv_invalid_length() {
        let yuyv = vec![100, 128]; // too short for 2x1
        let result = yuyv_to_grayscale(&yuyv, 2, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_dark_frame_all_black() {
        let gray = vec![0u8; 1000];
        assert!(is_dark_frame(&gray, 0.95));
    }

    #[test]
    fn test_dark_frame_normal() {
        let gray = vec![128u8; 1000];
        assert!(!is_dark_frame(&gray, 0.95));
    }

    #[test]
    fn test_dark_frame_empty() {
        assert!(is_dark_frame(&[], 0.95));
    }

    #[test]
    fn test_dark_frame_mostly_dark() {
        // 96% dark, 4% bright → should be dark
        let mut gray = vec![10u8; 960];
        gray.extend(vec![128u8; 40]);
        assert!(is_dark_frame(&gray, 0.95));
    }

    #[test]
    fn test_dark_frame_borderline_bright() {
        // 94% dark, 6% bright → should NOT be dark
        let mut gray = vec![10u8; 940];
        gray.extend(vec![128u8; 60]);
        assert!(!is_dark_frame(&gray, 0.95));
    }

    #[test]
    fn test_clahe_increases_contrast() {
        // Low-contrast 16x16 image: all pixels between 100–110
        let w = 16u32;
        let h = 16u32;
        let mut gray: Vec<u8> = (0..(w * h) as usize)
            .map(|i| 100 + (i % 11) as u8)
            .collect();

        let orig_stddev = stddev(&gray);
        clahe_enhance(&mut gray, w, h, 2, 0.02);
        let new_stddev = stddev(&gray);

        // CLAHE should increase contrast (stddev should grow)
        assert!(
            new_stddev > orig_stddev,
            "CLAHE should increase contrast: orig={orig_stddev:.2}, new={new_stddev:.2}"
        );
    }

    fn stddev(data: &[u8]) -> f32 {
        let n = data.len() as f32;
        let mean = data.iter().map(|&b| b as f32).sum::<f32>() / n;
        let variance = data.iter().map(|&b| (b as f32 - mean).powi(2)).sum::<f32>() / n;
        variance.sqrt()
    }
}
