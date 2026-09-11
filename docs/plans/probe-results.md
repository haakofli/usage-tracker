# Probe results

Captured 2026-09-11 on the target machine. Settles the items Task 1 of the
implementation plan listed as UNVERIFIED. One live request was made to the
Claude usage endpoint, via `cargo run -- --probe`.

## Claude

### Access token path

`.claudeAiOauth.accessToken` in `%USERPROFILE%\.claude\.credentials.json` —
as reported elsewhere. Token length 108.

Scopes present:

```
user:file_upload, user:inference, user:mcp_servers, user:profile,
user:sessions:claude_code
```

`user:profile` is present, so this account can call the endpoint. Confirmed by
the endpoint returning 200.

The parser does not hardcode this path: it prefers `claudeAiOauth` but falls
back to locating whichever object carries an `accessToken`, so a future
relocation of the block does not break the read.

### `utilization` scale — the important one

**It is a percentage, not a fraction.** The live response returned
`"utilization": 24.0` for a 24% session window, cross-checked against the
`limits[]` array in the same payload which reports `"percent": 24`.

The plan's scaffold shipped `UTILIZATION_IS_PERCENT = false`, which would have
under-reported by 100x. It also proposed a "treat anything above 1.0 as a
percentage" heuristic as a defence; that heuristic gets the common case right
but reads a genuine `0.5` (0.5%) as 50%.

`claude.rs::normalise` therefore divides by 100 unconditionally. If the API ever
switches to fractions this under-reports visibly (24% shown as 0%) rather than
silently overstating usage, which is the safer direction to fail in.
`keeps_sub_one_percent_readings_small` guards the regression.

### `resets_at` type

An **ISO-8601 string with microseconds and an explicit offset**, not an epoch
integer:

```
"2026-09-11T15:50:00.422605+00:00"
```

`chrono`'s `DateTime<Utc>` deserialiser handles this directly. The `RawReset`
untagged enum still accepts an epoch integer as a fallback, which costs nothing
and is what the Codex source uses.

### Window keys actually returned

Both the documented keys and a newer array shape are present:

- `five_hour` — populated (the 5-hour window)
- `seven_day` — populated (the weekly window)
- `seven_day_opus`, `seven_day_sonnet`, `seven_day_cowork` — all `null` on this
  account
- `limits[]` — an array of `{kind, group, percent, severity, resets_at, scope,
  is_active}` with `kind` values `session`, `weekly_all`, `weekly_scoped`.
  `percent` is an integer here.
- `extra_usage` — an object (disabled on this account), not the `{}` the plan
  showed
- `spend`, `member_dashboard_available`, `seven_day_breakdown` — present

v1 reads `five_hour` / `seven_day`. `limits[]` looks like the more canonical
future shape and is the obvious migration target if those keys are ever
retired; it is kept in the fixture to document the alternative.

### Fixture redaction

`tests/fixtures/claude_usage.json` contains no token and no account identifier.

The live response also carried several feature-bucket keys under
unreleased-sounding internal codenames. They are not read by the parser and
have been **omitted** from the committed fixture rather than preserved
verbatim — structural fidelity for the keys we consume, without checking
unannounced product names into the repo.

## Codex

Confirmed exactly as the plan described, with the nesting one level deeper than
the snippet implied — `rate_limits` sits under `payload`, alongside
`payload.info`, in a `type: "event_msg"` / `token_count` line:

```json
"rate_limits": {
  "limit_id": "codex", "limit_name": null,
  "primary":   {"used_percent": 0.0,  "window_minutes": 300,   "resets_at": 1789141883},
  "secondary": {"used_percent": 49.0, "window_minutes": 10080, "resets_at": 1789468915},
  "credits": {"has_credits": false, "unlimited": false, "balance": null},
  "individual_limit": null, "spend_control_reached": null,
  "plan_type": "team", "rate_limit_reached_type": null
}
```

- `primary` = 5-hour window (`window_minutes: 300`) — verified
- `secondary` = weekly window (`window_minutes: 10080`) — verified
- `used_percent` is 0–100 — verified
- `resets_at` is a unix epoch in seconds — verified

The deeper nesting vindicates the plan's `find_key` tree search over mirroring
the envelope. Two extra details worth recording:

- `primary`/`secondary` can be `null` individually, and the whole `rate_limits`
  object can be `null` on some lines. Both are skipped rather than parsed as
  zero, so a null window shows `—` instead of a false 0%.
- Rollout files are scanned newest-first, ten at most, reading lines bottom-up.
  Recent files carry 1–74 `rate_limits` lines each.

## Live values at probe time

Sanity check that both readers produce plausible, differing numbers:

| Provider | 5-hour | Weekly |
|---|---|---|
| Claude | 24% | 9% |
| Codex | 31% | 54% |
