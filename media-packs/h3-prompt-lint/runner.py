
if __name__ == "__main__":
    import json
    from pathlib import Path
    import sys
    request = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    warnings = lint(request["prompt"], mode=request["mode"], duration_seconds=request["duration_seconds"])
    Path(sys.argv[2]).write_text(json.dumps(warnings, ensure_ascii=False), encoding="utf-8")
