Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 45e969ed3c6a96399c623276452bb212f0ee3327

# A ListWorkflowRuns cursor carries only the workflow, not the WorkflowVersion filter that selects the walk, so resuming under a different filter silently skips stored runs

_Filed: 2026-10-29T12:17:35Z_
_Operation: ListWorkflowRuns | Class: pagination_

> _Note: this issue was identified against the code base at commit `45e969ed3c6a96399c623276452bb212f0ee3327`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListWorkflowRuns` pages by POSITION in the workflow's creation order and binds its cursor to the workflow ARN only. The `WorkflowVersion` filter — which selects WHICH runs the walk reports — is not part of the cursor's scope, and the handler applies a presented cursor as a position whatever filter the resuming request carries.

The result is a silently truncated page: a token minted while paging the runs of version B, presented on a call filtered to version A, is honoured, and every run of version A created BEFORE that position is dropped from the walk. The caller gets 200 with a short `WorkflowRuns` list and no indication that stored runs were skipped — exactly the outcome the crate's own cursor-scoping rule exists to prevent and which it refuses with `ValidationException` in the cross-workflow case.

Every other cursor in this crate is bound to whatever selects its set: `ListWorkflows` to the region partition it walks, `ListTaskInstances` to the workflow AND the run, `ListWorkflowVersions` to the workflow. `ListWorkflowRuns` is the one walk whose set is also narrowed by a request member, and that member is the one thing its cursor does not carry.

This is a distinct root cause from the two fixed `ListWorkflowRuns` pagination issues: `...-listworkflowruns-pagination` was the hashed-RunId ordering, and `...-listworkflowruns-pagination--r2` was scoping the cursor to the workflow ARN (the scope that is now present). Neither mentions the version filter.

## Evidence

The crate's own rule for what belongs in a cursor key — `src/state.rs:1677-1686`:
```
/// The cursor key for a position in ONE listing walk: the position of the last
/// item on the page, and the SCOPE that walk is over — the parent workflow for a
/// child listing, the endpoint's region for a listing partitioned by region.
///
/// A walk covers exactly one scope, so the scope belongs in the key: a cursor
/// presented on a walk over ANOTHER scope is then recognisable as one not issued
/// for that walk, instead of being applied as a position in a sequence it says
/// nothing about.
pub fn format_scoped_listing_key(scope: &str, position: &str) -> String {
```
and the harm it names, restated by this very handler — `src/ops/list_workflow_runs.rs:220-225`:
```
        // issued on: a cursor minted while walking another workflow's runs is not a
        // position in this history, so honouring it would silently truncate or restart
        // the listing — it is refused, exactly like a cursor issued for another of this
        // service's lists or one this service never issued.
```
The scope the cursor is actually bound to is the ARN alone — `src/ops/list_workflow_runs.rs:231` and `:296`:
```
                crate::state::parse_scoped_page_token(PAGE_KIND, token, &input.workflow_arn)
...
                &crate::state::format_run_listing_key(&input.workflow_arn, last.listing_key()),
```
while the set being walked is narrowed by a second, unscoped member — `src/ops/list_workflow_runs.rs:265-269` and `:278-284`:
```
        // `WorkflowVersion` is a FILTER, not a resolution: "If not specified, runs for
        // all versions are returned" ...
        let wanted_version = input.workflow_version.as_deref();
...
        let mut remaining: Vec<&crate::state::WorkflowRun> = workflow
            .runs
            .values()
            .filter(|run| wanted_version.is_none_or(|version| run.workflow_version == version))
            .filter(|run| after.is_none_or(|cursor| run.listing_key() > cursor))
            .collect();
```
So a cursor from the version-B walk passes the scope check (same ARN) and is then applied as a position against the version-A set.

Sibling walks bind every selector into the scope: `src/ops/list_workflows.rs:200` (`parse_workflow_listing_page_token(PAGE_KIND, token, &region)`), `src/ops/list_task_instances.rs:217-222` (`parse_task_instance_page_token(PAGE_KIND, token, &input.workflow_arn, &input.run_id)`).

Test coverage: `tests/all/child_listing_cursor_scope.rs` and `tests/all/workflow_run_listing.rs` cover foreign-kind, malformed and cross-workflow cursors; no case presents a cursor minted under one `WorkflowVersion` filter on a walk under a different one, and `tests/all/workflow_run_listing_order.rs` pins only the creation order.

## Steps to Reproduce

1. CreateWorkflow "wfFilter", then UpdateWorkflow once, so the workflow holds two versions V1 (older) and V2 (latest).
2. StartWorkflowRun with WorkflowVersion=V1 (run R1), then WorkflowVersion=V2 (run R2), then WorkflowVersion=V1 (run R3) — creation order R1, R2, R3.
3. Call ListWorkflowRuns with WorkflowArn=wfFilter, WorkflowVersion=V2, MaxResults=1. It answers [R2] plus a NextToken whose position is R2's.
4. Call ListWorkflowRuns with WorkflowArn=wfFilter, WorkflowVersion=V1, MaxResults=10 and that NextToken.
5. Expected (the rule the crate applies to a cursor that is not a position in the walk being asked for): 400 ValidationException.
6. Actual: 200 with WorkflowRuns=[R3] only — R1, a stored V1 run that sorts before the foreign cursor, is silently dropped and no NextToken signals it.
