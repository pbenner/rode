# Built-in response styles

These UTF-8 files are compiled into the agent core and sent as a separate text
block on each applicable turn. They do not configure provider operating modes.

Each personality has one current prompt: `default.txt` resets the agent's tone,
and `lamb.txt` supplies the Lamb voice. Edit these files directly during
pre-release development; no version numbers or historical snapshots are kept.
Replay strips only the current exact hint suffixes and recognized preview
prefixes, never arbitrary XML or assistant replies. Backends may retain earlier
wording in their saved conversations; compatibility with those development
snapshots is not maintained.

The Lamb description is a compact, original interpretation of Apple's
[character description](https://www.apple.com/tv-pr/originals/slow-horses/) and
[NPR's interview with creator Mick Herron](https://www.gpb.org/news/2023/11/29/gary-oldman-had-free-rein-in-spy-thriller-slow-horses-now-back-for-season-3).
It supplies speech traits rather than assuming the model knows the character.
Examples are original style illustrations, not quotations or results to claim.

Deterministic tests check transport, resets and history hygiene. They do not
measure whether a model's generated reply is sufficiently sardonic. Assess that
separately with real replies, including short final answers and follow-ups.
