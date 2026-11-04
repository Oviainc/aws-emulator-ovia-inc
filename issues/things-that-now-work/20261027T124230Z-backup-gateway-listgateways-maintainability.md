Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ac318820ce0cc5f19770024ed132bf63c4007170

# The MaxResults page-size clamp is reimplemented verbatim in all three listing handlers

_Filed: 2026-10-27T12:42:30Z_
_Operation: ListGateways | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ac318820ce0cc5f19770024ed132bf63c4007170`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "no `MaxResults` means `DEFAULT_PAGE_SIZE`, otherwise clamp the caller's value to at least 1 and at most `MAX_PAGE_SIZE`" is a single computation, but it is written out three times — once in each of `ListGateways`, `ListHypervisors` and `ListVirtualMachines` — character for character, comment included. Nothing in the three copies is resource-specific (unlike the `NextToken` decode beside them, which legitimately differs by resource type and message), so the three can drift: changing the default or the ceiling, or the `max(1)` guard, in one listing and not the others would silently give one resource a different page contract from its siblings while the crate's own paging core (`page_of`/`page_of_where`, which exists precisely so "a page boundary and the token that encodes it mean the same thing whichever resource is being listed") keeps claiming they agree. The fix is a single `page_limit(input.max_results)` helper in `crate::state`, beside the `DEFAULT_PAGE_SIZE` / `MAX_PAGE_SIZE` constants it reads; every response and every existing test stays byte-identical.

## Evidence

Copy 1 — services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/ops/list_gateways.rs:175-177:

        let limit = input.max_results.map_or(crate::state::DEFAULT_PAGE_SIZE, |max| {
            (max.max(1) as usize).min(crate::state::MAX_PAGE_SIZE)
        });

Copy 2 — src/ops/list_hypervisors.rs:178-180:

        let limit = input.max_results.map_or(crate::state::DEFAULT_PAGE_SIZE, |max| {
            (max.max(1) as usize).min(crate::state::MAX_PAGE_SIZE)
        });

Copy 3 — src/ops/list_virtual_machines.rs:185-187:

        let limit = input.max_results.map_or(crate::state::DEFAULT_PAGE_SIZE, |max| {
            (max.max(1) as usize).min(crate::state::MAX_PAGE_SIZE)
        });

The crate already centralises the other half of paging in one place — src/state.rs:1878-1888:

    /// One page of an ARN-keyed resource map in key order — the paging every list
    /// operation of this service does, written once.
...
    fn page_of<T: Clone>(records: &BTreeMap<String, T>, after: Option<&str>, limit: usize) -> Page<T> {

and its doc states the invariant the three copies have to keep by hand — src/state.rs:1297-1299: "`limit` is the caller's `MaxResults` already clamped to [`MAX_PAGE_SIZE`] by the handler".

Existing tests that pin the behaviour and must stay unchanged: tests/all/gateway_lifecycle.rs:1023 `async fn the_store_and_the_page_are_bounded()` ("a `ListGateways` that names no `MaxResults` is truncated at [`DEFAULT_PAGE_SIZE`] with a `NextToken`", tests/all/gateway_lifecycle.rs:1020-1022) and the page walkers at tests/all/gateway_lifecycle.rs:354, tests/all/hypervisor_listing.rs:470-499 and tests/all/virtual_machine_listing.rs:605-633.

## Steps to Reproduce

1. Open src/ops/list_gateways.rs:175-177, src/ops/list_hypervisors.rs:178-180 and src/ops/list_virtual_machines.rs:185-187 side by side.
2. Observe the identical `map_or(DEFAULT_PAGE_SIZE, |max| (max.max(1) as usize).min(MAX_PAGE_SIZE))` expression (and identical preceding comment) in all three.
3. Change the clamp in one file only (e.g. drop the `max(1)` guard) and note that nothing in the crate detects that the three listings now page by different rules.
