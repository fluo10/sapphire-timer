# sapphire-timer

A preset-based timer that keeps your data alive as plain text — timeless like fossils.

Presets are TOML files. Session logs are append-only JSONL. Both are ordinary
files in a directory you can put under git, and both are full-text searchable.

```console
$ sapphire-timer init
$ sapphire-timer preset list
break               5 min  Short break between focus blocks.
pomodoro           25 min  Focus block. One task, no context switching.

$ sapphire-timer start pomodoro -c "reviewing the framework migration PR"
pomodoro — 25 min. Ctrl-C to stop early.
  24:31  remaining
completed: pomodoro (25:00)

$ sapphire-timer search "framework"
logs/2026-07.jsonl
    comment: reviewing the framework migration PR
    preset_name: pomodoro
    ...
```

## Why it exists

sapphire-timer is a real app, but it is also the testbed for
[sapphire-framework](https://github.com/fluo10/sapphire-framework). It is
deliberately the smallest thing that exercises the parts of the framework its
other consumers don't:

- **TOML and JSONL, not Markdown.** sapphire-journal only ever indexes Markdown,
  so the framework's TOML and JSONL chunkers had no real user. Presets and logs
  are exactly those two shapes.
- **No database of its own.** sapphire-journal and sapphire-ledger each run a
  SQLite cache alongside the framework's index. sapphire-timer reads presets and
  logs from disk and delegates search to the framework — so if it works, the
  framework's index is enough on its own.
- **Consequently, no SQLite at all.** `cargo tree -i libsqlite3-sys` is empty.
  CI asserts it stays that way.

## Layout

```text
<root>/
├── .sapphire-timer/
│   ├── config.toml     # per-workspace, committed, shared across machines
│   └── .gitignore
├── presets/
│   └── pomodoro.toml   # one TOML per preset
└── logs/
    └── 2026-07.jsonl   # one line per finished session, append-only
```

Presets and logs live *outside* `.sapphire-timer/` on purpose: the framework's
indexer skips dot-prefixed directories at any depth, so anything moved inside
the marker directory silently stops being searchable.

### Presets

```toml
id = "gkq44t7"
name = "pomodoro"
duration_minutes = 25
description = "Focus block. One task, no context switching."
```

Write one by hand with just `duration_minutes` — the `name` falls back to the
filename and the `id` is assigned on first load and written back.

The `id` is what session logs reference, so **renaming a preset keeps its
history**. Ids are also de-duplicated on load, which matters because copying a
preset file to make a new one is the obvious way to add one.

### Session logs

One JSON object per line, appended and never rewritten:

```json
{"id":"gkq4787","preset_id":"gkq46kz","preset_name":"quicktest","started_at":"…","ended_at":"…","elapsed_secs":1500,"outcome":"completed","comment":"…"}
```

`preset_name` is denormalised deliberately: it records what the preset was
called *at the time*, so the log stays readable after a rename or a deletion.
Resolution always goes through `preset_id`.

Appending rather than rewriting is what lets the framework's line-keyed JSONL
chunker leave earlier sessions alone — a new session re-indexes one line, not
the whole month.

A session interrupted with Ctrl-C is still recorded, with
`"outcome":"interrupted"` and the elapsed time. A countdown you abandoned after
twenty minutes is data.

## Search

Full-text search is a trigram index, so substring and CJK queries work:

```console
$ sapphire-timer search "ramewor"     # matches mid-word
$ sapphire-timer search "レビュー"     # CJK
```

Queries shorter than three characters match nothing — that is inherent to a
trigram index, not a bug.

Vector search is available but off by default; a timer log is small enough that
full-text search over it is the right tool. See `docs/config/user-config.toml`.

## Sync

`sapphire-timer sync` commits, pulls, pushes and re-indexes, if the workspace is
inside a git repository.

> **Known limitation.** The framework resolves merge conflicts per-file, keeping
> whichever side has the newer author timestamp. Because sessions all land in
> one month file, **running timers on two machines at once and then syncing can
> drop one machine's sessions wholesale.** Sync from one machine at a time until
> this is addressed upstream.

## Status

Early. The framework it depends on is not published to crates.io yet, so
sapphire-timer depends on it via git and **cannot itself be published** until
that lands — see
[sapphire-framework#80](https://github.com/fluo10/sapphire-framework/issues/80).

## License

MIT OR Apache-2.0.
