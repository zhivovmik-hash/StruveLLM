# Third-party components

Struve LLM uses the MIT license declared in Cargo.toml. The included [license](LICENSE) retains the Strata copyright notice. The upstream project is [Strata](https://github.com/Niko1221/Strata).

The Rust dependency versions are recorded in [Cargo.lock](Cargo.lock). The desktop uses Tauri and the system WebView2 runtime. The JavaScript dependency versions and package license metadata are recorded in [package-lock.json](package-lock.json). The application uses React, Mermaid and highlight.js for the desktop interface, the Model Context Protocol SDK for integrations, and bundled JavaScript libraries for documents, PDF and spreadsheets. The included [H3 prompt lint pack](media-packs/h3-prompt-lint/LICENSE) carries its own license.

Runtime binaries such as Node.js, FFmpeg, Ollama, llama.cpp and stable-diffusion.cpp have their own distribution terms. Installed weights, adapters and downloaded custom nodes retain their publishers' licenses. A binary distribution must include the applicable notices and license texts for the specific binaries and packages it actually ships. The source license does not relicense model weights.

## Windows x64 alpha distribution

| Component | Bundled material and origin |
| --- | --- |
| Node.js 24.15.0 | Runtime and notices in `licenses/node-v24.15.0.txt`. [Upstream release](https://nodejs.org/dist/v24.15.0/) |
| Rust and npm dependencies | Package inventories in `licenses/rust-index.json` and `licenses/npm-index.json` with their available license texts. The lockfiles record source versions. |
| FFmpeg 6.1.1 essentials, Gyan Windows build | The executable, its GPLv3 license, build configuration and source reference are in `app/runtime/node_modules/ffmpeg-static/`. [FFmpeg source revision](https://github.com/FFmpeg/FFmpeg/commit/e38092ef93) · [Build distributor](https://www.gyan.dev/ffmpeg/builds/) |
| llama.cpp b12626, CPU x64 | Server and CPU backends in `runtimes/llama.cpp/`, with the upstream MIT and LLVM OpenMP license texts. Upstream archive SHA-256 is `c520c605af7b504651c581bd46c6e7e2bab1305c8c6e7f7f79372757e5b20472`. [Release and source](https://github.com/ggml-org/llama.cpp/releases/tag/b12626) |
| Embedded Python image editor | Python 3.14.0, NumPy, OpenCV, Pillow and ONNX Runtime with dependent packages. Versions and Python archive hash are in `runtimes/image-edit-python/installation.json`; the runtime and wheels carry their own licenses. |
| SlimSAM ONNX | The image editing helper includes the public segmentation weights, model card, license and download records in `models/images/slimsam/`. [Pinned model](https://huggingface.co/Xenova/slimsam-77-uniform/tree/5850ab45f587c112167512ffef949107115e26a0) |
| Microsoft Edge WebView2 | The installer carries Microsoft's signed Evergreen Bootstrapper for PCs missing the runtime. [Microsoft distribution information](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution) |
| Microsoft Visual C++ runtime | The CPU model server and Python image editor include the x64 app-local CRT from the Visual Studio Redistributable directory so they can start on a fresh PC. [Microsoft deployment information](https://learn.microsoft.com/en-us/cpp/windows/deployment-in-visual-cpp) · [Visual Studio license terms](https://visualstudio.microsoft.com/license-terms/) |
| NSIS | Windows installer engine. [License](https://nsis.sourceforge.io/License) |
