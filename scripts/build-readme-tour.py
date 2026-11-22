#!/usr/bin/env python3
"""Build the README's captioned GIF/MP4 tour from native 1380x900 UI captures.

Usage: python3 scripts/build-readme-tour.py target/readme-demo/captures
Requires FFmpeg with libx264/drawtext and the DejaVu Sans font. Captures must
contain demonstration data, not personal projects, credentials or real chats.
The screenshots remain unmodified inside the caption frame.
"""

import argparse
import hashlib
import json
import shutil
import subprocess
import tempfile
from pathlib import Path

from readme_tour import POSTER_SCENE, SCENES

ROOT = Path(__file__).resolve().parent.parent


def run(*args):
    """Run a literal external-tool argument list and stop publication immediately on failure."""
    subprocess.run(args, check=True)


def main():
    """Validate native captures and compose the captioned GIF, MP4 and still artifacts with bounded scene durations."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("captures", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "assets")
    options = parser.parse_args()
    for name, *_ in SCENES:
        if not (options.captures / f"{name}.png").is_file():
            parser.error(f"Missing native capture: {name}.png")
    manifest = options.captures / "capture.json"
    if not manifest.is_file():
        parser.error("Missing capture.json; record a fresh native capture set first")
    metadata = json.loads(manifest.read_text())
    if metadata.get("sandbox_backend") not in ("landlock", "bwrap", "none"):
        parser.error(
            "capture.json must identify the recorded sandbox backend or explicit normal mode"
        )
    for name, *_ in SCENES:
        path = options.captures / f"{name}.png"
        if hashlib.sha256(path.read_bytes()).hexdigest() != metadata.get("captures", {}).get(
            path.name
        ):
            parser.error(f"Native capture does not match capture.json: {path.name}")
    font = Path("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
    if not font.is_file():
        parser.error(f"Required font not found: {font}")
    options.output.mkdir(parents=True, exist_ok=True)
    ffmpeg = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y"]
    with tempfile.TemporaryDirectory(prefix="rode-tour-") as directory:
        work = Path(directory)
        frames = []
        chapters = max(scene[2] for scene in SCENES)
        for index, (name, seconds, chapter, title, subtitle, note) in enumerate(SCENES):
            if name == "05-sandbox" and metadata.get("sandbox_backend") in ("bwrap", "landlock"):
                backend = "Landlock" if metadata["sandbox_backend"] == "landlock" else "bubblewrap"
                note = f"Actual {backend} sandbox | explicit filesystem grants"
            elif name == "05-sandbox" and metadata["sandbox_backend"] == "none":
                title = "Configure project access."
                note = "Sandbox setup controls | normal mode shown; enforcement disabled for this capture"
            filters = ["pad=1380:1040:0:100:color=0x101010"]
            for label, content, x, y, size, colour in (
                ("title", title, "28", 16, 38, "0xe0ad37"),
                ("subtitle", subtitle, "30", 66, 22, "0xe0e0e0"),
                ("note", note, "28", 1012, 17, "0xb3b3b3"),
                ("chapter", f"{chapter:02d} / {chapters:02d}", "w-tw-28", 24, 20, "0xb3b3b3"),
            ):
                textfile = work / f"{index}-{label}.txt"
                textfile.write_text(content)
                filters.append(
                    f"drawtext=fontfile={font}:textfile={textfile}:"
                    f"x={x}:y={y}:fontsize={size}:fontcolor={colour}:expansion=none"
                )
            frame = work / f"{index:02d}.png"
            run(
                *ffmpeg,
                "-i",
                str(options.captures / f"{name}.png"),
                "-vf",
                ",".join(filters),
                "-frames:v",
                "1",
                str(frame),
            )
            frames.append((frame, seconds))
        manifest = work / "scenes.ffconcat"
        manifest.write_text(
            "ffconcat version 1.0\n"
            + "".join(f"file '{path}'\nduration {seconds}\n" for path, seconds in frames)
            + f"file '{frames[-1][0]}'\n"
        )
        duration = str(sum(seconds for _, seconds in frames))
        source = ["-f", "concat", "-safe", "0", "-i", str(manifest)]
        run(
            *ffmpeg,
            *source,
            "-vf",
            "fps=12",
            "-t",
            duration,
            "-c:v",
            "libx264",
            "-preset",
            "slow",
            "-tune",
            "stillimage",
            "-crf",
            "20",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-an",
            str(options.output / "rode-tour.mp4"),
        )
        # Keep the preview readable at README width and use the richest palette
        # that meets its established size budget. MP4/still quality is independent.
        gif = options.output / "rode-tour.gif"
        for colors in (96, 80, 64, 56, 48):
            run(
                *ffmpeg,
                *source,
                "-filter_complex",
                "scale=880:-2:flags=lanczos,split[a][b];"
                f"[a]palettegen=max_colors={colors}:stats_mode=full[p];"
                "[b][p]paletteuse=dither=none:diff_mode=rectangle",
                "-t",
                duration,
                "-fps_mode",
                "vfr",
                "-loop",
                "0",
                "-final_delay",
                str(frames[-1][1] * 100),
                str(gif),
            )
            if gif.stat().st_size < 1024 * 1024:
                print(f"GIF preview uses {colors} colors at 880px within its 1 MiB budget")
                break
        else:
            parser.error("GIF preview exceeds 1 MiB; simplify the tour before publishing")
        poster = next(
            frame for scene, (frame, _) in zip(SCENES, frames) if scene[0] == POSTER_SCENE
        )
        shutil.copyfile(poster, options.output / "rode-tour.png")
    for suffix in ("gif", "mp4", "png"):
        path = options.output / f"rode-tour.{suffix}"
        print(f"{path}: {path.stat().st_size / 1024:.0f} KiB")


if __name__ == "__main__":
    main()
