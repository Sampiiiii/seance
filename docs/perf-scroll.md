# Terminal Scroll Performance Scenario (120Hz ProMotion)

This document defines a repeatable scroll benchmark for macOS ProMotion displays, covering both local and SSH sessions.

## Goal

Validate smooth sustained trackpad scrolling and compare before/after changes using the same scenario.

Target acceptance:

- `presented fps >= 100`
- `ui cost p95 <= 8.3ms`
- `pacer flush/s <= display hz` (the pacer must never exceed one flush per display frame while `pacer req/s` stays high under load — the difference is coalesced work)
- `pacer defer/s` stays modest (<= `display hz / 2`); sustained growth means the target interval is too tight
- No regressions in alternate screen behavior, mouse-tracking wheel forwarding, scrollbar drag/absolute offset, or keyboard scroll commands.

## Baseline Run

1. Build and launch with expanded perf HUD:

```bash
make debug-run-perf-expanded
```

2. Confirm expanded HUD is visible in terminal pane.
3. Record these values before changes:

- `presented fps`
- `ui cost` (last/avg/p95)
- `term hz`
- `dirty rows`
- `rebuilt`
- `shape hits`
- `shape misses`
- `row cache hits`
- `row cache misses`
- `link deferred`
- `scroll batches`
- `pacer req/s`
- `pacer flush/s`
- `pacer defer/s`
- `pacer coalesced`
- `pacer target`

## Scenario A: Local High-Scrollback

1. Open a local session.
2. Generate enough output for deep scrollback, for example:

```bash
for i in {1..20000}; do echo "local-scroll-$i"; done
```

3. Perform sustained two-finger scroll for ~15 seconds (up and down).
4. Record the same metrics list from HUD.
5. Verify behavior:

- Shift+PageUp/PageDown, Shift+Home/End still work.
- Scrollbar thumb drag still maps to correct absolute offset.
- Clicking scrollbar track still jumps by absolute offset.

## Scenario B: SSH High-Scrollback

1. Connect to an SSH host with stable latency.
2. Generate output remotely:

```bash
for i in {1..20000}; do echo "ssh-scroll-$i"; done
```

3. Repeat the same sustained scroll sequence for ~15 seconds.
4. Record the same metrics list from HUD.
5. Verify behavior:

- Alternate screen apps (for example `vim`, `less`) do not use viewport scrollback.
- When mouse tracking is active (for example in TUI apps), wheel input is forwarded to remote app.

## Scenario C: Fullscreen + Live Resize

Validates that ProMotion opt-in holds in fullscreen and that live resize does
not leave stale rows below the prompt.

1. Open a local session and enter fullscreen (`Cmd+Ctrl+F`).
2. Confirm `display hz` on the HUD stabilizes around `~120` (within ~5% of
   the display max on ProMotion panels). If it stays pinned at 60, the macOS
   ProMotion opt-in has not applied — re-run once the window has been main
   for ~2 seconds, or exit/re-enter fullscreen.
3. While still in fullscreen, kick off a steady stream:

```bash
for i in {1..5000}; do echo "scenario-c-$i"; sleep 0.002; done
```

4. Exit fullscreen and drag the window corner to rapidly resize (grow + shrink
   cycles across ~5 seconds).
5. After each shrink, verify:

- No "dead content" rows remain below the prompt at stale y-positions.
- `rebuilt` on the HUD spikes to the new `visible rows` on the first frame
  after each resize, then returns to steady-state caching.
- `display hz` tracks the window's screen refresh (fullscreen on ProMotion
  stays > 100; a 60 Hz external display drops cleanly to ~60 without artifacts).
- `pacer target` adjusts within ~2 seconds of any fullscreen transition.

## Comparison Checklist

Use the same hardware, display mode, window size, and session setup for baseline and post-change runs.

For each scenario, compare:

- `presented fps` delta
- `ui cost p95` delta
- `term hz` delta
- `rebuilt` and `dirty rows` trends
- cache effectiveness (`shape hits/misses`, `row cache hits/misses`)
- `link deferred` during active scrolling and restoration when idle
- `scroll batches` growth during sustained input
- `pacer req/s` vs `pacer flush/s` — large request-to-flush gap (plus growing `pacer coalesced`) is the expected win signal: terminal publishes and input bursts get coalesced down to at most one present per display frame
- `pacer defer/s` — brief spikes during heavy bursts are fine; sustained growth indicates `target_interval` needs tuning

## Notes

- Run each scenario at least twice and keep the best stable run for comparison.
- If metrics regress, capture a short screen recording plus HUD values for investigation.
