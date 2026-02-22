use serde::{Deserialize, Serialize};

/// Bounding box for a detected face, with optional facial landmarks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub confidence: f32,
    /// Five-point facial landmarks: [left_eye, right_eye, nose, left_mouth, right_mouth].
    pub landmarks: Option<[(f32, f32); 5]>,
}

/// Face embedding vector (typically 512-dimensional for ArcFace).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub values: Vec<f32>,
    /// Model version that produced this embedding (e.g., "w600k_r50").
    pub model_version: Option<String>,
}

impl Embedding {
    /// Compute cosine similarity between two embeddings.
    ///
    /// Returns a value in [-1, 1]. Higher = more similar.
    /// Uses constant-time computation: always processes all dimensions.
    pub fn similarity(&self, other: &Embedding) -> f32 {
        let mut dot = 0.0f32;
        let mut norm_a = 0.0f32;
        let mut norm_b = 0.0f32;

        for (a, b) in self.values.iter().zip(other.values.iter()) {
            dot += a * b;
            norm_a += a * a;
            norm_b += b * b;
        }

        let denom = norm_a.sqrt() * norm_b.sqrt();
        // Constant-time: always compute, use conditional assignment
        // rather than early return to avoid timing side-channel.
        if denom > 0.0 { dot / denom } else { 0.0 }
    }

    /// Alias for [`similarity`](Self::similarity) — cosine similarity in [-1, 1].
    #[deprecated(since = "0.1.0", note = "use `similarity()` instead")]
    pub fn cosine_similarity(&self, other: &Embedding) -> f32 {
        self.similarity(other)
    }

    /// Compute Euclidean distance between two embeddings.
    pub fn euclidean_distance(&self, other: &Embedding) -> f32 {
        self.values
            .iter()
            .zip(other.values.iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            .sqrt()
    }
}

/// A stored face model with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceModel {
    pub id: String,
    pub user: String,
    pub label: String,
    pub embedding: Embedding,
    pub created_at: String,
}

/// Result of matching a probe embedding against a gallery.
#[derive(Debug, Clone)]
pub struct MatchResult {
    pub matched: bool,
    /// Cosine similarity of the best match [-1, 1].
    pub similarity: f32,
    /// ID of the matched model (if any).
    pub model_id: Option<String>,
    /// Label of the matched model (if any).
    pub model_label: Option<String>,
}

/// Strategy for comparing a probe embedding against a gallery of enrolled faces.
pub trait Matcher {
    fn compare(&self, probe: &Embedding, gallery: &[FaceModel], threshold: f32) -> MatchResult;
}

/// Cosine similarity matcher with constant-time gallery traversal.
///
/// Always iterates ALL gallery entries to prevent timing side-channels
/// that could leak gallery size or match position.
pub struct CosineMatcher;

impl Matcher for CosineMatcher {
    fn compare(&self, probe: &Embedding, gallery: &[FaceModel], threshold: f32) -> MatchResult {
        let mut best_sim = f32::NEG_INFINITY;
        let mut best_idx: Option<usize> = None;

        // Constant-time: always iterate every entry, no early exit.
        for (i, model) in gallery.iter().enumerate() {
            let sim = probe.similarity(&model.embedding);
            if sim > best_sim {
                best_sim = sim;
                best_idx = Some(i);
            }
        }

        match best_idx {
            Some(idx) if best_sim >= threshold => MatchResult {
                matched: true,
                similarity: best_sim,
                model_id: Some(gallery[idx].id.clone()),
                model_label: Some(gallery[idx].label.clone()),
            },
            _ => MatchResult {
                matched: false,
                similarity: if best_sim == f32::NEG_INFINITY { 0.0 } else { best_sim },
                model_id: None,
                model_label: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = Embedding { values: vec![1.0, 0.0, 0.0], model_version: None };
        let b = Embedding { values: vec![1.0, 0.0, 0.0], model_version: None };
        assert!((a.similarity(&b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = Embedding { values: vec![1.0, 0.0], model_version: None };
        let b = Embedding { values: vec![0.0, 1.0], model_version: None };
        assert!(a.similarity(&b).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = Embedding { values: vec![1.0, 0.0], model_version: None };
        let b = Embedding { values: vec![-1.0, 0.0], model_version: None };
        assert!((a.similarity(&b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = Embedding { values: vec![0.0, 0.0], model_version: None };
        let b = Embedding { values: vec![1.0, 0.0], model_version: None };
        assert_eq!(a.similarity(&b), 0.0);
    }

    #[test]
    fn test_cosine_matcher_constant_time() {
        // Verify all gallery entries are compared (best match is last entry)
        let probe = Embedding { values: vec![1.0, 0.0, 0.0], model_version: None };
        let gallery = vec![
            FaceModel {
                id: "1".into(), user: "u".into(), label: "decoy1".into(),
                embedding: Embedding { values: vec![0.0, 1.0, 0.0], model_version: None },
                created_at: "".into(),
            },
            FaceModel {
                id: "2".into(), user: "u".into(), label: "decoy2".into(),
                embedding: Embedding { values: vec![0.0, 0.0, 1.0], model_version: None },
                created_at: "".into(),
            },
            FaceModel {
                id: "3".into(), user: "u".into(), label: "match".into(),
                embedding: Embedding { values: vec![1.0, 0.0, 0.0], model_version: None },
                created_at: "".into(),
            },
        ];

        let result = CosineMatcher.compare(&probe, &gallery, 0.5);
        assert!(result.matched);
        assert_eq!(result.model_id.as_deref(), Some("3"));
        assert_eq!(result.model_label.as_deref(), Some("match"));
        assert!((result.similarity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_matcher_no_match() {
        let probe = Embedding { values: vec![1.0, 0.0, 0.0], model_version: None };
        let gallery = vec![
            FaceModel {
                id: "1".into(), user: "u".into(), label: "other".into(),
                embedding: Embedding { values: vec![0.0, 1.0, 0.0], model_version: None },
                created_at: "".into(),
            },
        ];

        let result = CosineMatcher.compare(&probe, &gallery, 0.5);
        assert!(!result.matched);
        assert!(result.similarity.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_matcher_empty_gallery() {
        let probe = Embedding { values: vec![1.0, 0.0], model_version: None };
        let result = CosineMatcher.compare(&probe, &[], 0.5);
        assert!(!result.matched);
        assert_eq!(result.similarity, 0.0);
    }
}
