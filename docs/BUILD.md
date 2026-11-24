# Build and contribute

The Windows desktop uses Tauri, React and TypeScript. The Rust controller serves both the desktop and CLI. Node bridge workers implement tools and file handling. The model engines run as separate processes.

## Build from a source checkout

Install the Rust stable MSVC toolchain, Visual Studio C++ build tools with the Windows SDK, and Node.js 24 with npm. Open a Windows developer terminal in the source root:

```powershell
npm ci
npm run build
cargo build --locked --release --features desktop --bins
cargo test --locked
npm run test:mcp
```

The results are `target/release/struve-desktop.exe` and `target/release/struve-llm.exe` unless CARGO_TARGET_DIR is set. The executables alone do not include the Node bridge. To try a development build, install the alpha into a separate directory, exit from its tray, replace only the two executables under its `app` directory and copy any edited bridge files to `app/runtime/bridge`. Keep production chats separate.

When preparing public binaries, set the compiler's path remapping before the Cargo build. For example, in PowerShell:

```powershell
$env:CARGO_ENCODED_RUSTFLAGS = ("--remap-path-prefix=$env:USERPROFILE=/build/home", "--remap-path-prefix=$($PWD.Path)=/build/struve" -join [char]31)
```

## Rebuild the installer

Use Windows, Python 3.11 or later and NSIS 3.13. Start with a clean payload: the installed alpha before first run, excluding its uninstaller, hidden installer marker and app/installer-language.txt. Copy only app, bin, docs, engines/h3-ram-v7, examples, licenses, media-packs, models/images/slimsam, runtimes/image-edit-python, runtimes/llama.cpp, skills, README files, notices and lockfiles into a staging directory. Replace the two executables and bridge with your build. Retain the license files for all dependencies. The CPU engine is the upstream b12626 Windows x64 build. [llama.cpp release](https://github.com/ggml-org/llama.cpp/releases/tag/b12626)

Download the Evergreen Bootstrapper from Microsoft. Verify its Microsoft Authenticode signature with `Get-AuthenticodeSignature`. Then run:

```powershell
python packaging/windows/build_setup.py --payload .\payload --webview-bootstrapper .\tools\MicrosoftEdgeWebview2Setup.exe --makensis .\tools\nsis-3.13\makensis.exe --output .\out\Struve-LLM-0.8.2-alpha.1-windows-x64-setup.exe
```

The builder generates an uninstall list from the explicitly staged files. The default installer runs per user. Test install, first launch, reinstall and uninstall in a disposable directory before attaching the installer to a GitHub prerelease. [NSIS](https://nsis.sourceforge.io/Download) · [Microsoft WebView2](https://developer.microsoft.com/en-us/microsoft-edge/webview2/)

The source archive also contains the patch series for the optional H3 RAM v7 engine. [H3 source and application order](../engines/h3-ram-v7/README.md)

## Reporting an issue or proposing a change

Include reproducible steps, package tag, Windows build, GPU, engine and expected output. For scheduler issues include the wait reason, selected device, model size and context length. For visual changes include a screenshot. Keep private chats, credentials and model files out of issue attachments. Changes to CLI JSON should preserve existing callers; test desktop changes in both interface languages.
