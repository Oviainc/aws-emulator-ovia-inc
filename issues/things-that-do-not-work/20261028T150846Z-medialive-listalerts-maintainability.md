Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# The documented-StateFilter membership rule is open-coded in all three alert listings, and the copies have already drifted

_Filed: 2026-10-28T15:08:45Z_
_Operation: ListAlerts | Class: maintainability_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

state/alerts.rs exists precisely so the three alert surfaces of this service cannot disagree about the alert vocabulary: it owns the state names, the documented `StateFilter` value set (`ALERT_STATE_FILTERS`) and the admission predicate (`alert_admitted_by`), and each resource slice aliases that one set. The REFUSAL for a value outside the set is not centralized: the same branch ("if the request carried a filter and the documented set does not contain it, answer this operation's BadRequestException naming the member, the accepted set and the offending value") is written out once per listing in ListAlerts, ListClusterAlerts and ListMultiplexAlerts, over three aliases of one constant.

The copies have already drifted: two of them format the offending value bare (`got {filter}`), the third quotes it (`got {filter:?}`), so the three surfaces report the same refusal differently — exactly the divergence the alerts module says it exists to prevent. The crate already demonstrates the centralized shape for a cross-listing rule: `crate::state::list_page_size` returns `Err(message)` and every paginated handler wraps that one message in its own modelled error.

A fix can state the membership rule once beside `ALERT_STATE_FILTERS` in state/alerts.rs and have each of the three handlers call it while keeping its own message text, so every response stays byte-identical and no test changes.

## Evidence

The module that owns the vocabulary says the surfaces must not disagree — services/service-com-amazonaws-medialive-medialive/src/state/alerts.rs:17-21:
```
//! So the state names, the documented filter set, the state-from-cleared-instant
//! derivation and the filter predicate live here ONCE, and each resource's own
//! slice ([`super::clusters`], [`super::multiplexes`]) delegates to them: two
//! alert surfaces of one service cannot then disagree about what `SET` means,
//! which filter values are documented, or which alerts a filter admits.
```
and all three aliases are that one set: src/state/channel_alerts.rs:69 `pub const CHANNEL_ALERT_FILTERS: [&str; 3] = super::alerts::ALERT_STATE_FILTERS;`, src/state/clusters.rs:110 `pub const CLUSTER_ALERT_FILTERS: [&str; 3] = super::alerts::ALERT_STATE_FILTERS;`, src/state/multiplexes.rs:164 `pub const MULTIPLEX_ALERT_STATE_FILTERS: [&str; 3] = super::alerts::ALERT_STATE_FILTERS;`.

Copy 1 — src/ops/list_alerts.rs:175-182:
```
        if let Some(filter) = state_filter {
            if !crate::state::CHANNEL_ALERT_FILTERS.contains(&filter) {
                return Err(bad_request(&format!(
                    "stateFilter must be one of {}, got {filter}",
                    crate::state::CHANNEL_ALERT_FILTERS.join(", ")
                )));
```
Copy 2 — src/ops/list_cluster_alerts.rs:177-184:
```
        if let Some(filter) = state_filter {
            if !crate::state::CLUSTER_ALERT_FILTERS.contains(&filter) {
                return Err(bad_request(&format!(
                    "stateFilter must be one of {}, got {filter}",
                    crate::state::CLUSTER_ALERT_FILTERS.join(", ")
                )));
```
Copy 3 (drifted wording) — src/ops/list_multiplex_alerts.rs:175-182:
```
        if let Some(filter) = state_filter {
            if !crate::state::MULTIPLEX_ALERT_STATE_FILTERS.contains(&filter) {
                return Err(bad_request(format!(
                    "stateFilter must be one of {}, got {filter:?}",
                    crate::state::MULTIPLEX_ALERT_STATE_FILTERS.join(", ")
                )));
```
The centralized counterpart the crate already uses for a cross-listing rule — src/state.rs:1158-1168 (`list_page_size`, "every paginated operation of this service cuts its ... exactly as the operations wrapping [`bounded_length`] and ...").

Tests that cover these three refusals do not pin the exact sentence, only that it names the member and the value: tests/all/channel_alerts.rs:562-589 (`a_state_filter_outside_the_documented_values_is_refused`, asserting `message.contains("stateFilter") && message.contains(case)`), tests/all/cluster_alerts.rs:724-726 (the unknown/lowercase/two-at-once rows), tests/all/multiplex_alerts.rs:909-932 (asserting `message.contains("stateFilter")`).

## Steps to Reproduce

1. Read src/state/alerts.rs:17-44 — the documented StateFilter set and the admission predicate are centralized there, with the module doc stating that two alert surfaces must not disagree about which filter values are documented.
2. Read src/ops/list_alerts.rs:175-182, src/ops/list_cluster_alerts.rs:177-184 and src/ops/list_multiplex_alerts.rs:175-182 — the membership check and its refusal are hand-copied three times over three aliases of that one set.
3. Compare the three format strings: two end `got {filter}`, the third `got {filter:?}` — the copies have already drifted, so a caller gets the offending value quoted on one alert surface and bare on the other two.
4. Confirm the suites named above assert only that the message mentions `stateFilter` and the offending value, so stating the rule once (each handler keeping its own message and its own modelled error) leaves every response and every test unchanged.
