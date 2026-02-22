//! SCRFD face detector via ONNX Runtime.
//!
//! Implements the SCRFD (Sample and Computation Redistribution for Efficient Face
//! Detection) model with 3-stride anchor-free decoding and NMS post-processing.

use crate::types::BoundingBox;
use ndarray::Array4;
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;
use thiserror::Error;

// --- Named constants (no magic numbers) ---
const SCRFD_INPUT_SIZE: usize = 640;
const SCRFD_MEAN: f32 = 127.5;
const SCRFD_STD: f32 = 128.0;
const SCRFD_CONFIDENCE_THRESHOLD: f32 = 0.5;
const SCRFD_NMS_THRESHOLD: f32 = 0.4;
const SCRFD_STRIDES: [usize; 3] = [8, 16, 32];
const SCRFD_ANCHORS_PER_CELL: usize = 2;

#[derive(Error, Debug)]
pub enum DetectorError {
    #[error("model file not found: {0} — download from insightface and place in models/")]
    ModelNotFound(String),
    #[error("inference failed: {0}")]
    InferenceFailed(String),
    #[error("no face detected")]
    NoFaceDetected,
    #[error("ort: {0}")]
    Ort(#[from] ort::Error),
}

/// Metadata for coordinate de-mapping after letterbox resize.
struct LetterboxInfo {
    scale: f32,
    pad_x: f32,
    pad_y: f32,
}

/// Output tensor indices for one stride: (score_idx, bbox_idx, kps_idx).
type StrideOutputIndices = (usize, usize, usize);

/// SCRFD-based face detector.
pub struct FaceDetector {
    session: Session,
    input_height: usize,
    input_width: usize,
    /// Per-stride output indices [(score, bbox, kps)] for strides [8, 16, 32].
    /// Discovered by name at load time; falls back to positional ordering.
    stride_indices: [StrideOutputIndices; 3],
}

impl FaceDetector {
    /// Load the SCRFD ONNX model from the given path.
    pub fn load(model_path: &str) -> Result<Self, DetectorError> {
        if !Path::new(model_path).exists() {
            return Err(DetectorError::ModelNotFound(model_path.to_string()));
        }

        let session = Session::builder()?
            .with_intra_threads(2)?
            .commit_from_file(model_path)?;

        let output_names: Vec<String> = session.outputs().iter().map(|o| o.name().to_string()).collect();
        let num_outputs = output_names.len();

        tracing::info!(
            path = model_path,
            inputs = ?session.inputs().iter().map(|i| (i.name(), i.dtype())).collect::<Vec<_>>(),
            outputs = ?output_names,
            "loaded SCRFD model"
        );

        if num_outputs < 9 {
            return Err(DetectorError::InferenceFailed(format!(
                "SCRFD model requires 9 outputs (3 strides × score/bbox/kps), got {num_outputs}"
            )));
        }

        // Discover output ordering by name. SCRFD exports may name tensors as:
        //   "score_8", "score_16", "score_32" / "bbox_8", "bbox_16", "bbox_32" / "kps_8", ...
        // or as generic integers ("428", "429", ...).
        // Fall back to standard positional ordering when names are not recognized.
        let stride_indices = discover_output_indices(&output_names);
        tracing::debug!(?stride_indices, "SCRFD output tensor mapping");

        Ok(Self {
            session,
            input_height: SCRFD_INPUT_SIZE,
            input_width: SCRFD_INPUT_SIZE,
            stride_indices,
        })
    }

