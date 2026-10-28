Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 19fca1009697525153c46de34cf9f84b5fd316c3

# GetProfile serves a window wider than the documented 7-day maximum instead of refusing it

_Filed: 2026-10-27T19:00:42Z_
_Operation: GetProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `19fca1009697525153c46de34cf9f84b5fd316c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The GetProfile model doc states the window rules the operation enforces on `startTime`/`endTime`/`period`, and the handler implements only two of the three: it refuses a request naming all three members, and it refuses a pair of bounds that is not an interval. The THIRD rule in the same sentence — "The maximum time range allowed is 7 days" — is never checked, so a request naming a range of any width (e.g. `startTime` and `endTime` two months apart, or `startTime` plus `period=P30D`) is accepted and answered from whatever profile falls in it, rather than refused with the operation's modelled `ValidationException` ("The parameter is not valid").

The window is computed by `requested_window` (get_profile.rs:137-153), which delegates to `crate::state::requested_window` (state.rs:128-161). That shared reading applies the over-determination rule and `validate_interval`, and bounds nothing: there is no length check on the window it returns, and the handler adds none after it (get_profile.rs:673).

The crate already has the exact shape of the missing check in a sibling handler that reads the SAME trio through the SAME helper: `BatchGetFrameMetricData` completes the window and then refuses it when it is longer than the maximum ITS member doc states (`MAX_PERIOD` = PT24H), naming both completed bounds. Nothing in the crate justifies GetProfile diverging — the only mention of the 7-day rule anywhere is the sentence quoted into the handler's own module header, and the crate's period table stops at exactly `P7D`, calling it "the 7-day maximum range the operation's doc names".

(Distinct from the already-fixed GetProfile validation gap, which was about a reversed or zero-width window: that rule is now enforced; this is the window's WIDTH.)

## Evidence

Smithy, the rule and the number — `smithy_query(query="doc", shape="GetProfile")`:
```
Specify the time range for the requested aggregated profile using 1 or 2 of the following parameters: startTime,
endTime, period. The maximum time range allowed is 7 days. If you specify all 3 parameters,
an exception is thrown. If you specify only period, the latest aggregated profile is returned.
```
The same sentence is quoted into the handler itself — src/ops/get_profile.rs:54-56:
```
//! Specify the time range for the requested aggregated profile using 1 or 2 of the
//! following parameters: startTime, endTime, period. The maximum time range allowed is 7
//! days. If you specify all 3 parameters, an exception is thrown. If you specify only
```
The handler's whole window handling — src/ops/get_profile.rs:673:
```
        let (start, end) = requested_window(&input)?;
```
and the helper it calls, src/ops/get_profile.rs:146-152:
```
    let window = crate::state::requested_window(
        input.start_time,
        input.end_time,
        input.period.as_deref(),
    )
    .map_err(crate::scaffolded::Scaffolded::validation)?;
    Ok((window.start, window.end))
```
which bounds nothing — src/state.rs:144-161 refuses only all-three-members and a non-interval pair:
```
    let (start, end) = match (start_time, end_time, span) {
        (Some(_), Some(_), Some(_)) => {
            return Err(OVER_DETERMINED_WINDOW_MESSAGE.to_string());
        }
...
    if let (Some(start), Some(end)) = (start, end) {
        validate_interval(start, end)?;
    }
    Ok(RequestedWindow { start, end, span })
```
The sibling that DOES enforce its own documented maximum over the same helper — src/ops/batch_get_frame_metric_data.rs:139-146 and :226-229:
```
const MAX_PERIOD: Duration = Duration::from_secs(24 * 60 * 60);
...
    let longest = crate::state::shifted(start, crate::state::span_nanos(MAX_PERIOD));
    if crate::state::instant_key(end) > crate::state::instant_key(longest) {
        return Err(validation(period_too_long_message(start, end)));
    }
```
(its member doc, `smithy_query(query="doc", shape="BatchGetFrameMetricDataRequest", member="period")`: "The maximum period duration is one day (PT24H or P1D).")

No comment, test or doc in the crate justifies the divergence. `grep -n "7 days|maximum time range"` over the crate matches only src/ops/get_profile.rs:55. The crate's period table acknowledges the rule rather than waiving it — tests/all/profile_representations.rs:81-94:
```
/// The rows span the three periods this service aggregates at (PT5M, PT1H, P1D), the
/// member doc's own example, durations between and below them, a sub-second one, a
/// multi-field one, and the 7-day maximum range the operation's doc names.
...
    ("P7D", 604_800_000),
```
Test coverage that would move: tests/all/get_profile_interval.rs:37-39 uses a 99-year window purely as a convenience, with no rule stated for it:
```
/// A window wide enough to hold any instant these cases place a profile at, used to show
/// that the group serves a profile over a range that IS an interval.
const WIDE_WINDOW: &str = "?startTime=2000-01-01T00:00:00Z&endTime=2099-01-01T00:00:00Z";
```

## Steps to Reproduce

1. Boot the emulator and `CreateProfilingGroup(profilingGroupName="wide")`, then `PostAgentProfile` a JSON profile against it and wait for the aggregation window to fall due.
2. Call `GET /profilingGroups/wide/profile?startTime=2026-01-01T00:00:00Z&endTime=2026-03-01T00:00:00Z` (a 59-day range) — the request is served (200, or 404 for an empty range) instead of being refused with 400 `ValidationException`, although the operation's doc says "The maximum time range allowed is 7 days".
3. Call `GET /profilingGroups/wide/profile?startTime=2026-01-01T00:00:00Z&period=P30D` — the completed 30-day window is likewise accepted.
4. Compare with the sibling rule the crate already implements: `POST /profilingGroups/wide/frames/-/metrics?startTime=2026-01-01T00:00:00Z&period=P30D` is refused 400 with the message from src/ops/batch_get_frame_metric_data.rs:151-158, because that operation bounds its completed period.
5. Read src/ops/get_profile.rs:673 and src/state.rs:128-161 — no length bound exists on the GetProfile path.
