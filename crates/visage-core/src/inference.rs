//! How the face models are run.

/// How models are loaded and run.
#[derive(Debug, Clone)]
pub struct InferenceOptions {
    /// Intra-op threads for ONNX Runtime on the CPU.
    pub cpu_threads: usize,
}

impl Default for InferenceOptions {
    fn default() -> Self {
        Self { cpu_threads: 2 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default must be exactly what Visage did before this was
    /// configurable: two threads per model.
    #[test]
    fn defaults_match_the_previous_hard_coded_value() {
        assert_eq!(InferenceOptions::default().cpu_threads, 2);
    }
}
