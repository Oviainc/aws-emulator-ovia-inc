Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 0c701d5f8ae664c771f5c5cdfd2b2b982b891619

# Scope the ListWorkflowRuns page cursor to the workflow it was issued for

_Filed: 2026-10-25T04:08:10Z_
_Operation: ListWorkflowRuns | Class: pagination_

> _Note: this issue was identified against the code base at commit `0c701d5f8ae664c771f5c5cdfd2b2b982b891619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListWorkflowRuns issues a pagination cursor that carries only a GLOBAL run position — `(created_at nanos, sequence)` — and nothing identifying the workflow whose run history the cursor was minted on. When a cursor minted while paging workflow A's runs is presented on a ListWorkflowRuns call for workflow B, the handler accepts it and filters B's runs with `run.listing_key() > cursor`, so every run of B created before that position is silently dropped: the caller gets a 200 with a truncated (often empty) `WorkflowRuns` list and no indication anything was skipped.

The sibling listing in this same crate, ListWorkflowVersions, implements exactly this check: its cursor key carries the `WorkflowArn` of the walk it was issued for and a cursor whose scope is a different workflow is refused with the modelled `ValidationException`, with the rationale spelled out in the code. The run listing needs the same scoping (its key builder `crate::state::format_run_listing_key` carries no workflow at all), and its test file deliberately omits the "cursor of another workflow" case the version-listing test exercises.

## Evidence

The sibling applies the check — `src/ops/list_workflow_versions.rs:240-246`:
```
                // A cursor minted while walking ANOTHER workflow's versions was not issued
                // for this walk: its position means nothing in this history, so honouring
                // it would silently restart or truncate the listing.
                if scope != input.workflow_arn {
                    return Err(refused().into());
                }
```
and its key carries the workflow — `src/state.rs:801-803`:
```
pub fn format_version_listing_key(workflow_arn: &str, sequence: u64) -> String {
    format!("{sequence:020}{VERSION_LISTING_KEY_SEPARATOR}{workflow_arn}")
}
```
This handler skips it — `src/ops/list_workflow_runs.rs:226-238` decodes the token with no scope check:
```
        let after: Option<crate::state::RunListingKey> = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                crate::state::decode_page_token(PAGE_KIND, token)
                    .as_deref()
                    .and_then(crate::state::parse_run_listing_key)
```
and then applies the foreign position directly — `src/ops/list_workflow_runs.rs:284`:
```
            .filter(|run| after.is_none_or(|cursor| run.listing_key() > cursor))
```
The run cursor key holds no workflow — `src/state.rs:775-778`:
```
pub fn format_run_listing_key(key: RunListingKey) -> String {
    let (nanos, sequence) = key;
    format!("{nanos:039}.{sequence:020}")
}
```
Test coverage: `tests/all/workflow_version_listing.rs:605-608` lists the case for versions (`("a cursor from another workflow's version listing", cursor_of_another_workflow)`), while the equivalent run-listing test `tests/all/workflow_run_listing.rs:654-660` covers only malformed tokens and `("a ListWorkflows cursor", foreign_kind)` — no cross-workflow case.

## Steps to Reproduce

1. CreateWorkflow "wfOld" and start 3 runs on it; then CreateWorkflow "wfNew" and start 3 runs on it (so wfNew's runs are newer than wfOld's).
2. Call ListWorkflowRuns for wfNew with MaxResults=2 and keep the returned NextToken (its cursor is the position of wfNew's 2nd run).
3. Call ListWorkflowRuns for wfOld with MaxResults=10 and that NextToken.
4. Expected (the rule ListWorkflowVersions applies): 400 ValidationException — the cursor was not issued for this walk.
5. Actual: 200 with an EMPTY `WorkflowRuns` list and no NextToken; wfOld's three runs are silently skipped because their `listing_key()` is lower than the foreign cursor.
