# Built-in response styles

These UTF-8 files are compiled into the agent core and sent on each applicable
turn. Codex receives them through `turn/start.additionalContext` under the stable
`rode_response_personality` source with `kind: "application"`, which the runtime
delivers as developer instructions. ACP currently receives a separate user-text
block; RODE does not yet negotiate agents' optional session `systemPrompt`
capability, including the support advertised by `claude-agent-acp` 0.100.0.
These prompts do not configure provider operating modes or replace the backend's
base instructions. Never place user,
project or attachment content in the trusted application context.

Each prompt applies until the next tone selection, including follow-ups. Codex
can deduplicate unchanged application context, so a reminder scoped only to
"this turn" would expire semantically even though RODE resends it every time.

Each personality has one current prompt: `default.txt` resets the agent's tone,
`lamb.txt` supplies the Lamb voice, and `panne.txt` supplies the Panne voice. Edit
these files directly during pre-release development; no version numbers or
historical snapshots are kept. Replay recognizes generated style envelopes
independently of their wording and preserves quoted blocks and assistant replies.

The Lamb description is a compact, original interpretation of Apple's
[character description](https://www.apple.com/tv-pr/originals/slow-horses/) and
[NPR's interview with creator Mick Herron](https://www.gpb.org/news/2023/11/29/gary-oldman-had-free-rein-in-spy-thriller-slow-horses-now-back-for-season-3).
It supplies speech traits rather than assuming the model knows the character.
Examples are original style illustrations, not quotations or results to claim.

Default leaves a fresh chat's configured tone alone. After a styled or restored
chat, its reset prompt replaces the current application context or ACP reminder
without restarting the conversation. Backend instructions and model behavior
can still limit the requested voice; delivery is not a guarantee of compliance.

Deterministic tests check instruction roles, resets and history hygiene. They do not
measure whether a model's generated reply is sufficiently sardonic. Assess that
separately with real replies, including short final answers and follow-ups.
