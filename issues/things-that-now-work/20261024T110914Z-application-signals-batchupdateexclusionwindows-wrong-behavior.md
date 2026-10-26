Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ee0f6e80c5f72b72cdc0a537b4cc8caa49bfe93a

# Default an added exclusion window's omitted StartTime to the time of the call

_Filed: 2026-10-24T11:09:14Z_
_Operation: BatchUpdateExclusionWindows | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ee0f6e80c5f72b72cdc0a537b4cc8caa49bfe93a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ExclusionWindow.StartTime` is optional and the model documents its default: "The start of the SLO time window exclusion. Defaults to current time if not specified."

Expected: adding a window through `AddExclusionWindows` without `StartTime` stores a window whose `StartTime` is the instant the add was served, so a following `ListServiceLevelObjectiveExclusionWindows` reports a concrete `StartTime` for it (and the window actually excludes a period anchored at that instant).

Actual: the handler stores the window exactly as sent — `update_exclusion_windows` pushes `window.clone()` with no server-side stamping — so the stored and listed window carries no `StartTime` at all, and nothing in any response tells the caller when the exclusion begins. A client that adds a window with only `Window` set reads back a window with `StartTime` absent where the real service reports the create instant. The omission also leaks into window identity: `same_exclusion_window` compares `start_time` as an `Option`, so removal must re-send the member as absent, whereas on the real service the stored window carries the defaulted timestamp.

## Evidence

Spec: `smithy_query(query="shape", shape="ExclusionWindow")` — member `StartTime`: "The start of the SLO time window exclusion. Defaults to current time if not specified." `BatchUpdateExclusionWindows` is the only writer of this member, and `ListServiceLevelObjectiveExclusionWindows` the only reader.

Handler/state: `src/ops/batch_update_exclusion_windows.rs:155-192` never reads a clock (no `SystemTime::now()` in the handler) and passes the request's windows straight to `crate::state::update_exclusion_windows`; `src/state.rs:1186-1197` pushes `window.clone()` verbatim. A repo-wide grep for `start_time` in this crate's `src/` shows the member is only ever compared (`src/state.rs:1224`), never assigned.

Tests: `tests/all/exclusion_windows.rs:174-227, 234-267` deliberately spans rows with "an omitted StartTime" and asserts the stored set equals "the configuration sent", i.e. the current no-default behavior is pinned; no test asserts a defaulted StartTime.

## Steps to Reproduce

1. POST /slo to create an SLO named "window-default".
2. PATCH /exclusion-windows with {"SloIds":["window-default"],"AddExclusionWindows":[{"Window":{"DurationUnit":"HOUR","Duration":6},"Reason":"migration"}]} — note StartTime is omitted.
3. GET /slo/window-default/exclusion-windows and inspect the reported window: StartTime is absent; expected StartTime to carry the instant the add was served.
