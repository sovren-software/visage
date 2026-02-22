# Visage Models

ONNX models required for face detection and recognition. These files are not
included in the repository — download them manually.

## Required Models

### SCRFD Face Detector (det_10g.onnx)

- **Source:** insightface/detection/scrfd
- **Download:** https://github.com/deepinsight/insightface/releases
- **File:** `det_10g.onnx` (16.1 MB)
- **Input:** [1, 3, 640, 640] float32 (NCHW, normalized)
- **Output:** 9 tensors (3 strides × scores/bboxes/landmarks)

### ArcFace Recognizer (w600k_r50.onnx)

- **Source:** insightface/recognition/arcface
- **Download:** https://github.com/deepinsight/insightface/releases
- **File:** `w600k_r50.onnx` (166 MB)
- **Input:** [1, 3, 112, 112] float32 (NCHW, normalized)
- **Output:** [1, 512] float32 embedding

## Setup

```bash
# Create model directory
mkdir -p ~/.local/share/visage/models/

# Place downloaded models
cp det_10g.onnx ~/.local/share/visage/models/
cp w600k_r50.onnx ~/.local/share/visage/models/
```

Visage will look for models in `$XDG_DATA_HOME/visage/models/` (defaults to
`~/.local/share/visage/models/`). Model paths can also be specified explicitly
via CLI arguments or daemon configuration.

## Checksums

Verify downloads:

```bash
sha256sum ~/.local/share/visage/models/det_10g.onnx
sha256sum ~/.local/share/visage/models/w600k_r50.onnx
```
