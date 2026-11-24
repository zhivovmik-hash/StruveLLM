"""Level the exposure of a short, fixed-camera H3 clip using its end frames."""
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

import numpy as np


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def previews(ffmpeg, path, threads):
    args = [
        ffmpeg, "-hide_banner", "-nostdin", "-nostats", "-loglevel", "info", "-xerror",
        "-threads", threads, "-filter_threads", threads, "-i", str(path), "-map", "0:v:0",
        "-frames:v", "363", "-vf", "showinfo,scale=192:108", "-vsync", "0",
        "-threads", threads, "-pix_fmt", "rgb24", "-f", "rawvideo", "-",
    ]
    result = subprocess.run(args, capture_output=True, check=True)
    log = result.stderr.decode("utf-8", errors="replace")
    sizes = set(re.findall(r"\bs:(\d+)x(\d+)", log))
    timing = re.search(r"config in time_base: (\d+/\d+), frame_rate: (\d+/\d+)", log)
    if len(sizes) != 1 or not timing or Fraction(timing[2]) != 24:
        raise ValueError("Expected a constant-size, 24 fps Struve video")
    width, height = map(int, next(iter(sizes)))
    frames = np.frombuffer(result.stdout, np.uint8).reshape(-1, 108, 192, 3)
    if not 2 <= len(frames) <= 362 or not (32 <= width <= 1536 and 32 <= height <= 1536):
        raise ValueError("Expected 2 to 362 frames and a resolution up to 1536 pixels per side")
    pts = [int(value) for value in re.findall(r"\bn:\s*\d+\s+pts:\s*(-?\d+)\s+pts_time:", log)]
    if len(pts) != len(frames) or any((b-a) * Fraction(timing[1]) != Fraction(1, 24) for a, b in zip(pts, pts[1:])):
        raise ValueError("Input video has irregular frame timestamps")
    audio_count = len(re.findall(r"Stream #\d+:\d+[^\n]*?: Audio:", log))
    return frames, (width, height), audio_count


def luma(rgb):
    # Fixed-point Rec. 709, evaluated without uint8 overflow.
    wide = rgb.astype(np.int32)
    return ((54 * wide[..., 0] + 183 * wide[..., 1] + 19 * wide[..., 2] + 128) >> 8).astype(np.uint8)


def curves(frames):
    first, last = frames[0].astype(np.int16), frames[-1].astype(np.int16)
    stable = np.abs(first - last).max(axis=2) <= 18
    if stable.mean() < 0.18:
        raise ValueError("The end frames have too little shared background for fixed-camera correction")
    a = luma(frames[0])[stable].astype(np.float32)
    b = luma(frames[-1])[stable].astype(np.float32)
    identity = np.arange(256, dtype=np.float32)
    tables = []
    for index, frame in enumerate(frames):
        if index in (0, len(frames) - 1):
            tables.append(identity.copy())
            continue
        fraction = index / (len(frames) - 1)
        target = a * (1 - fraction) + b * fraction
        observed = luma(frame)[stable]
        x, y = [0.0], [0.0]
        for lower in range(0, 256, 12):
            group = (target >= lower) & (target < lower + 12)
            if group.sum() < 35:
                continue
            current = float(np.median(observed[group]))
            wanted = float(np.median(target[group]))
            if x[-1] + 1 <= current < 255 and wanted > y[-1]:
                x.append(current)
                y.append(wanted)
        if len(x) < 5:
            raise ValueError("Insufficient tonal detail in the shared background")
        table = np.interp(identity, [*x, 255.0], [*y, 255.0])
        # A few quantized levels in deep shadows contain little recoverable detail.
        tables.append(np.minimum(table, 5 * identity))
    raw = np.stack(tables)
    smoothed = raw.copy()
    for index in range(1, len(raw) - 1):
        smoothed[index] = np.median(raw[index-1:index+2], axis=0)
    return np.rint(smoothed).astype(np.uint8), stable


