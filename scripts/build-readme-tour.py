#!/usr/bin/env python3
"""Build the README's captioned GIF/MP4 tour from native 1380x900 UI captures.

Usage: python3 scripts/build-readme-tour.py target/readme-demo/captures
Requires FFmpeg with libx264/drawtext and the DejaVu Sans font. Captures must
contain demonstration data, not personal projects, credentials or real chats.
The screenshots remain unmodified inside the caption frame.
"""

import argparse
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCENES = [
    (
        "01-editor",
        3,
        1,
        "Native editing. Tools on demand.",
        "Rust desktop | familiar keyboard controls | configurable themes",
        "Native RODE captures | synthetic demonstration project",
    ),
    (
        "02-ssh",
        4,
        2,
        "Work where your files live.",
        "OpenSSH aliases, keys and jump hosts | reuse the authenticated connection",
        "Actual SSH connection to an isolated loopback host",
    ),
    (
        "03-sandbox",
        4,
        3,
        "One project. One sandbox policy.",
        "Editor access, shells, agents and tools share the active policy",
        "Actual bubblewrap sandbox | explicit filesystem grants",
    ),
    (
        "04-persist-off",
        1,
        4,
        "Keep your terminal running.",
        "Enable Persist, then reattach after editor exit or SSH loss",
        "Opt-in persistence | no tmux or screen dependency",
    ),
    (
        "04-persist-on",
        3,
        4,
        "Keep your terminal running.",
        "Enable Persist, then reattach after editor exit or SSH loss",
        "Opt-in persistence | no tmux or screen dependency",
    ),
    (
        "05-notebook",
        5,
        5,
        "Run notebooks. Inspect results.",
        "Cells expand to fit their content | kernels run on the project host",
        "Real Python kernel | synthetic experiment data",
    ),
    (
        "06-agents",
        3,
        6,
        "Choose your coding agent.",
        "Codex, local models and ACP | project-host execution",
        "Provider controls | no model call or staged conversation",
    ),
    (
        "07-slurm",
        4,
        7,
        "Request compute resources.",
        "Slurm CPU / GPU allocations for shells, notebooks and debugging",
        "Allocation form with an empty demo scheduler | no resources requested",
    ),
    (
        "08-git",
        3,
        8,
        "Review changes in the editor.",
        "Git diffs, staging, branches and worktrees on the project host",
        "Real Git diff in the demonstration repository",
    ),
]


def run(*args):
    subprocess.run(args, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("captures", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "assets")
    options = parser.parse_args()
    for name, *_ in SCENES:
        if not (options.captures / f"{name}.png").is_file():
            parser.error(f"Missing native capture: {name}.png")
    font = Path("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
    if not font.is_file():
        parser.error(f"Required font not found: {font}")
    options.output.mkdir(parents=True, exist_ok=True)
    ffmpeg = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y"]
    with tempfile.TemporaryDirectory(prefix="rode-tour-") as directory:
        work = Path(directory)
        frames = []
        for index, (name, seconds, chapter, title, subtitle, note) in enumerate(SCENES):
            filters = ["pad=1380:1040:0:100:color=0x101010"]
            for label, content, x, y, size, colour in (
                ("title", title, "28", 16, 38, "0xe0ad37"),
                ("subtitle", subtitle, "30", 66, 22, "0xe0e0e0"),
                ("note", note, "28", 1012, 17, "0xb3b3b3"),
                ("chapter", f"{chapter:02d} / 08", "w-tw-28", 24, 20, "0xb3b3b3"),
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
        run(
            *ffmpeg,
            *source,
            "-filter_complex",
            "scale=1000:-2:flags=lanczos,split[a][b];"
            "[a]palettegen=max_colors=256:stats_mode=full[p];"
            "[b][p]paletteuse=dither=none:diff_mode=rectangle",
            "-t",
            duration,
            "-fps_mode",
            "vfr",
            "-loop",
            "0",
            "-final_delay",
            str(frames[-1][1] * 100),
            str(options.output / "rode-tour.gif"),
        )
        shutil.copyfile(frames[0][0], options.output / "rode-tour.png")
    for suffix in ("gif", "mp4", "png"):
        path = options.output / f"rode-tour.{suffix}"
        print(f"{path}: {path.stat().st_size / 1024:.0f} KiB")


if __name__ == "__main__":
    main()
