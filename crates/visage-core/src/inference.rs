//! Model execution backends.
//!
//! ONNX Runtime on the CPU is the default and always available. A build with the
//! `openvino` feature can run both models on a GPU through OpenVINO instead,
//! selected at runtime with [`Device::Gpu`].
//!
//! The GPU path never makes authentication depend on the GPU: if OpenVINO is not
//! installed, finds no GPU, or fails to compile a model, loading falls back to
//! the CPU with a warning. A daemon that refused to start because a GPU driver
//! broke would lock face authentication out for a reason unrelated to faces.

use ndarray::Array4;
use std::path::PathBuf;
use thiserror::Error;

/// Where the models run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// ONNX Runtime on the CPU.
    Cpu,
    /// OpenVINO on the GPU. Needs a build with the `openvino` feature and the
    /// OpenVINO runtime with its GPU plugin installed; otherwise falls back to
    /// [`Device::Cpu`].
    Gpu,
}

impl std::str::FromStr for Device {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cpu" => Ok(Device::Cpu),
            "gpu" => Ok(Device::Gpu),
            other => Err(format!(
                "unknown inference device '{other}' (expected cpu or gpu)"
            )),
        }
    }
}

/// How models are loaded and run.
#[derive(Debug, Clone)]
pub struct InferenceOptions {
    pub device: Device,
    /// Intra-op threads for ONNX Runtime on the CPU.
    pub cpu_threads: usize,
    /// Where OpenVINO keeps compiled GPU kernels. Without it every daemon start
    /// recompiles both models, which takes seconds on an integrated GPU; with it
    /// a start after the first loads them in well under a second.
    pub cache_dir: Option<PathBuf>,
}

impl Default for InferenceOptions {
    fn default() -> Self {
        Self {
            device: Device::Cpu,
            cpu_threads: 2,
            cache_dir: None,
        }
    }
}

#[derive(Error, Debug)]
pub enum InferenceError {
    #[error("ort: {0}")]
    Ort(#[from] ort::Error),
    #[error("openvino: {0}")]
    OpenVino(String),
    #[error("input tensor is not contiguous")]
    NonContiguousInput,
    #[error("input shape {got:?} does not match the model's {expected:?}")]
    InputShape {
        got: Vec<usize>,
        expected: [usize; 4],
    },
}

/// A loaded model with a fixed NCHW input shape.
pub(crate) struct Model {
    backend: Backend,
    input_shape: [usize; 4],
    output_names: Vec<String>,
}

enum Backend {
    Ort(ort::session::Session),
    #[cfg(feature = "openvino")]
    OpenVino(openvino_backend::OpenVinoModel),
}

impl Model {
    /// Load `model_path` for inputs of `input_shape` on the requested device,
    /// falling back to the CPU when the GPU path is unavailable.
    pub(crate) fn load(
        model_path: &str,
        input_shape: [usize; 4],
        options: &InferenceOptions,
    ) -> Result<Self, InferenceError> {
        if options.device == Device::Gpu {
            #[cfg(feature = "openvino")]
            match openvino_backend::OpenVinoModel::load(
                model_path,
                input_shape,
                options.cache_dir.as_deref(),
            ) {
                Ok((model, output_names)) => {
                    return Ok(Self {
                        backend: Backend::OpenVino(model),
                        input_shape,
                        output_names,
                    })
                }
                Err(e) => tracing::warn!(
                    path = model_path,
                    error = %e,
                    "GPU inference unavailable, falling back to the CPU"
                ),
            }

            #[cfg(not(feature = "openvino"))]
            tracing::warn!(
                path = model_path,
                "GPU inference requested, but this build has no `openvino` feature; \
                 using the CPU"
            );
        }

        let session = ort::session::Session::builder()?
            .with_intra_threads(options.cpu_threads.max(1))?
            .commit_from_file(model_path)?;
        let output_names = session
            .outputs()
            .iter()
            .map(|o| o.name().to_string())
            .collect();
        Ok(Self {
            backend: Backend::Ort(session),
            input_shape,
            output_names,
        })
    }

    /// Where this model actually runs, for logs and `visage status`.
    pub(crate) fn device(&self) -> Device {
        match self.backend {
            Backend::Ort(_) => Device::Cpu,
            #[cfg(feature = "openvino")]
            Backend::OpenVino(_) => Device::Gpu,
        }
    }

    pub(crate) fn output_names(&self) -> &[String] {
        &self.output_names
    }

    /// Run the model and return every output, flattened, in output order.
    pub(crate) fn run(&mut self, input: &Array4<f32>) -> Result<Vec<Vec<f32>>, InferenceError> {
        // The GPU path is compiled for exactly this shape; hold the CPU path to
        // it too, so a preprocessing bug fails the same way on both.
        if input.shape() != self.input_shape {
            return Err(InferenceError::InputShape {
                got: input.shape().to_vec(),
                expected: self.input_shape,
            });
        }
        match &mut self.backend {
            Backend::Ort(session) => {
                let outputs = session.run(ort::inputs![ort::value::TensorRef::from_array_view(
                    input.view()
                )?])?;
                let mut flat = Vec::with_capacity(outputs.len());
                for value in outputs.values() {
                    let (_, data) = value.try_extract_tensor::<f32>()?;
                    flat.push(data.to_vec());
                }
                Ok(flat)
            }
            #[cfg(feature = "openvino")]
            Backend::OpenVino(model) => model.run(input),
        }
    }
}

#[cfg(feature = "openvino")]
mod openvino_backend {
    use super::InferenceError;
    use ndarray::Array4;
    use openvino::{
        CompiledModel, Core, DeviceType, ElementType, InferRequest, PartialShape, RwPropertyKey,
        Shape, Tensor,
    };
    use std::path::Path;

