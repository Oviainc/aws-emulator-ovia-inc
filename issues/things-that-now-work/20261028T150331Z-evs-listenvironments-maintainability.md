Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 4db25e6bf87f4906c725859fd3714c6300682a7e

# The page-size default-and-clamp computation is duplicated verbatim in all five paginated handlers

_Filed: 2026-10-28T15:03:31Z_
_Operation: ListEnvironments | Class: maintainability_

> _Note: this issue was identified against the code base at commit `4db25e6bf87f4906c725859fd3714c6300682a7e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `@paginated` operation of this crate computes its page bound with the same four-line expression: default the absent `maxResults` to `crate::state::MAX_PAGE_SIZE`, then clamp it into `1..=MAX_PAGE_SIZE` and cast to `usize`. The expression is byte-identical in all five handlers (ListEnvironments, ListEnvironmentHosts, ListEnvironmentConnectors, ListEnvironmentVlans, ListVmEntitlements), so the one pagination rule it encodes lives in five places and the copies can drift — exactly the situation the already-fixed `distinct_vm_ids` duplication issue (20261028T115144Z-evs-createentitlement-maintainability) was filed for, where the rule was moved next to the identity it derives from in `crate::state`.

Stating it once in `crate::state` beside `MAX_PAGE_SIZE` (the constant all five already name) and calling it from each handler changes no response and no test: the computed value is identical for every input, including an absent `maxResults` and the modelled 1 and 100 bounds.

## Evidence

Copy 1 — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/list_environments.rs:175-182:
```
        // "If you specify MaxResults in the request, the response includes information
        // up to the limit specified"; absent, the page is bounded by the top of the
        // modelled range. Clamped here as well, so the bound holds locally and not only
        // because the boundary validator enforced the modelled 1..100 range.
        let page_size = input
            .max_results
            .unwrap_or(crate::state::MAX_PAGE_SIZE)
            .clamp(1, crate::state::MAX_PAGE_SIZE) as usize;
```
Copy 2 — src/ops/list_environment_hosts.rs:186-191:
```
        // Clamped locally, so the page bound holds whatever reaches the handler and not
        // only because the boundary validator enforced the modelled 1..100 range.
        let page_size = input
            .max_results
            .unwrap_or(crate::state::MAX_PAGE_SIZE)
            .clamp(1, crate::state::MAX_PAGE_SIZE) as usize;
```
Copy 3 — src/ops/list_environment_connectors.rs:196-199; Copy 4 — src/ops/list_environment_vlans.rs:192-195; Copy 5 — src/ops/list_vm_entitlements.rs:204-207 — each the same `input.max_results.unwrap_or(crate::state::MAX_PAGE_SIZE).clamp(1, crate::state::MAX_PAGE_SIZE) as usize;`.

The single rule they all restate — src/state.rs:2873-2875:
```
/// The largest page a `@paginated` operation of this service returns when the caller
/// names no `maxResults`: the top of the modelled `MaxResults` range (1..100).
pub const MAX_PAGE_SIZE: i32 = 100;
```

Existing tests that pin the behaviour the fix must leave unchanged: tests/all/environment_lifecycle.rs:1093 and :1171 (page loops over `maxResults` for ListEnvironments and the per-environment lists), tests/all/hosts.rs:143-164 (`maxResults` of 1 bounds the page and `nextToken` resumes), tests/all/connectors.rs:374-377, tests/all/entitlements.rs:499-505, and tests/all/validation_message_rendering.rs:536-537 (`maxResults` at its floor and ceiling are served).

## Steps to Reproduce

1. Open src/ops/list_environments.rs:179, src/ops/list_environment_hosts.rs:188, src/ops/list_environment_connectors.rs:196, src/ops/list_environment_vlans.rs:192 and src/ops/list_vm_entitlements.rs:204 side by side: the page-size expression is the same in all five.
2. Change the rule in one of them (for example drop the `.clamp(1, ..)`) and note that nothing links the other four, so the five paginated operations can bound their pages differently.
3. Confirm the fix is behaviour-preserving by re-running the pagination tests named above after stating the rule once in crate::state.
