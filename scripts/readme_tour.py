"""Shared scene order, captions and theme selection for the native README tour."""

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
        "SSH aliases, keys and jump hosts | tools run on the project host",
        "Actual SSH connection to an isolated loopback host",
    ),
    (
        "03-sandbox",
        4,
        3,
        "One project. One sandbox policy.",
        "Project policy for editor access, agents and tools | separate terminal and Git modes",
        "Actual project sandbox | explicit filesystem grants",
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
        "Codex, Claude Code, local models and ACP | project-host execution",
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
        "Change markers and inline previews | Git staging, branches and worktrees",
        "Real Git change preview in the demonstration repository",
    ),
]

# Keep theme IDs and capture names shared by the recorder and encoder.
THEMES = (
    (
        "atelier-light",
        "Atelier Light",
        "A quiet light workspace with coordinated editor and terminal colors",
    ),
    ("atelier-dark", "Atelier Dark", "Neutral dark surfaces with clear syntax and mint accents"),
    (
        "sumie-light",
        "Sumi-e Light",
        "Washi-paper colors, ink-style controls and original Japanese artwork",
    ),
    ("samurai", "Samurai", "Obsidian surfaces, silver-steel details and original samurai artwork"),
    (
        "synthwave84",
        "SynthWave ’84",
        "The adapted SynthWave palette across native chrome and source highlighting",
    ),
)

SCENES += [
    (
        f"09-theme-{theme_id}",
        2,
        9,
        f"Make it yours. {name}",
        description,
        "Actual theme switching | the same source, font size and project",
    )
    for theme_id, name, description in THEMES
]
