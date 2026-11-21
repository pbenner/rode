"""Shared scene order, captions and theme selection for the native README tour."""

SCENES = [
    (
        "01-editor",
        3,
        1,
        "More room for your code.",
        "A clean editor canvas | Project tools and Explorer in the left activity bar",
        "Native RODE captures | synthetic demonstration project",
    ),
    (
        "02-project-tools",
        3,
        2,
        "Project tools, one place.",
        "Chat, Terminal, Git, Problems and Tasks | toggle from the left activity bar",
        "Project-scoped tabs | Detach and Detach tab keep their existing sessions",
    ),
    (
        "03-connections",
        3,
        3,
        "Connections for every project.",
        "Saved SSH hosts and return tunnels | generic tools stay on the left",
        "Synthetic saved profiles and tunnel setups | no connection or tunnel started",
    ),
    (
        "04-ssh",
        4,
        4,
        "Work where your files live.",
        "SSH aliases, keys and jump hosts | tools run on the project host",
        "Actual SSH connection to an isolated loopback host",
    ),
    (
        "05-sandbox",
        4,
        5,
        "One project. One sandbox policy.",
        "Project policy for editor access, agents and tools | separate terminal and Git modes",
        "Actual project sandbox | explicit filesystem grants",
    ),
    (
        "06-persist-off",
        1,
        6,
        "Keep your terminal running.",
        "Enable Persist, then reattach after editor exit or SSH loss",
        "Opt-in persistence | no tmux or screen dependency",
    ),
    (
        "06-persist-on",
        3,
        6,
        "Keep your terminal running.",
        "Enable Persist, then reattach after editor exit or SSH loss",
        "Opt-in persistence | no tmux or screen dependency",
    ),
    (
        "07-notebook",
        5,
        7,
        "Run notebooks. Inspect results.",
        "Cells expand to fit their content | kernels run on the project host",
        "Real Python kernel | synthetic experiment data",
    ),
    (
        "08-agents",
        3,
        8,
        "Choose your coding agent.",
        "Codex, Codex local, Claude Code and ACP | project-host execution",
        "Native provider controls | no model inference",
    ),
    (
        "08-agent-edits",
        3,
        8,
        "Keep control of agent edits.",
        "Undo last edits restores the completed prompt's recorded text changes",
        "Scripted local agent fixture | real file edit | no model inference",
    ),
    (
        "08-agent-undo",
        3,
        8,
        "Undo the prompt. Keep earlier work.",
        "The original setting is restored | later file edits are protected",
        "Real host-side Undo | existing uncommitted changes and chat history retained",
    ),
    (
        "09-slurm",
        4,
        9,
        "Request compute resources.",
        "Slurm CPU / GPU allocations for shells, notebooks and debugging",
        "Allocation form with an empty demo scheduler | no resources requested",
    ),
    (
        "10-git",
        3,
        10,
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
        f"11-theme-{theme_id}",
        2,
        11,
        f"Make it yours. {name}",
        description,
        "Actual theme switching | the same source, font size and project",
    )
    for theme_id, name, description in THEMES
]
