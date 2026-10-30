Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: bdfeee3e6512ca319f6cb10a0fb27e2039f01e12

# A GetProfile request naming only `period` serves every aggregated profile the group holds combined, not the latest aggregated profile

_Filed: 2026-10-29T15:32:14Z_
_Operation: GetProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `bdfeee3e6512ca319f6cb10a0fb27e2039f01e12`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own Smithy documentation fixes what a request that names only `period` gets back: "If you specify only `period`, the latest aggregated profile is returned." The handler does the opposite — it turns a period-alone request into a window with BOTH bounds open and then serves the combination of EVERY aggregated profile the group retains, whatever window each one sits in.

The path: `requested_window(&input)` (src/ops/get_profile.rs:1149) refuses only a LONE bound (lines 199-211) and otherwise defers to `crate::state::requested_window`, which for `(None, None, Some(period))` returns `start: None, end: None` — "A length alone (and no member at all) names no bound" (src/state.rs:156-157) — and GetProfile's wrapper then throws the span away, returning `Ok((window.start, window.end))` (src/ops/get_profile.rs:227). Those two `None`s are handed to `group.aggregated_profiles_in(start, end, now)` (src/ops/get_profile.rs:1170-1174), whose bound tests are `lower.is_none_or(..) && upper.is_none_or(..)` (src/state.rs:1437-1446), so an absent bound filters nothing and every aggregated profile of the group is selected. `served_aggregate` then folds all of them into one document (src/ops/get_profile.rs:589-608).

Two consequences, both wrong against the quoted sentence:
* a group holding aggregated profiles in several windows answers a period-alone request with all of them combined — frames merged and sample counts summed across windows the caller did not ask about — instead of the latest aggregated profile;
* the value of `period` has no effect on which data is served (only on the 7-day length check), so `?period=PT5M` and `?period=P7D` return byte-identical bodies on such a group.

This is not a request for new machinery: the crate already derives a group's most recent aggregated profile from the same stored receipts (`ProfilingGroup::latest_aggregated_profile`, src/state.rs:1049, used for `profilingStatus.latestAggregatedProfile`) and already aligns windows with `aggregation_window_start` (src/state.rs:207). The handler's own doc comment even states the intended rule, which the code under it breaks.

## Evidence

The model sentence — `smithy_query(query="doc", shape="GetProfile")`:
```
Specify the time range for the requested aggregated profile using 1 or 2 of the following parameters: startTime, endTime, period. The maximum time range allowed is 7 days. If you specify all 3 parameters, an exception is thrown. If you specify only period, the latest aggregated profile is returned.
```

The handler's own claim about that case — src/ops/get_profile.rs:173-176:
```
/// you specify all 3 parameters, an exception is thrown. If you specify only period, the
/// latest aggregated profile is returned." So `period` completes whichever single bound
/// was given, and on its own leaves both bounds open — which is how this operation selects
/// the LATEST profile — and the range the request ends up naming is no longer than
```

What "both bounds open" actually selects — src/state.rs:156-157 (period alone names no bound):
```
        // A length alone (and no member at all) names no bound.
        (start, end, _) => (start, end),
```
src/ops/get_profile.rs:227 (the span is dropped, only the two `None` bounds survive):
```
    Ok((window.start, window.end))
```
src/ops/get_profile.rs:1170-1174 (the open window is the selection):
```
            let profiles: Vec<crate::state::AgentProfile> = group
                .aggregated_profiles_in(start, end, crate::state::now())
                .into_iter()
                .cloned()
                .collect();
```
src/state.rs:1440-1446 (an absent bound filters nothing — EVERY aggregated profile):
```
            .filter(|profile| {
                let received = instant_key(profile.received_at);
                profile.is_aggregated(at)
                    && lower.is_none_or(|s| received >= s)
                    && upper.is_none_or(|e| received <= e)
            })
```
src/ops/get_profile.rs:589-601 (all of them are then combined into one document):
```
    let mut combination: Option<Element> = None;
    for (_, _, document) in &read {
        combination = Some(match combination {
            None => document.clone(),
            Some(earlier) => match combined(&earlier, document, DOCUMENT_NESTING_LIMIT, true) {
```

The helper the crate already has for "the latest aggregated profile" — src/state.rs:1049:
```
    pub fn latest_aggregated_profile(&self, at: DateTime) -> Option<AggregatedProfileTime> {
```

Test coverage: every existing period-alone case runs against a group holding exactly ONE profile, so none of them can see this — tests/all/profile_representations.rs:153-154:
```
            // "If you specify only period, the latest aggregated profile is returned."
            ("the period alone", format!("?period={period}"), 200),
```
and tests/all/profiling_group_lifecycle.rs:1382-1386:
```
        (
            "period alone (the latest profile)",
            "?period=PT1H".to_string(),
            200,
        ),
```
while every multi-profile combination case names an explicit pair of bounds — tests/all/aggregated_profile_combination.rs:334-337:
```
fn window_query() -> String {
    format!(
        "?startTime={}&endTime={}",
```
So no existing test pins the period-alone answer for a group holding profiles in more than one window.

## Steps to Reproduce

1. Create a profiling group and post THREE agent profiles whose receipt times sit in three different five-minute aggregation windows (e.g. 00:00, 00:10 and 00:20 of a past day, as tests/all/profile_times.rs::place_receipts does), each document recording a different frame with a sample count, and let their aggregation fall due.
2. GET /profilingGroups/<name>/profile?period=PT5M — the documented way to ask for the latest aggregated profile.
3. Observe the served body: it is the combination of all three profiles (every frame of all three present, counts summed), not the aggregated profile of the latest window, although the operation's doc says "If you specify only period, the latest aggregated profile is returned".
4. Repeat with ?period=P7D: the body is byte-identical, showing the named period affects nothing but the 7-day length check.
5. Read src/ops/get_profile.rs:173-176 — the handler's own comment claims a period alone "is how this operation selects the LATEST profile" — then src/ops/get_profile.rs:227 and src/state.rs:1440-1446, where both-bounds-open selects every aggregated profile instead.
