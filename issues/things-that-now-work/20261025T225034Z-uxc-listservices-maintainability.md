Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 3934dd8dfa9c40453bce3aa6cb9dd67d7ad9e6cc

# Delete the dead maxResults range guard in ListServices: the crate's own front door already serves that refusal

_Filed: 2026-10-25T22:50:34Z_
_Operation: ListServices | Class: maintainability_

> _Note: this issue was identified against the code base at commit `3934dd8dfa9c40453bce3aa6cb9dd67d7ad9e6cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `maxResults` @range(1, 1000) refusal is implemented TWICE in this crate's own code, and the two copies have already drifted.

Copy 1 — `src/service.rs`'s `FrontDoor`: every modelled operation is served wrapped in it (`EmulatedService::new`), and `constraint_rejection` partitions each violation by HTTP binding, sending a `@httpQuery`-bound one (which `maxResults` is) to `member_constraint_rejection`, which renders the modelled `ValidationException` with the framework envelope (`N validation error(s) detected: Value '<v>' at 'maxResults' failed to satisfy constraint: ...`) plus `fieldList`.

Copy 2 — `src/ops/list_services.rs`: the handler declares `MAX_RESULTS_RANGE: 1..=1000` (a hand-restated copy of the model's `@range`) and re-checks it at the top of `handle`, answering through its own `invalid_member` helper with a DIFFERENT message shape (`"maxResults: Member must be between 1 and 1000, inclusive"`, no envelope, one-entry `fieldList`).

Copy 2 is unreachable: the front door refuses the request before `handle` runs, so no caller can ever see the handler's wording. The existing test suite proves it (see evidence) — it asserts the envelope form for every out-of-range value. The defect is drift risk: the dead copy restates a model bound and an error wording that nothing exercises, so a change to the model's range or to the refusal wording leaves a stale second rule in the handler, and `invalid_member` is dead with it. Deleting the guard and the now-unused `invalid_member` helper leaves every response byte-identical and every existing test passing.

## Evidence

Dead copy — services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/list_services.rs:242-257 (the comment itself admits the duplication):
```
        // The boundary validator already refuses a `maxResults` outside the
        // modelled range and a `nextToken` outside its length/pattern; this guard
        // keeps the page size an invariant of the handler itself and states the
        // refusal as the modelled `ValidationException`.
        if let Some(max_results) = input.max_results {
            if !MAX_RESULTS_RANGE.contains(&max_results) {
                return Err(invalid_member(
                    "maxResults",
                    format!(
                        "Member must be between {} and {}, inclusive",
```
with the restated bound at list_services.rs:103-105 (`const MAX_RESULTS_RANGE: std::ops::RangeInclusive<i32> = 1..=1000;`) and the only call site of `invalid_member` (list_services.rs:157-166) being that branch.

Live copy — services/service-com-amazonaws-uxc-awsaccountuxsetting/src/service.rs:299-305:
```
    let (body, uri): (Vec<Violation>, Vec<Violation>) = violations
        .into_iter()
        .partition(|violation| binds_to_body(O::INPUT_SCHEMA, &violation.path));
    if !body.is_empty() {
        return Some(front_door_rejection(svc, body));
    }
    Some(member_constraint_rejection(svc, protocol, uri))
```
and `binds_to_body` (service.rs:316-328) places `maxResults` in the `uri` bucket because the model binds it with `@httpQuery` (`smithy_query(query="operation", operation="ListServices")`: `"maxResults": {"httpQuery": "maxResults", "range": {"min": 1, "max": 1000}}`).

Proof the handler copy never runs — tests/all/member_constraint_field_list.rs:40 drives `const OUT_OF_RANGE_MAX_RESULTS: [i64; 7] = [0, -1, -7, -1000, 1001, 5000, 1_000_000];` and lines 251-256 assert
```
        assert_eq!(
            entries.len(),
            stated_count(case, &message),
```
where `stated_count` (lines 201-207) parses the LEADING integer of the envelope message and panics with `"no violation count in {message:?}"` otherwise. The handler's message starts with `maxResults:`, so this test would panic if the handler branch were reached; it passes, so the front door serves every row.

Test coverage that must stay unchanged after the deletion: tests/all/member_constraint_field_list.rs::a_member_constraint_refusal_reports_every_refused_member_in_the_field_list, tests/all/body_validation_refusals.rs::a_uri_bound_value_outside_its_constraint_keeps_the_modelled_code (lines 295-325), tests/all/account_customizations.rs::bad_page_inputs_are_refused (lines 378-413).

## Steps to Reproduce

1. Read services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/list_services.rs:242-257 — the handler re-checks the modelled `maxResults` @range and renders its own refusal via `invalid_member`.
2. Read src/service.rs:293-306 and 316-328 — `FrontDoor` already refuses that violation (a `@httpQuery`-bound member) before the operation runs, rendering the framework envelope plus `fieldList`.
3. GET /v1/services?maxResults=0 and observe the message is `1 validation error(s) detected: Value '0' at 'maxResults' failed to satisfy constraint: ...`, not the handler's `maxResults: Member must be between 1 and 1000, inclusive` — the handler branch is unreachable.
4. Confirm with tests/all/member_constraint_field_list.rs:201-207 + 251-256: `stated_count` panics on a message with no leading count, so the passing test proves the handler branch never executes.
5. Delete the guard at list_services.rs:246-257, the now-unused `invalid_member` helper (lines 155-166) and `MAX_RESULTS_RANGE` (lines 103-105); re-run the crate's suites — every response and every assertion is unchanged.
