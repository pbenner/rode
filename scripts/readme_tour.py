"""Shared scene order, captions and theme selection for the native README tour."""

SCENES = [
    (
        "00-welcome",
        3,
        1,
        "Start with a clear workspace.",
        "Your working directory is ready | reveal Explorer and Project tools when needed",
        "Native welcome screen | no project tools start until requested",
    ),
    (
        "01-editor",
        3,
        2,
        "More room for your code.",
        "A clean editor canvas | Project tools and Explorer in the left activity bar",
        "Native RODE captures | synthetic demonstration project",
    ),
    (
        "02-project-tools",
        3,
        3,
        "Project tools, one place.",
        "Chat, Terminal, Git, Problems and Tasks | toggle from the left activity bar",
        "Project-scoped tabs | Detach and Detach tab keep their existing sessions",
    ),
    (
        "03-connections",
        3,
        4,
        "Connections for every project.",
        "Saved projects, SSH hosts and tunnels | one dedicated sidebar",
        "Real project bookmark | synthetic SSH profiles and saved tunnel setups",
    ),
    (
        "04-ssh",
        4,
        5,
        "Work where your files live.",
        "SSH aliases, keys and jump hosts | tools run on the project host",
        "Actual SSH connection to an isolated loopback host",
    ),
    (
        "05-sandbox",
        4,
        6,
        "One project. One sandbox policy.",
        "Project policy for editor access, agents and tools | separate terminal and Git modes",
        "Actual project sandbox | explicit filesystem grants",
    ),
    (
        "06-persist-off",
        1,
        7,
        "Keep your terminal running.",
        "Enable Persist, then reattach after editor exit or SSH loss",
        "Opt-in persistence | no tmux or screen dependency",
    ),
    (
        "06-persist-on",
        3,
        7,
        "Keep your terminal running.",
        "Enable Persist, then reattach after editor exit or SSH loss",
        "Opt-in persistence | no tmux or screen dependency",
    ),
    (
        "07-notebook",
        5,
        8,
        "Run notebooks. Inspect results.",
        "Compact controls, rendered equations and plots | native notebook cells",
        "Real Python kernel | synthetic experiment data",
    ),
    (
        "07-markdown-math",
        4,
        9,
        "Write equations. Read your results.",
        "Inline and display LaTeX in Markdown | themed previews and Copy LaTeX",
        "Bundled native math renderer | no TeX installation or network service",
    ),
    (
        "08-encrypted-browser",
        3,
        10,
        "Browse to encrypted storage.",
        "Explorer shortcut and native folder selection | local or connected SSH host",
        "Synthetic folders | storage selection only; no vault is unlocked",
    ),
    (
        "08-encrypted",
        3,
        10,
        "Open or create an encrypted project.",
        "Choose storage, then unlock | executable overrides stay under Advanced options",
        "Native setup form | no vault is created or unlocked in this demonstration",
    ),
    (
        "08-agents",
        3,
        11,
        "Choose your coding agent.",
        "Codex, Codex local, Claude Code and ACP | project-host execution",
        "Native provider controls | no model inference",
    ),
    (
        "08-agent-edits",
        3,
        11,
        "Keep control of agent edits.",
        "Undo last edits restores the completed prompt's recorded text changes",
        "Scripted local agent fixture | real file edit | no model inference",
    ),
    (
        "08-agent-undo",
        3,
        11,
        "Undo the prompt. Keep earlier work.",
        "The original setting is restored | later file edits are protected",
        "Real host-side Undo | existing uncommitted changes and chat history retained",
    ),
    (
        "09-slurm",
        4,
        12,
        "Request compute resources.",
        "Slurm CPU / GPU allocations for shells, notebooks and debugging",
        "Allocation form with an empty demo scheduler | no resources requested",
    ),
    (
        "10-git",
        3,
        13,
        "Review changes in the editor.",
        "Change markers and inline previews | Git staging, branches and worktrees",
        "Real Git change preview in the demonstration repository",
    ),
    (
        "10-close-project",
        3,
        14,
        "Close the project. Protect your edits.",
        "One contextual project action | unsaved changes are checked before closing",
        "Native confirmation | only an unsaved demonstration note is discarded",
    ),
    (
        "10-project-closed",
        3,
        14,
        "Return to a clear workspace.",
        "Welcome and Open project return | saved projects stay in Connections",
        "Actual project close | all file tabs closed | working directory retained",
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
        15,
        f"Make it yours. {name}",
        description,
        "Actual theme switching | the same source, font size and project",
    )
    for theme_id, name, description in THEMES
]

POSTER_SCENE = "01-editor"
