Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 67bdaa95f371a43447a945b4dd8bda66772a6b0f

# The readiness-summary build/page/aggregate logic is implemented twice, once per summary handler

_Filed: 2026-10-25T05:02:25Z_
_Operation: GetCellReadinessSummary | Class: maintainability_

> _Note: this issue was identified against the code base at commit `67bdaa95f371a43447a945b4dd8bda66772a6b0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetCellReadinessSummary` and `GetRecoveryGroupReadinessSummary` each carry their own copy of the same three-step rule: build a `BTreeMap<String, ReadinessCheckSummary>` with `readiness = READINESS_UNKNOWN` keyed by check name, page it with `crate::state::page`, and answer with the all-UNKNOWN aggregate plus the page's records. The two copies differ only in the scope helper they call and the error enum they name. The crate explicitly centralises exactly this kind of shared rule in `state.rs` — including the membership half of this very rule, which the state module already shares precisely so the two summaries "cannot drift apart" — so the remaining per-check summary construction and aggregate-readiness decision being duplicated is the one part that can drift: changing the summary's readiness value, the key used for the cursor, or the aggregate in one handler silently leaves the other reporting the old rule. A shared state.rs helper taking the scope set and returning the summaries map (and the aggregate value) would leave every response byte and every existing test unchanged.

## Evidence

Copy 1 — src/ops/get_cell_readiness_summary.rs:175-205:
```
// One summary per readiness check that watches a resource scoped to this cell,
// keyed by check name so the page order — and therefore the cursor — is the same
// on every call.
let summaries: ::std::collections::BTreeMap<String, ReadinessCheckSummary> = state
    .readiness_check_names_in_cell(&input.cell_name)
    .into_iter()
    .map(|name| { (name.clone(), ReadinessCheckSummary { readiness: Some(crate::state::READINESS_UNKNOWN.to_string()), readiness_check_name: Some(name) }) })
    .collect();
let page = crate::state::page(&summaries, input.max_results, input.next_token.as_deref()) ...
// The aggregate of parts that are all UNKNOWN is UNKNOWN ...
readiness: Some(crate::state::READINESS_UNKNOWN.to_string()),
readiness_checks: Some(page.records.into_iter().cloned().collect()),
```

Copy 2 — src/ops/get_recovery_group_readiness_summary.rs:180-212: byte-for-byte the same construction, page call, aggregate comment and output assembly, with `readiness_check_names_in_recovery_group(&input.recovery_group_name)` in place of `readiness_check_names_in_cell(...)`.

The crate states the one-implementation rule it is breaking here: src/state.rs:5-9 "Besides the state struct this module holds the few helpers the resource-set, readiness-check, cell and recovery-group handlers share ... so the operation files agree on one implementation of each instead of carrying a copy apiece that can drift apart"; and for this exact pair, src/state.rs:580-583 "the rule a cell's and a recovery group's summary both follow, which is why they share this one implementation and differ only in the scope set" (said of `readiness_check_names_in_scopes`, the membership half — the summary half was copied instead).

Tests that pin the responses a de-duplication must not change: tests/all/account_gate_and_not_found.rs:991 `a_cell_readiness_summary_reports_the_checks_scoped_to_that_cell` (and :885 `a_stored_checks_readiness_is_unknown_until_an_audit_runs`, :840 token refusal, :1093-1118 maxResults/nextToken walk), tests/all/recovery_group_readiness_summary.rs:187 `a_recovery_group_summary_names_the_checks_scoped_to_that_group`, :259 `the_summary_pages_over_the_scoped_checks`, :317 `the_summary_refuses_a_token_it_did_not_mint`, :353 `a_summary_of_a_group_that_does_not_exist_is_not_found`, :430 `reading_the_summary_changes_nothing`.

## Steps to Reproduce

1. Read src/ops/get_cell_readiness_summary.rs:175-205 and src/ops/get_recovery_group_readiness_summary.rs:180-212 side by side: the summaries map construction, the `crate::state::page` call and the all-UNKNOWN aggregate are the same code twice
2. Read src/state.rs:5-9 and src/state.rs:577-583: the crate states that the rule both summaries follow is shared in one place so the two cannot drift, yet only the membership half is shared
3. Change the per-check summary value (e.g. the key used for the cursor) in one handler and observe the other keeps the old rule; `cargo test -p service-com-amazonaws-route53recoveryreadiness-route53recoveryreadiness` only fails for the handler that was changed
