Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 63dbcd68d6fd7b54c4f6138d45d0f4dfd539b7cc

# GetProfile serves a lone startTime (or lone endTime), which the model says must be accompanied by period or the other bound — and which bypasses the 7-day maximum

_Filed: 2026-10-28T16:42:31Z_
_Operation: GetProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `63dbcd68d6fd7b54c4f6138d45d0f4dfd539b7cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetProfile`'s window members are modelled with a mutual-companion rule: a `startTime` must be accompanied by `period` or `endTime`, and an `endTime` must be accompanied by `period` or `startTime`. The only single member a request may name on its own is `period` ("If you specify only period, the latest aggregated profile is returned").

The handler implements only ONE half of the trio rule. `crate::state::requested_window` refuses the all-three case and completes a single bound from a named `period`, but its final catch-all arm passes a LONE bound straight through as a half-open window. `GetProfile`'s own wrapper (`get_profile.rs::requested_window`) adds nothing but the length check, so:

* `GET /profilingGroups/{name}/profile?startTime=2000-01-01T00:00:00Z` answers 200 (or the no-data 404) instead of refusing the request with the operation's modelled `ValidationException`;
* `GET /profilingGroups/{name}/profile?endTime=2099-01-01T00:00:00Z` likewise.

Two consequences:

1. A request the model documents as invalid is served, so a client that forgets the companion member is never told.
2. It defeats the 7-day limit this same handler enforces everywhere else. `validate_window_length` bounds a window only when it has BOTH bounds, or when the request named a `span`; a lone bound has neither, so it returns `Ok(())` and `latest_profile_in(Some(start), None, now)` selects over an unbounded range — exactly the "maximum time range allowed is 7 days" rule that `MAX_TIME_RANGE` exists to apply.

Scope note for a fixer: the fix belongs in `get_profile.rs`'s own `requested_window` wrapper (beside `MAX_TIME_RANGE`, which is already held per-operation), NOT in the shared `crate::state::requested_window`. `BatchGetFrameMetricData` is the other caller of the shared helper and its `startTime`/`endTime` docs carry no companion rule (`smithy_query(query="doc", shape="BatchGetFrameMetricDataRequest", member="startTime")` is only the ISO-8601 sentence), and that handler deliberately completes an open side itself.

## Evidence

Smithy, `GetProfileRequest$startTime` (`smithy_query(query="doc", shape="GetProfileRequest", member="startTime")`):
> "If you specify `startTime`, then you must also specify `period` or `endTime`, but not both."

Smithy, `GetProfileRequest$endTime` (`smithy_query(query="doc", shape="GetProfileRequest", member="endTime")`):
> "If you specify `endTime`, then you must also specify `period` or `startTime`, but not both."

Smithy, the `GetProfile` operation doc:
> "Specify the time range for the requested aggregated profile using 1 or 2 of the following parameters: startTime, endTime, period. The maximum time range allowed is 7 days. If you specify all 3 parameters, an exception is thrown. If you specify only period, the latest aggregated profile is returned."

The code that lets a lone bound through — src/state.rs:144-152:
```
    let (start, end) = match (start_time, end_time, span) {
        (Some(_), Some(_), Some(_)) => {
            return Err(OVER_DETERMINED_WINDOW_MESSAGE.to_string());
        }
        (Some(start), None, Some(span)) => (Some(start), Some(shifted(start, span_nanos(span)))),
        (None, Some(end), Some(span)) => (Some(shifted(end, -span_nanos(span))), Some(end)),
        // A length alone (and no member at all) names no bound.
        (start, end, _) => (start, end),
    };
```
The crate's own doc for that helper quotes only the all-three half of the rule — src/state.rs:116-121:
```
/// The rule, as the `GetProfile` doc states it: "Specify the time range for the requested
/// aggregated profile using 1 or 2 of the following parameters: startTime, endTime,
/// period. … If you specify all 3 parameters, an exception is thrown." So a length
/// completes whichever single bound was given, all three together are refused
```

