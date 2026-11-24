# Optional H3 engine source changes

Struve's H3 RAM v7 integration uses a separately built stable-diffusion.cpp engine. These patches cover chunked linear operations, CPU memory caching, staging inputs, attention output and tiled residuals. They are optional for building the desktop and for chatting with a GGUF model.

Use stable-diffusion.cpp revision `532d2c15c430a785a712b2d94281f04e733008a4`. The upstream source archive hashes and dependency revisions are recorded in [source-receipt.json](source-receipt.json). From the matching engine source checkout, set the path to this patch directory and apply the files in order:

```powershell
$patchDirectory = Read-Host "Path to engines/h3-ram-v7"
foreach ($name in 'struve-linear-chunk.patch','struve-host-cache.patch','struve-stage-inputs.patch','struve-attention-output.patch','struve-h3-residual-tiles-v7.patch','struve-h3-memory-docs.patch') {
    git apply --whitespace=nowarn (Join-Path $patchDirectory $name)
    if ($LASTEXITCODE) { throw "Could not apply $name" }
}
```

Then follow the pinned upstream build instructions for your backend and select its executable in the media profile. GPU libraries and H3 model weights are separate. The patch sequence was reapplied to the baseline for this source release, and the resulting C++ files matched the locally qualified v7 source. Hardware and media quality still require testing on each configuration.

[Pinned engine source and build guide](https://github.com/leejet/stable-diffusion.cpp/tree/532d2c15c430a785a712b2d94281f04e733008a4) · [Engine license](stable-diffusion.cpp.txt) · [ggml license](ggml.txt) · [CUTLASS license](cutlass.txt)