def apply_curve(rgb, table):
    lum = luma(rgb)
    brightness = table[lum]
    levels = np.arange(256, dtype=np.float32)
    # Gently lift colour while avoiding per-channel amplification of H.264 shadow noise.
    gains = np.sqrt((table.astype(np.float32) + 1) / (levels + 1)).clip(0.65, 2)
    chroma = rgb.astype(np.int16) - lum[..., None]
    output = brightness[..., None] + chroma * gains[lum, None]
    return np.rint(output).clip(0, 255).astype(np.uint8)


def encode(ffmpeg, source, output, size, tables, threads, log):
    width, height = size
    common = [ffmpeg, "-hide_banner", "-nostdin", "-loglevel", "error", "-xerror", "-filter_threads", threads]
    decode = [*common, "-threads", threads, "-i", str(source), "-map", "0:v:0", "-vsync", "0",
              "-threads", threads, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
    encode_args = [*common, "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{width}x{height}", "-framerate",
                   "24", "-i", "pipe:0", "-i", str(source), "-map", "0:v:0", "-map", "1:a?", "-c:a", "copy",
                   "-c:v", "libx264", "-threads", threads, "-crf", "16", "-pix_fmt", "yuv420p",
                   "-movflags", "+faststart", "-n", str(output)]
    with log.open("wb") as errors, subprocess.Popen(decode, stdout=subprocess.PIPE, stderr=errors) as reader:
        try:
            with subprocess.Popen(encode_args, stdin=subprocess.PIPE, stderr=errors) as writer:
                try:
                    for table in tables:
                        raw = reader.stdout.read(width * height * 3)
                        if len(raw) != width * height * 3:
                            raise ValueError("Input ended before the final frame")
                        pixels = np.frombuffer(raw, np.uint8).reshape(height, width, 3)
                        writer.stdin.write(apply_curve(pixels, table).tobytes())
                    if reader.stdout.read(1):
                        raise ValueError("Input frame count changed")
                    writer.stdin.close()
                    if writer.wait() != 0 or reader.wait() != 0:
                        raise ValueError("FFmpeg processing failed")
                finally:
                    if writer.poll() is None:
                        writer.kill()
        finally:
            if reader.poll() is None:
                reader.kill()


def main(job):
    source = Path(job["source"]).resolve(strict=True)
    directory = Path(job["directory"]).resolve(strict=True)
    target = directory / "exposure-matched.mp4"
    metrics_file = directory / "exposure-metrics.json"
    if target.exists() or metrics_file.exists():
        raise FileExistsError("This output already exists")
    threads = str(max(1, min(32, int(job["cpu_threads"]))))
    original = digest(source)
    ffmpeg = job["ffmpeg"]
    frames, size, audio_count = previews(ffmpeg, source, threads)
    tables, stable = curves(frames)
    with tempfile.TemporaryDirectory(dir=directory, prefix="grade-") as temporary:
        graded = Path(temporary) / "graded.mp4"
        encode(ffmpeg, source, graded, size, tables, threads, directory / "video-exposure-ffmpeg.log")
        checked, output_size, output_audio = previews(ffmpeg, graded, threads)
        if checked.shape != frames.shape or size != output_size or audio_count != output_audio or digest(source) != original:
            raise ValueError("Output streams or original video changed unexpectedly")
        before, after = [np.median(luma(clip)[:, stable], axis=1) for clip in (frames, checked)]
        reference = np.linspace(before[0], before[-1], len(before))
        result = dict(source_sha256=original, output_sha256=digest(graded), frames=len(frames), fps=24,
                      width=size[0], height=size[1], audio_streams=output_audio,
                      stable_pixel_fraction=float(stable.mean()), reference_luma=[float(before[0]), float(before[-1])],
                      source_min_background_luma=float(before.min()), output_min_background_luma=float(after.min()),
                      source_mean_background_error=float(np.abs(before-reference).mean()),
                      output_mean_background_error=float(np.abs(after-reference).mean()))
        graded.rename(target)
    metrics_file.write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result))


if __name__ == "__main__":
    job = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8-sig"))
    try:
        main(job)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        (Path(job["directory"]) / "exposure-error.txt").write_text(str(error), encoding="utf-8")
        raise