    fn ov_err(e: impl std::fmt::Display) -> InferenceError {
        InferenceError::OpenVino(e.to_string())
    }

    pub(super) struct OpenVinoModel {
        // Field order is drop order: the request before the compiled model it
        // came from, both before the core that owns the plugin.
        request: InferRequest,
        _compiled: CompiledModel,
        _core: Core,
        // Plain dimensions rather than an `openvino::Shape`, which holds a raw
        // pointer and so cannot move to the engine thread.
        input_dims: [i64; 4],
        output_count: usize,
    }

    impl OpenVinoModel {
        pub(super) fn load(
            model_path: &str,
            input_shape: [usize; 4],
            cache_dir: Option<&Path>,
        ) -> Result<(Self, Vec<String>), InferenceError> {
            // The runtime is loaded here, not linked at build time, so a build
            // with this feature still starts where OpenVINO is not installed.
            let mut core = Core::new().map_err(ov_err)?;
            let gpu = DeviceType::GPU;

            if let Some(dir) = cache_dir {
                match std::fs::create_dir_all(dir) {
                    Ok(()) => core
                        .set_property(&gpu, &RwPropertyKey::CacheDir, &dir.to_string_lossy())
                        .map_err(ov_err)?,
                    Err(e) => tracing::warn!(
                        dir = %dir.display(),
                        error = %e,
                        "cannot create the GPU kernel cache; every start will recompile"
                    ),
                }
            }
            // One request at a time, as fast as possible: the verify path waits
            // on each inference before starting the next.
            core.set_property(&gpu, &RwPropertyKey::HintPerformanceMode, "LATENCY")
                .map_err(ov_err)?;

            let mut model = core.read_model_from_file(model_path, "").map_err(ov_err)?;

            // The models are exported with dynamic spatial dimensions. A GPU
            // compiles far better kernels for a static shape, and the callers
            // only ever feed one shape per model.
            let dims = input_shape.map(|d| d as i64);
            let partial = PartialShape::new_static(4, &dims).map_err(ov_err)?;
            model.reshape_single_input(&partial).map_err(ov_err)?;

            let output_count = model.get_outputs_len().map_err(ov_err)?;
            let mut output_names = Vec::with_capacity(output_count);
            for i in 0..output_count {
                let node = model.get_output_by_index(i).map_err(ov_err)?;
                output_names.push(node.get_name().map_err(ov_err)?);
            }

            let mut compiled = core.compile_model(&model, gpu).map_err(ov_err)?;
            let request = compiled.create_infer_request().map_err(ov_err)?;

            Ok((
                Self {
                    request,
                    _compiled: compiled,
                    _core: core,
                    input_dims: dims,
                    output_count,
                },
                output_names,
            ))
        }

        pub(super) fn run(&mut self, input: &Array4<f32>) -> Result<Vec<Vec<f32>>, InferenceError> {
            let src = input.as_slice().ok_or(InferenceError::NonContiguousInput)?;
            let shape = Shape::new(&self.input_dims).map_err(ov_err)?;
            let mut tensor = Tensor::new(ElementType::F32, &shape).map_err(ov_err)?;
            let dst = tensor.get_data_mut::<f32>().map_err(ov_err)?;
            if dst.len() != src.len() {
                return Err(InferenceError::OpenVino(format!(
                    "input has {} values, the model expects {}",
                    src.len(),
                    dst.len()
                )));
            }
            dst.copy_from_slice(src);

            self.request.set_input_tensor(&tensor).map_err(ov_err)?;
            self.request.infer().map_err(ov_err)?;

            let mut flat = Vec::with_capacity(self.output_count);
            for i in 0..self.output_count {
                let out = self.request.get_output_tensor_by_index(i).map_err(ov_err)?;
                flat.push(out.get_data::<f32>().map_err(ov_err)?.to_vec());
            }
            Ok(flat)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_parses_case_insensitively_and_rejects_unknown_values() {
        assert_eq!("cpu".parse::<Device>(), Ok(Device::Cpu));
        assert_eq!("GPU".parse::<Device>(), Ok(Device::Gpu));
        assert_eq!(" gpu ".parse::<Device>(), Ok(Device::Gpu));
        assert!("npu".parse::<Device>().is_err());
        assert!("".parse::<Device>().is_err());
    }

    /// The default must be exactly what Visage did before backends existed:
    /// ONNX Runtime, CPU, two threads. A build or a config without any of the
    /// new knobs must not change behaviour.
    #[test]
    fn defaults_match_the_pre_backend_behaviour() {
        let o = InferenceOptions::default();
        assert_eq!(o.device, Device::Cpu);
        assert_eq!(o.cpu_threads, 2);
        assert!(o.cache_dir.is_none());
    }
}
