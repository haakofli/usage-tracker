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

## Copilot

`GET https://api.github.com/copilot_internal/user`, which reports
`quota_snapshots.premium_interactions` — an `entitlement`, a `remaining`, a
`percent_remaining` and a `quota_reset_date`.

The published billing API was the obvious candidate and is the wrong one. It
reports consumption after the fact with no allowance to measure it against,
needs a classic PAT created by hand, and returns nothing at all for the
org-licensed seats most people have. `copilot_internal/user` is undocumented but
is what GitHub's own editor clients call, so it covers every seat type.

The allowance is monthly and there is no second window, so it fills the rail's
ring and leaves the hover ring empty. A plan with unlimited premium requests
reports no remainder, and is shown as such rather than as a full ring — which
would read as "none used".

### Finding a token

No new sign-in: whichever one is already on the machine, in order.

| Source | Where |
| --- | --- |
| Environment | `GH_TOKEN`, `GITHUB_TOKEN` |
| Editor extension | `%LOCALAPPDATA%\github-copilot\apps.json`, or `~/.config/github-copilot/` |
| `gh` CLI | `~/.config/gh/hosts.yml` |

The Copilot CLI stores its own token in the OS keychain whenever there is one.
That copy is deliberately not read: a dock that shows a number is not worth a
keychain prompt, and every other way of signing in leaves a plaintext copy.
Without any of the above, Copilot reads "not signed in" rather than showing a
guess.

## Staleness and caching

Last-good readings are cached, so losing the network shows yesterday's numbers
rather than blanks. Nothing on the card marks a reading as stale: every row is
painted at full strength whatever its age, and only the card itself is
see-through. `DOCK_DIAG=1` is how you tell a fresh reading from a cached one.

The cache sits alongside the settings:

| Platform | Settings and cache |
| --- | --- |
| Windows | `%LOCALAPPDATA%\usage-tracker\` |
| macOS | `~/Library/Application Support/usage-tracker/` |

## Providers

The tray menu lists the AI CLIs found on this machine — detected from their
config directory under your home directory, or their binary on `PATH`. Tick one
to include it in the dock.

**Claude**, **Codex** and **Copilot** can be ticked. Copilot is also considered
installed when only a token is found, since most people use it through an editor
extension that leaves neither a CLI on `PATH` nor a `~/.copilot` directory.

Detection and readability stay separate questions. **Gemini** publishes no
remaining-quota figure at all — its own CLI can only report the current session,
which its maintainers confirm — and **Cursor**'s is behind an undocumented
endpoint plus a session token held in the editor's SQLite. Both are listed
greyed rather than hidden, because hiding them looked identical to failing to
detect them, and both are left unreadable rather than given an invented ring.

Turning a provider off stops it being polled as well as drawn, which matters for
Claude and Copilot: there is no reason to spend rate-limited requests on
something that is not on screen.

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
| Windows | the window is clipped to the card by a window region, and `WM_NCHITTEST` answers `HTTRANSPARENT` across the shadow margin left inside it |
| macOS | `ignoresMouseEvents` is toggled as the cursor crosses the card |

`HTTRANSPARENT` is not enough on its own: Win32 only forwards a hit test
answered that way to other windows on the *same thread*, so a click aimed at
another application landed on the dock's empty space and went nowhere.
`WindowFromPoint` honours it regardless of thread, which is why hovering read
correctly while clicking did not. A window region is applied by the window
manager itself and holds for every process.

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
