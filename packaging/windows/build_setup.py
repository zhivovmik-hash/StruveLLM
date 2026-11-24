"""Build the Windows installer from a clean distribution staging directory."""
import argparse
from pathlib import Path
import re
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--payload", type=Path, required=True)
    parser.add_argument("--webview-bootstrapper", type=Path, required=True)
    parser.add_argument("--makensis", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version", default="0.8.2-alpha.1")
    args = parser.parse_args()
    version = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)-alpha\.(\d+)", args.version)
    if not version or any(int(part) > 65535 for part in version.groups()):
        parser.error("Expected a Windows-compatible X.Y.Z-alpha.N version")
    root = args.payload.resolve(strict=True)
    output = args.output.resolve()
    if output.is_relative_to(root) or output.exists():
        parser.error("Choose a new output file outside the payload")
    files = sorted(p.relative_to(root) for p in root.rglob("*") if p.is_file())
    required = {"app/struve-desktop.exe", "app/struve-llm.exe", "app/node.exe", "app/runtime/bridge/runtime.mjs",
                "runtimes/llama.cpp/llama-server.exe", "bin/struve-llm.cmd", "LICENSE", "docs/TEST_SECOND_PC_RU.md"}
    paths = {p.as_posix() for p in files}
    if missing := required - paths:
        parser.error(f"Incomplete staging directory: {sorted(missing)}")
    allowed_dirs = {"app", "bin", "docs", "engines", "examples", "licenses", "media-packs", "models", "runtimes", "skills"}
    allowed_files = {"Cargo.lock", "package-lock.json", "LICENSE", "README.md", "README.en.md", "THIRD_PARTY_NOTICES.md"}
    for p in files:
        if (p.parts[0] not in allowed_dirs and p.as_posix() not in allowed_files
                or p.parts[0] == "engines" and not p.as_posix().startswith("engines/h3-ram-v7/")
                or p.parts[0] == "models" and not p.as_posix().startswith("models/images/slimsam/")
                or p.parts[0] == "runtimes" and p.parts[1] not in ("llama.cpp", "image-edit-python")
                or p.parts[0] == "app" and p.parts[1] != "runtime" and p.name not in {
                    "struve-desktop.exe", "struve-llm.exe", "node.exe", "struve-green.ico", "STRATA-LICENSE.txt"}
                or p.suffix.lower() in (".sqlite", ".gguf", ".safetensors") or p.name == "controller.token"):
            parser.error(f"Unexpected distribution path: {p}")
    # Use only exact file paths in the uninstaller. Unknown files and nonempty directories survive.
    dirs = {parent for p in files for parent in p.parents if parent != Path(".")}
    def nsis(p):
        return str(p).replace("$", "$$")
    with tempfile.TemporaryDirectory() as tmp:
        include = Path(tmp) / "owned-paths.nsh"
        commands = [f'Delete "$INSTDIR\\{nsis(p)}"' for p in files]
        commands.extend(f'RMDir "$INSTDIR\\{nsis(p)}"' for p in sorted(dirs, key=lambda d: (-len(d.parts), d)))
        include.write_text("\n".join(commands), encoding="utf-8-sig")
        script = Path(__file__).with_name("StruveLLM.nsi").resolve()
        args.output.resolve().parent.mkdir(parents=True, exist_ok=True)
        subprocess.run([str(args.makensis.resolve(strict=True)), "/V2", "/INPUTCHARSET", "UTF8",
                        f"/DVERSION={args.version}", f"/DFILEVERSION={'.'.join(version.groups())}",
                        f"/DPAYLOAD={root}", f"/DOUTFILE={output}",
                        f"/DWEBBOOT={args.webview_bootstrapper.resolve(strict=True)}",
                        f"/DMAXROOT={247 - max(len(str(p)) for p in files)}",
                        f"/DUNINSTALL_LIST={include}", str(script)], check=True)
    print(f"{output.name}: {len(files)} packaged files")


if __name__ == "__main__":
    main()
