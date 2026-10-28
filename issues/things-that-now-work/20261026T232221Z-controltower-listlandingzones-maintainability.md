Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# The index-token pagination rule is implemented six times across this crate's list handlers

_Filed: 2026-10-26T23:22:21Z_
_Operation: ListLandingZones | Class: maintainability_

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every list operation of this crate paginates by the same rule: an absent-or-empty `nextToken` starts at 0, any other token is parsed as a `usize` index and refused with `ValidationException` unless it is `<= len`, `maxResults` is clamped with `.max(1)` (or defaults to the whole remainder), `end = start.saturating_add(limit).min(len)`, and `nextToken` is `(end < len).then(|| end.to_string())`. That computation is written out six times — once per list handler — so the copies can drift from each other (one already differs: `list_landing_zones.rs` and `list_enabled_baselines.rs` carry a `.max(1)` floor comment the operation-log handlers omit). The crate has already centralised its sibling list rule, the filter matching, into `crate::state` (`filter_set`/`selected`/`selected_optional`, src/state.rs:3048-3118) for exactly this reason; the pagination span is the remaining duplicate. Extracting it changes no response and no existing test.

## Evidence

Copy 1 — services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/list_landing_zones.rs:174-199, 210:
```
let start = match input.next_token.as_deref() {
    None | Some("") => 0usize,
    Some(token) => match token.parse::<usize>() {
        Ok(index) if index <= arns.len() => index,
        _ => { return Err(ListLandingZonesErrorModeled::ValidationException(...)) }
    },
};
let limit = input.max_results.map_or(arns.len(), |max| max.max(1) as usize);
let end = start.saturating_add(limit).min(arns.len());
...
next_token: (end < arns.len()).then(|| end.to_string()),
```

Copy 2 — src/ops/list_enabled_baselines.rs:243-268, 302:
```
let start = match input.next_token.as_deref() {
    None | Some("") => 0usize,
    Some(token) => match token.parse::<usize>() {
        Ok(index) if index <= enabled.len() => index,
...
let limit = input.max_results.map_or(enabled.len(), |max| max.max(1) as usize);
let end = start.saturating_add(limit).min(enabled.len());
...
next_token: (end < enabled.len()).then(|| end.to_string()),
```

The same span appears again verbatim modulo the collection name and the message at src/ops/list_enabled_controls.rs:246-269/298, src/ops/list_control_operations.rs:230-253/260, src/ops/list_landing_zone_operations.rs:227-250/257 and src/ops/list_baselines.rs:175-200/217.

Precedent inside this same crate for extracting a shared list rule — src/state.rs:60-65:
```
//! The LIST operations over these stores all narrow their listing the same way, so the
//! rule lives here once rather than in each handler: [`Filter`] is one `filter` member as
//! a handler matches it, [`filter_set`] turns a sent member into that set ...
```

Tests that pin the behaviour and must keep passing unchanged: tests/all/list_baselines.rs:199 `every_page_size_walks_the_same_listing`, :299 `a_token_the_service_never_handed_out_is_refused`, :334 `an_empty_token_lists_from_the_start_and_the_end_index_lists_nothing`; tests/all/list_enabled_baselines.rs:533; tests/all/enabled_control_lifecycle.rs:674; tests/all/control_operations.rs:463; tests/all/landing_zone_operations.rs:576; tests/all/landing_zone_lifecycle.rs:1189.

## Steps to Reproduce

1. Open the six list handlers under services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/ (list_landing_zones.rs, list_landing_zone_operations.rs, list_control_operations.rs, list_enabled_controls.rs, list_enabled_baselines.rs, list_baselines.rs).
2. Compare the `let start = match input.next_token.as_deref()` → `let limit = ...` → `let end = start.saturating_add(limit).min(..)` → `next_token: (end < ..).then(..)` span in each: the logic is identical and only the collection name and the refusal message differ.
3. Note that the sibling list rule (filter matching) was already extracted once into src/state.rs:3048-3118 with the module header at src/state.rs:60-65 giving the reason, while this span was not.
