Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ce187dd52ad3609a091254aaff4a955715a54b39

# Collapse the five copies of the createdBefore/createdAfter bound rule into one helper beside page_of

_Filed: 2026-10-27T03:42:21Z_
_Operation: ListJobRuns | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ce187dd52ad3609a091254aaff4a955715a54b39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `createdBefore` / `createdAfter` strict-bound rule for a listing's `createdAt` is written out five times, once per paginated list handler, as byte-identical code differing only in the record type it reads `created_at_millis` from. Each copy's own doc comment says it is deliberately the SAME rule as the others ('the same way `ListJobTemplates` and `ListJobRuns` bound theirs'), so the crate already treats it as one shared rule — but there is no single place that holds it, and the five copies can drift: flipping one `>=` to `>` in one handler would silently give that one listing inclusive bounds while the other four stayed exclusive, and nothing in the crate would object.

This is exactly the drift the crate already guards against for the other half of the listing contract: the page bounds, order and cursor live in ONE function, `crate::state::page_of`, whose doc states the reason. The created-at bound rule should sit beside it (e.g. a `state::within_created_bounds(created_at_millis, created_before, created_after) -> bool` the five `matches_filters` functions call), leaving every response byte-identical and every existing test untouched.

The neighbouring `states` filter is duplicated the same way in three of the five handlers (list_job_runs.rs:85-89, list_virtual_clusters.rs:96-100, list_managed_endpoints.rs:86-90) and can move with it; the per-listing filters that are genuinely unique (`name`, `types`, `containerProviderId`, `eksAccessEntryIntegrated`) stay where they are.

## Evidence

Five identical copies of the same rule (one per paginated listing):

- `src/ops/list_job_runs.rs:90-105`:
```
    // `createdBefore` / `createdAfter` bound `createdAt` strictly, so the two together
    // name an open interval and neither ever returns the run created exactly at a bound.
    if input
        .created_before
        .as_ref()
        .is_some_and(|bound| record.created_at_millis >= crate::state::millis_of(bound))
    {
        return false;
    }
    if input
        .created_after
        .as_ref()
        .is_some_and(|bound| record.created_at_millis <= crate::state::millis_of(bound))
    {
        return false;
    }
```
- `src/ops/list_virtual_clusters.rs:128-144` — the same two `if` blocks, comment reworded to '…the cluster created exactly at a bound.'
- `src/ops/list_managed_endpoints.rs:98-114` — the same, '…the endpoint created exactly at a bound.'
- `src/ops/list_job_templates.rs:84-97`, whose doc admits the copy: "`createdBefore` / `createdAfter` bound `createdAt` strictly, the same way `ListJobRuns` bounds a job run's" (list_job_templates.rs:77-79).
- `src/ops/list_security_configurations.rs:88-101`, likewise: "`createdBefore` / `createdAfter` bound `createdAt` strictly, the same way `ListJobTemplates` and `ListJobRuns` bound theirs" (list_security_configurations.rs:81-83).

The crate already states the one-place principle for the other half of the same contract — `src/state.rs:443-444`: "One rule for every paginated list here, beside the one cursor encoder, so the two halves of the pagination contract cannot drift apart." — and `page_of` is called by all five handlers (list_job_runs.rs:213, list_virtual_clusters.rs:240, list_managed_endpoints.rs:228, list_job_templates.rs:188, list_security_configurations.rs:202).

Existing tests that pin the behaviour the refactor must not change (all drive real restJson1 requests through the router):
- `tests/all/virtual_cluster_listing.rs:441-556` — "RULE 4: `createdAfter` and `createdBefore` bound the stored `createdAt` strictly", including the at-the-midpoint rows.
- `tests/all/security_configuration_list.rs:277-400` — the same rule for `ListSecurityConfigurations`.
- `tests/all/managed_endpoint_listing.rs:612-706` — the same rule for `ListManagedEndpoints`.
- `tests/all/virtual_cluster_job_run_loop.rs:1112-1140` — the `createdAfter`/`createdBefore` rows for `ListJobRuns`.
- `tests/all/paginated_page_bounds.rs:242-311` — the shared page rule over all five listings.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/list_job_runs.rs` and read `matches_filters` (lines 78-107).
2. Open `list_virtual_clusters.rs` (lines 90-146), `list_managed_endpoints.rs` (lines 80-116), `list_job_templates.rs` (lines 74-99) and `list_security_configurations.rs` (lines 78-103): the two `created_before` / `created_after` `if` blocks are byte-identical in all five, and two of the doc comments say so explicitly.
3. Compare with `crate::state::page_of` (src/state.rs:430-483), where the rest of the listing contract is written exactly once, with the stated reason that the halves 'cannot drift apart'.
4. Change `>=` to `>` in just one of the five copies and run `cargo test -p service-com-amazonaws-emrcontainers-awschicagowebservice`: only that listing's own bound test fails, showing the copies are independent and free to diverge.
