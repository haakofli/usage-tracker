# usage-tracker

A small always-on-top Windows dock showing how much of your Claude and Codex
quota is left.

Collapsed, it is a narrow rail: one ring per provider for the 5-hour window,
with the time until it resets. Hover it and the weekly ring slides out
alongside. The 5-hour rings never move, so opening the panel adds information
rather than rearranging it.

```
   collapsed                    hovered
  ┌────────┐        ┌──────────────────────────┐
  │   ◕    │        │    ◔          ◕          │
  │  47%   │        │   11%        47%         │
  │ 1h 39m │        │  Friday     1h 39m       │
  │        │        │  01:00                   │
  │   ◕    │        │    ◔          ◕          │
  │  100%  │        │   64%        100%        │
  │ 1h 41m │        │  Tuesday    1h 41m       │
  └────────┘        │  12:41                   │
                    └──────────────────────────┘
```

Ring colour tracks usage: teal under 50%, amber to 80%, rose above.

## Install

```powershell
.\install.ps1
```

Builds a release binary, copies it to
`%LOCALAPPDATA%\Programs\usage-tracker`, adds a Start Menu shortcut, and starts
it at login. Per-user, no admin rights.

```powershell
.\install.ps1 -NoStartup    # install without running at login
.\install.ps1 -Uninstall    # remove it again
```

## Using it

| Action | How |
|---|---|
| See weekly quota | Hover the dock |
| Move it | Drag it; it attaches to whichever screen edge you release it near |
| Resize | Hover it, then **Ctrl +** / **Ctrl −** (**Ctrl 0** resets) |
| Choose providers | Tray menu — tick the ones to show |
| Refresh now | Right-click → **Refresh now** |
| Switch edge | Right-click → **Attach to left/right edge** |
| Hide / show | Left-click the tray icon, or its menu |
| Quit | Right-click → **Quit**, or the tray menu |

The zoom keys only act while the pointer is over the dock, so Ctrl +/- keeps
working normally everywhere else.

The tray tooltip carries both percentages plus when each reading was taken.
Position, edge, size and provider choice persist across restarts.

## Providers

The tray menu lists the AI CLIs found on this machine — detected from their
config directory under your profile, or their binary on `PATH`. Tick one to
include it in the dock.

Only **Claude** and **Codex** can actually be ticked. Detection and readability
are separate questions: Copilot, Gemini and Cursor are detected and listed, but
none of them publishes a quota you can read locally the way Claude's usage
endpoint and Codex's rollout files do, so they appear greyed out rather than as
empty rings.

Turning a provider off stops it being polled as well as drawn, which matters
for Claude — there is no reason to spend rate-limited requests on something
that is not on screen.

## Where the numbers come from

**Claude** — `GET /api/oauth/usage` on `api.anthropic.com`, authorised with the
OAuth token in `~/.claude/.credentials.json`.

That file is **only ever read, never written**. A bug that wrote to it would
corrupt the credentials your real Claude Code sessions depend on, so when the
token is expired or the API returns 401 the dock shows the cached value marked
stale instead of attempting a refresh. Claude Code refreshes the token itself
during normal use and the dock picks that up on its next poll.

**Codex** — parsed from the `rate_limits` snapshot Codex already writes into
`~/.codex/sessions/**/rollout-*.jsonl`. No subprocess, and it cannot be rate
limited. Its weakness is freshness: it is only as current as your last Codex
request, which is why the tray tooltip reports the observation time.

### Polling and rate limits

The Claude usage endpoint rate-limits aggressively — sustained 30–60s polling
is reported to return a session-long 429 with no signal for when it clears. So:

- 5 minute default interval, with a hard 2 minute floor enforced in code that
  even a manual refresh cannot bypass
- On 429, exponential backoff 5 → 10 → 20 → 40 → 60 minutes, capped
- The cached value keeps showing throughout, marked stale

Codex polls every 60s, being a local file read.

A dimmed block with an amber dot means stale. Last-good readings are cached to
`%LOCALAPPDATA%\usage-tracker\last-good.json`, so losing the network shows
yesterday's numbers rather than blanks.

## Build

```powershell
cargo run              # run it
cargo test             # 64 tests
cargo run -- --probe   # print raw provider responses once, for diagnosis
```

Rust with `eframe`/`egui`. A background thread polls each provider on its own
interval and publishes an immutable snapshot through a mutex; the render loop
only reads that snapshot. No async runtime.

`egui` is built on the `glow` (OpenGL) backend rather than the default `wgpu`:
measured at 117 MB resident and an 11 MB binary against 278 MB and 19 MB, with
identical output.

`DOCK_DIAG=1` logs poll cadence, hover transitions and DPI facts to stderr.

## Credits

Provider marks are the trademarks of their respective owners, included here to
label their own quota. `assets/claude.svg` is from
[simple-icons](https://github.com/simple-icons/simple-icons) (CC0);
`assets/codex.svg` is the OpenAI mark from
[gilbarbara/logos](https://github.com/gilbarbara/logos).