    /// Detect faces in a grayscale frame, returning bounding boxes sorted by confidence.
    pub fn detect(
        &mut self,
        frame: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Vec<BoundingBox>, DetectorError> {
        let (input, letterbox) = self.preprocess(frame, width as usize, height as usize);

        let outputs = self.session.run(ort::inputs![TensorRef::from_array_view(input.view())?])?;

        let mut all_detections = Vec::new();

        for (stride_pos, &stride) in SCRFD_STRIDES.iter().enumerate() {
            let (score_idx, bbox_idx, kps_idx) = self.stride_indices[stride_pos];

            let (_, scores) = outputs[score_idx]
                .try_extract_tensor::<f32>()
                .map_err(|e| DetectorError::InferenceFailed(format!("scores stride {stride}: {e}")))?;
            let (_, bboxes) = outputs[bbox_idx]
                .try_extract_tensor::<f32>()
                .map_err(|e| DetectorError::InferenceFailed(format!("bboxes stride {stride}: {e}")))?;
            let (_, kps) = outputs[kps_idx]
                .try_extract_tensor::<f32>()
                .map_err(|e| DetectorError::InferenceFailed(format!("kps stride {stride}: {e}")))?;

            let dets = decode_stride(
                scores,
                bboxes,
                kps,
                stride,
                self.input_width,
                self.input_height,
                &letterbox,
                SCRFD_CONFIDENCE_THRESHOLD,
            );
            all_detections.extend(dets);
        }

        let mut result = nms(all_detections, SCRFD_NMS_THRESHOLD);
        result.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(result)
    }

    /// Preprocess a grayscale frame into a NCHW float tensor with letterbox padding.
    ///
    /// Resizes using bilinear interpolation to preserve edge sharpness, then
    /// normalizes to the SCRFD input distribution.
    fn preprocess(
        &self,
        frame: &[u8],
        width: usize,
        height: usize,
    ) -> (Array4<f32>, LetterboxInfo) {
        // Compute letterbox scale (fit within input_width × input_height)
        let scale_w = self.input_width as f32 / width as f32;
        let scale_h = self.input_height as f32 / height as f32;
        let scale = scale_w.min(scale_h);

        let new_w = (width as f32 * scale).round() as usize;
        let new_h = (height as f32 * scale).round() as usize;
        let pad_x = (self.input_width - new_w) as f32 / 2.0;
        let pad_y = (self.input_height - new_h) as f32 / 2.0;

        let letterbox = LetterboxInfo { scale, pad_x, pad_y };

        // Resize grayscale using bilinear interpolation for sub-pixel accuracy.
        let inv_scale = 1.0 / scale;
        let mut resized = vec![0u8; new_w * new_h];
        for y in 0..new_h {
            let src_y = (y as f32 + 0.5) * inv_scale - 0.5;
            let y0 = (src_y.floor() as i32).clamp(0, height as i32 - 1) as usize;
            let y1 = (y0 + 1).min(height - 1);
            let fy = (src_y - src_y.floor()).clamp(0.0, 1.0);

            for x in 0..new_w {
                let src_x = (x as f32 + 0.5) * inv_scale - 0.5;
                let x0 = (src_x.floor() as i32).clamp(0, width as i32 - 1) as usize;
                let x1 = (x0 + 1).min(width - 1);
                let fx = (src_x - src_x.floor()).clamp(0.0, 1.0);

                let tl = frame[y0 * width + x0] as f32;
                let tr = frame[y0 * width + x1] as f32;
                let bl = frame[y1 * width + x0] as f32;
                let br = frame[y1 * width + x1] as f32;

                let val = tl * (1.0 - fx) * (1.0 - fy)
                    + tr * fx * (1.0 - fy)
                    + bl * (1.0 - fx) * fy
                    + br * fx * fy;

                resized[y * new_w + x] = val.round().clamp(0.0, 255.0) as u8;
            }
        }

        // Create NCHW tensor with letterbox padding (pad with SCRFD_MEAN → normalizes to 0.0)
        let pad_x_start = pad_x.floor() as usize;
        let pad_y_start = pad_y.floor() as usize;

        let mut tensor = Array4::<f32>::zeros((1, 3, self.input_height, self.input_width));

        for y in 0..self.input_height {
            for x in 0..self.input_width {
                let pixel = if y >= pad_y_start
                    && y < pad_y_start + new_h
                    && x >= pad_x_start
                    && x < pad_x_start + new_w
                {
                    resized[(y - pad_y_start) * new_w + (x - pad_x_start)] as f32
                } else {
                    SCRFD_MEAN // pad value normalizes to 0.0
                };

                let normalized = (pixel - SCRFD_MEAN) / SCRFD_STD;
                // Grayscale → 3-channel: replicate Y → [R=Y, G=Y, B=Y]
                tensor[[0, 0, y, x]] = normalized;
                tensor[[0, 1, y, x]] = normalized;
                tensor[[0, 2, y, x]] = normalized;
            }
        }

        (tensor, letterbox)
    }
}

/// Discover output tensor ordering by name.
///
/// SCRFD models may export tensors with named outputs ("score_8", "bbox_16", ...) or
/// generic numeric names. If named pattern is detected, maps them to stride slots.
/// Otherwise falls back to the standard positional ordering:
///   [0-2] = scores (strides 8, 16, 32)
///   [3-5] = bboxes (strides 8, 16, 32)
///   [6-8] = kps    (strides 8, 16, 32)
fn discover_output_indices(names: &[String]) -> [StrideOutputIndices; 3] {
    // Try name-based discovery: look for "score_8", "bbox_8", "kps_8" patterns.
    let find = |prefix: &str, stride: usize| -> Option<usize> {
        let target = format!("{prefix}_{stride}");
        names.iter().position(|n| n == &target)
    };

    let named = SCRFD_STRIDES.iter().all(|&stride| {
        find("score", stride).is_some()
            && find("bbox", stride).is_some()
            && find("kps", stride).is_some()
    });

    if named {
        tracing::info!("SCRFD: using name-based output tensor mapping");
        std::array::from_fn(|i| {
            let stride = SCRFD_STRIDES[i];
            (
                find("score", stride).unwrap(),
                find("bbox", stride).unwrap(),
                find("kps", stride).unwrap(),
            )
        })
    } else {
        // Positional fallback: [scores 8/16/32, bboxes 8/16/32, kps 8/16/32]
        tracing::info!(
            ?names,
            "SCRFD: output names not recognized, using positional mapping [0-2]=scores, [3-5]=bboxes, [6-8]=kps"
        );
        [(0, 3, 6), (1, 4, 7), (2, 5, 8)]
    }
}

/// Decode detections for a single stride level.
#[allow(clippy::too_many_arguments)]
fn decode_stride(
    scores: &[f32],
    bboxes: &[f32],
    kps: &[f32],
    stride: usize,
    input_width: usize,
    input_height: usize,
    letterbox: &LetterboxInfo,
    threshold: f32,
) -> Vec<BoundingBox> {
    let grid_h = input_height / stride;
    let grid_w = input_width / stride;
    let num_anchors = grid_h * grid_w * SCRFD_ANCHORS_PER_CELL;

    let mut detections = Vec::new();

    for idx in 0..num_anchors {
        let score = scores.get(idx).copied().unwrap_or(0.0);
        if score <= threshold {
            continue;
        }

        let anchor_idx = idx / SCRFD_ANCHORS_PER_CELL;
        let cy = (anchor_idx / grid_w) as f32;
        let cx = (anchor_idx % grid_w) as f32;

        let anchor_cx = cx * stride as f32;
        let anchor_cy = cy * stride as f32;

        // Decode bbox: [x1_offset, y1_offset, x2_offset, y2_offset] * stride
        let bbox_off = idx * 4;
        if bbox_off + 3 >= bboxes.len() {
            continue;
        }
        let x1 = anchor_cx - bboxes[bbox_off] * stride as f32;
        let y1 = anchor_cy - bboxes[bbox_off + 1] * stride as f32;
        let x2 = anchor_cx + bboxes[bbox_off + 2] * stride as f32;
        let y2 = anchor_cy + bboxes[bbox_off + 3] * stride as f32;

        // Map from letterboxed space to original frame space
        let orig_x1 = (x1 - letterbox.pad_x) / letterbox.scale;
        let orig_y1 = (y1 - letterbox.pad_y) / letterbox.scale;
        let orig_x2 = (x2 - letterbox.pad_x) / letterbox.scale;
        let orig_y2 = (y2 - letterbox.pad_y) / letterbox.scale;

        // Decode landmarks
        let kps_off = idx * 10;
        let landmarks = if kps_off + 9 < kps.len() {
            let mut lms = [(0.0f32, 0.0f32); 5];
            for i in 0..5 {
                let lx = anchor_cx + kps[kps_off + i * 2] * stride as f32;
                let ly = anchor_cy + kps[kps_off + i * 2 + 1] * stride as f32;
                lms[i] = (
                    (lx - letterbox.pad_x) / letterbox.scale,
                    (ly - letterbox.pad_y) / letterbox.scale,
                );
            }
            Some(lms)
        } else {
            None
        };

        detections.push(BoundingBox {
            x: orig_x1,
            y: orig_y1,
            width: orig_x2 - orig_x1,
            height: orig_y2 - orig_y1,
            confidence: score,
            landmarks,
        });
    }

    detections
}

/// Non-Maximum Suppression: remove overlapping detections.
fn nms(mut detections: Vec<BoundingBox>, iou_threshold: f32) -> Vec<BoundingBox> {
    detections.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut keep = Vec::new();
    let mut suppressed = vec![false; detections.len()];

    for i in 0..detections.len() {
        if suppressed[i] {
            continue;
        }
        keep.push(detections[i].clone());

        for j in (i + 1)..detections.len() {
            if suppressed[j] {
                continue;
            }
            if iou(&detections[i], &detections[j]) > iou_threshold {
                suppressed[j] = true;
            }
        }
    }

    keep
}

/// Compute Intersection-over-Union between two bounding boxes.
fn iou(a: &BoundingBox, b: &BoundingBox) -> f32 {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.width).min(b.x + b.width);
    let y2 = (a.y + a.height).min(b.y + b.height);

