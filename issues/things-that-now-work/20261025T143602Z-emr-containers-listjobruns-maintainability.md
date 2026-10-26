Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f3bbb57dafae938115053b9b00b150b1f9a288bb

# The page-bounds rule of the five paginated lists is implemented five times

_Filed: 2026-10-25T14:36:02Z_
_Operation: ListJobRuns | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f3bbb57dafae938115053b9b00b150b1f9a288bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All five `@paginated` list handlers carry a character-for-character copy of the same paging LOGIC: the `maxResults < 1` refusal, the newest-first `(created_at_millis, sequence)` sort, the cursor-position scan, the `end` computation and the "a nextToken only when rows remain" rule. The cursor FORMAT was already centralised in `state.rs` for exactly this reason ("One encoder for every paginated list here, so the three cannot drift apart"), but the rule that consumes it was left duplicated, so the five copies can drift: a change to the page-bounds rule (or to the off-by-one in the `end`/`next_token` pair) has to be made five times and a missed copy is silent. Lifting the slice-and-token step next to `encode_list_cursor`/`decode_list_cursor` in `state.rs` leaves every response byte-identical and every existing test unchanged.

## Evidence

Copy 1, src/ops/list_job_runs.rs:231-242 and :279-289:
```
        let page_size = match input.max_results {
            Some(limit) if limit < 1 => { return Err(… "maxResults must be at least 1, but {limit} was requested" …) }
            Some(limit) => Some(usize::try_from(limit).unwrap_or(usize::MAX)),
            None => None,
        };
…
        let end = page_size.map_or(matching.len(), |size| {
            start.saturating_add(size).min(matching.len())
        });
…
        let next_token = matching.get(end).map(|record| encode_cursor(&input.virtual_cluster_id, record));
```
Copy 2, src/ops/list_job_templates.rs:205-218 and :253-261 — the same `page_size` match and
```
        let end = page_size.map_or(matching.len(), |size| {
            start.saturating_add(size).min(matching.len())
        });
…
        let next_token = matching.get(end).map(|record| encode_cursor(record));
```
Copies 3-5: src/ops/list_security_configurations.rs:219 + :262-275, src/ops/list_managed_endpoints.rs:249-262 + :299-309, src/ops/list_virtual_clusters.rs:257 + :308-318 (identical `position(|record| (record.created_at_millis, record.sequence) <= cursor)` scan in each).

The crate's own reason for centralising the other half, src/state.rs:275-279: "One encoder for every paginated list here, so the three cannot drift apart."

Existing tests that must keep passing unchanged: tests/all/virtual_cluster_listing.rs:229-294, :685-686; tests/all/virtual_cluster_job_run_loop.rs:943-1031, :997-999; tests/all/security_configuration_list.rs:221-257, :471-472; tests/all/managed_endpoint_listing.rs:422-459, :825-826; tests/all/job_template_idempotency.rs:772-840.

## Steps to Reproduce

1. Open src/ops/list_job_runs.rs:231-289 and src/ops/list_job_templates.rs:205-261 side by side: the maxResults refusal, the sort key, the cursor scan, the `end` computation and the next_token rule are the same code.
2. Repeat for src/ops/list_security_configurations.rs, src/ops/list_managed_endpoints.rs and src/ops/list_virtual_clusters.rs — five copies of one rule.
3. Change the rule in one copy only (e.g. the `end` bound) and observe that nothing in the crate forces the other four to follow.
