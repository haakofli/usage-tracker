# How it works

Detail that would crowd the [README](../README.md), kept because most of it is
the reasoning behind a decision rather than a description of one.

## Claude

`GET /api/oauth/usage` on `api.anthropic.com`, authorised with the OAuth token in
`~/.claude/.credentials.json`.

That file is **only ever read, never written**. A bug that wrote to it would
corrupt the credentials your real Claude Code sessions depend on, so when the
token is expired or the API returns 401 the dock shows the cached value marked
stale instead of attempting a refresh. Claude Code refreshes the token itself
during normal use, and the dock picks that up on its next poll.

### Polling and rate limits

The Claude usage endpoint rate-limits aggressively — sustained 30–60s polling is
reported to return a session-long 429 with no signal for when it clears. So:

- 5 minute default interval, with a hard 2 minute floor enforced in code that
  even a manual refresh cannot bypass
- On 429, exponential backoff 5 → 10 → 20 → 40 → 60 minutes, capped
- The cached value keeps showing throughout, marked stale

## Codex

Parsed from the `rate_limits` snapshot Codex already writes into
`~/.codex/sessions/**/rollout-*.jsonl`. No subprocess, and it cannot be rate
limited. Its weakness is freshness: it is only as current as your last Codex
request, which is why the tray tooltip reports the observation time.

Codex polls every 60s, being a local file read.

## Staleness and caching

A dimmed block with an amber dot means stale. Last-good readings are cached, so
losing the network shows yesterday's numbers rather than blanks:

| Platform | Settings and cache |
| --- | --- |
| Windows | `%LOCALAPPDATA%\usage-tracker\` |
| macOS | `~/Library/Application Support/usage-tracker/` |

## Providers

The tray menu lists the AI CLIs found on this machine — detected from their
config directory under your home directory, or their binary on `PATH`. Tick one
to include it in the dock.

Only **Claude** and **Codex** can actually be ticked. Detection and readability
are separate questions: Copilot, Gemini and Cursor are detected and listed, but
none of them publishes a quota you can read locally the way Claude's usage
endpoint and Codex's rollout files do, so they appear greyed out rather than as
empty rings — hiding them looked identical to failing to detect them.

Turning a provider off stops it being polled as well as drawn, which matters for
Claude: there is no reason to spend rate-limited requests on something that is
not on screen.

## Rendering

`egui` is built on the `glow` (OpenGL) backend rather than the default `wgpu`:
measured at 117 MB resident and an 11 MB binary against 278 MB and 19 MB, with
identical output. Footprint is the point of a dock that runs all day.

The window is never resized while hovering — that recreates the GL surface and
makes the animation stutter — so it stays at its expanded size and only the
painted card animates. The rest of the window is transparent, and each platform
has to be told not to swallow clicks meant for what is behind it:

| Platform | Mechanism |
| --- | --- |
| Windows | `WM_NCHITTEST` answers `HTTRANSPARENT` outside the card |
| macOS | `ignoresMouseEvents` is toggled as the cursor crosses the card |

Geometry is stored in physical pixels throughout, because they are the only unit
that does not move when the zoom changes. `src/platform/` holds everything that
differs between the two systems; nothing above it is platform-aware.

## Starting at login

The app registers itself rather than shipping an install script, so nothing has
to be run with elevated trust to get a dock that survives a reboot. Both entries
are plain, per-user and auditable:

| Platform | Entry |
| --- | --- |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `usage-tracker` — visible in Task Manager's Startup tab |
| macOS | `~/Library/LaunchAgents/dev.haakofli.usage-tracker.plist` |

The OS entry is the source of truth, not a flag in `settings.json`. Deleting it
by hand is therefore honoured rather than silently rewritten on next launch, and
the menu tick reflects what is actually registered.

## Diagnostics

`DOCK_DIAG=1` logs poll cadence, hover transitions and DPI facts to stderr.
`DOCK_SCALE` forces a zoom factor without driving the pointer.