    let inter_w = (x2 - x1).max(0.0);
    let inter_h = (y2 - y1).max(0.0);
    let inter_area = inter_w * inter_h;

    let area_a = a.width * a.height;
    let area_b = b.width * b.height;
    let union_area = area_a + area_b - inter_area;

    if union_area > 0.0 {
        inter_area / union_area
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bbox(x: f32, y: f32, w: f32, h: f32, conf: f32) -> BoundingBox {
        BoundingBox {
            x, y, width: w, height: h, confidence: conf, landmarks: None,
        }
    }

    #[test]
    fn test_iou_identical() {
        let a = make_bbox(0.0, 0.0, 100.0, 100.0, 1.0);
        assert!((iou(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_iou_no_overlap() {
        let a = make_bbox(0.0, 0.0, 10.0, 10.0, 1.0);
        let b = make_bbox(20.0, 20.0, 10.0, 10.0, 1.0);
        assert!(iou(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn test_iou_partial() {
        let a = make_bbox(0.0, 0.0, 10.0, 10.0, 1.0);
        let b = make_bbox(5.0, 0.0, 10.0, 10.0, 1.0);
        // Overlap: 5x10 = 50, union: 100+100-50 = 150
        let expected = 50.0 / 150.0;
        assert!((iou(&a, &b) - expected).abs() < 1e-6);
    }

    #[test]
    fn test_nms_suppresses_overlapping() {
        let detections = vec![
            make_bbox(0.0, 0.0, 100.0, 100.0, 0.9),
            make_bbox(5.0, 5.0, 100.0, 100.0, 0.8),
            make_bbox(200.0, 200.0, 50.0, 50.0, 0.7),
        ];
        let result = nms(detections, 0.4);
        assert_eq!(result.len(), 2);
        assert!((result[0].confidence - 0.9).abs() < 1e-6);
        assert!((result[1].confidence - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_nms_no_suppression() {
        let detections = vec![
            make_bbox(0.0, 0.0, 10.0, 10.0, 0.9),
            make_bbox(50.0, 50.0, 10.0, 10.0, 0.8),
        ];
        let result = nms(detections, 0.4);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_nms_empty() {
        let result = nms(vec![], 0.4);
        assert!(result.is_empty());
    }

    #[test]
    fn test_letterbox_coordinate_roundtrip() {
        let width = 320.0f32;
        let height = 240.0f32;
        let scale_w = 640.0 / width;
        let scale_h = 640.0 / height;
        let scale = scale_w.min(scale_h);
        let new_w = (width * scale).round();
        let new_h = (height * scale).round();
        let pad_x = (640.0 - new_w) / 2.0;
        let pad_y = (640.0 - new_h) / 2.0;

        let letterbox = LetterboxInfo { scale, pad_x, pad_y };

        let orig_x = 100.0f32;
        let orig_y = 50.0f32;
        let letterboxed_x = orig_x * scale + pad_x;
        let letterboxed_y = orig_y * scale + pad_y;

        let recovered_x = (letterboxed_x - letterbox.pad_x) / letterbox.scale;
        let recovered_y = (letterboxed_y - letterbox.pad_y) / letterbox.scale;

        assert!((recovered_x - orig_x).abs() < 0.1, "x: {recovered_x} vs {orig_x}");
        assert!((recovered_y - orig_y).abs() < 0.1, "y: {recovered_y} vs {orig_y}");
    }

    #[test]
    fn test_discover_output_indices_named() {
        let names: Vec<String> = [
            "score_8", "score_16", "score_32",
            "bbox_8",  "bbox_16",  "bbox_32",
            "kps_8",   "kps_16",   "kps_32",
        ].iter().map(|s| s.to_string()).collect();

        let indices = discover_output_indices(&names);

        // stride 8: score at 0, bbox at 3, kps at 6
        assert_eq!(indices[0], (0, 3, 6));
        // stride 16: score at 1, bbox at 4, kps at 7
        assert_eq!(indices[1], (1, 4, 7));
        // stride 32: score at 2, bbox at 5, kps at 8
        assert_eq!(indices[2], (2, 5, 8));
    }

    #[test]
    fn test_discover_output_indices_shuffled_named() {
        // Named but in non-standard order
        let names: Vec<String> = [
            "bbox_8", "kps_8", "score_8",
            "bbox_16", "kps_16", "score_16",
            "bbox_32", "kps_32", "score_32",
        ].iter().map(|s| s.to_string()).collect();

        let indices = discover_output_indices(&names);

        // score_8 at index 2, bbox_8 at 0, kps_8 at 1
        assert_eq!(indices[0], (2, 0, 1));
        assert_eq!(indices[1], (5, 3, 4));
        assert_eq!(indices[2], (8, 6, 7));
    }

    #[test]
    fn test_discover_output_indices_positional_fallback() {
        // Generic numeric names — should fall back to positional
        let names: Vec<String> = (0..9).map(|i: usize| i.to_string()).collect();
        let indices = discover_output_indices(&names);
        assert_eq!(indices, [(0, 3, 6), (1, 4, 7), (2, 5, 8)]);
    }

    #[test]
    fn test_bilinear_resize_uniform() {
        // Uniform frame resized should remain uniform
        let w = 100usize;
        let h = 100usize;
        let frame = vec![128u8; w * h];

        // Simulate the bilinear resize portion of preprocess
        let new_w = 200usize;
        let new_h = 200usize;
        let inv_scale = 0.5f32;

        let mut resized = vec![0u8; new_w * new_h];
        for y in 0..new_h {
            let src_y = (y as f32 + 0.5) * inv_scale - 0.5;
            let y0 = (src_y.floor() as i32).clamp(0, h as i32 - 1) as usize;
            let y1 = (y0 + 1).min(h - 1);
            let fy = (src_y - src_y.floor()).clamp(0.0, 1.0);
            for x in 0..new_w {
                let src_x = (x as f32 + 0.5) * inv_scale - 0.5;
                let x0 = (src_x.floor() as i32).clamp(0, w as i32 - 1) as usize;
                let x1 = (x0 + 1).min(w - 1);
                let fx = (src_x - src_x.floor()).clamp(0.0, 1.0);
                let tl = frame[y0 * w + x0] as f32;
                let tr = frame[y0 * w + x1] as f32;
                let bl = frame[y1 * w + x0] as f32;
                let br = frame[y1 * w + x1] as f32;
                let val = tl * (1.0 - fx) * (1.0 - fy)
                    + tr * fx * (1.0 - fy)
                    + bl * (1.0 - fx) * fy
                    + br * fx * fy;
                resized[y * new_w + x] = val.round() as u8;
            }
        }

        // All pixels should be 128 (uniform input stays uniform)
        assert!(resized.iter().all(|&p| p == 128), "uniform resize should stay uniform");
    }
}