GetProfile's wrapper adds no companion check — src/ops/get_profile.rs:167-181:
```
    let window = crate::state::requested_window(
        input.start_time,
        input.end_time,
        input.period.as_deref(),
    )
    .map_err(crate::scaffolded::Scaffolded::validation)?;
    crate::state::validate_window_length(
        &window,
        input.period.as_deref(),
        MAX_TIME_RANGE,
        MAX_TIME_RANGE_TEXT,
        TIME_RANGE_SUBJECT,
    )
```
and the lone bound is then used as the selection window — src/ops/get_profile.rs:716 and :737-738:
```
        let (start, end) = requested_window(&input)?;
...
            group
                .latest_profile_in(start, end, crate::state::now())
```

Why the 7-day rule does not catch it — src/state.rs:2089-2098:
```
    match (window.start, window.end) {
        (Some(start), Some(end)) => validate_bounds_length(start, end, maximum, limit, subject),
        // No pair of bounds, so the only length the request stated is the `period` itself,
        _ => match (window.span, period) {
            (Some(span), Some(period)) if span > maximum => {
                Err(window_too_long_message(subject, limit, period))
            }
            _ => Ok(()),
        },
    }
```
against the limit the same file declares — src/ops/get_profile.rs:128-134:
```
/// The LONGEST time range this operation serves an aggregated profile over. Not an
/// emulator choice: the operation's own doc states it — "… The maximum time range allowed is 7 days"
const MAX_TIME_RANGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
```

Test coverage (both pin the current acceptance under a NARROWER reason than the model's rule, so both need a row update as part of the fix):
* tests/all/get_profile_window_length.rs:367-380 — rows `"a start alone, which names no length"` (`?startTime=2000-01-01T00:00:00Z`) and `"an end alone, which names no length"` (`?endTime=2099-01-01T00:00:00Z`), both `Expected::Allowed`, asserted at :442-449 as "a range the length rule allows must not be refused" — i.e. the stated reason is only the LENGTH rule, not the companion rule.
* tests/all/profiling_group_lifecycle.rs:1325-1338 — the comment claims "Rows: each way the operation's doc allows a window to be named" and then lists `("a start before the profile", "?startTime=2000-01-01T00:00:00Z", 200)`, a naming the quoted member doc does NOT allow.
* tests/all/get_profile_interval.rs:67 — `Window::Open` is documented as "A request that names no pair at all: one open bound, or a period on its own" and expected `Accepted` because "the interval rule has nothing to refuse" (:95-97), again a reason about the interval rule only.

## Steps to Reproduce

1. Boot the emulator and point a codeguruprofiler client at it.
2. CreateProfilingGroup(profilingGroupName="pg-window", clientToken=<uuid>, agentOrchestrationConfig={"profilingEnabled": true}).
3. PostAgentProfile(profilingGroupName="pg-window", agentProfile=<valid profile JSON carrying a `start` member>, contentType="application/json") and wait for the aggregation pass (crate::state::AGGREGATION_PASS_MILLIS past the 5-minute window end).
4. GET /profilingGroups/pg-window/profile?startTime=2000-01-01T00:00:00Z — i.e. GetProfile(profilingGroupName="pg-window", startTime="2000-01-01T00:00:00Z") with no endTime and no period. The emulator answers 200 with the profile; the model's `startTime` doc requires `period` or `endTime` alongside it, so this is a request that should be refused with 400 ValidationException.
5. Repeat with only endTime: GET /profilingGroups/pg-window/profile?endTime=2099-01-01T00:00:00Z — also served.
6. Confirm the knock-on effect on the 7-day rule: GET /profilingGroups/pg-window/profile?startTime=2000-01-01T00:00:00Z&endTime=2099-01-01T00:00:00Z is correctly refused with "the time range for the requested aggregated profile must be at most P7D long, got …", while dropping either bound makes the same (far wider) range succeed.
