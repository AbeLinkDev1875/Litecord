# Noise suppression

Files from [@sapphi-red/web-noise-suppressor](https://github.com/sapphi-red/web-noise-suppressor)
version 0.4.1 (MIT license, see LICENSE). They run open-source AI noise suppression
models in the browser's audio engine:

- `rnnoise*.wasm` + `rnnoise-worklet.js`: RNNoise (Xiph.Org / Mozilla, BSD license)
- `gtcrn.wasm` + `gtcrn-worklet.js`: GTCRN, a newer and stronger speech enhancement model

Only the source-map comment lines were removed from the worklet files.
