Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 3a66b075702e523a87f5e33a7353c602a74f8e6d

# A ListWorkflowVersions cursor is hard-refused once the version it names is deleted, aborting a walk mid-pagination

_Filed: 2026-10-24T19:15:32Z_
_Operation: ListWorkflowVersions | Class: pagination_

> _Note: this issue was identified against the code base at commit `3a66b075702e523a87f5e33a7353c602a74f8e6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The pagination cursor this listing issues is the `WorkflowVersion` of the last item on the page, and the next page is resolved by finding that version's POSITION in the stored `versions` vector. If that exact version is no longer held, the handler refuses the request with `ValidationException` ("invalid page token", HTTP 400).

The handler's own comment justifies this by claiming "A version is immutable and is never removed on its own — it goes only with the workflow that holds it", but that is not true of this emulator: `DeleteWorkflow` with a `WorkflowVersion` removes exactly one version and leaves the workflow in place (the model documents `DeleteWorkflow$WorkflowVersion` as "The specific version of the workflow to delete. If not specified, all versions of the workflow are deleted").

Expected: a token the service itself issued continues the walk over the versions that remain (the deleted version simply no longer appears). Deleting an unrelated/own version must not turn a client's in-flight pagination into a 400.

Actual: the second page request returns HTTP 400 ValidationException and no `WorkflowVersions`, so the client cannot finish enumerating the workflow's versions and has to restart from page one.

## Evidence

Spec: `smithy_query(query="operation", operation="ListWorkflowVersions")` marks it `@paginated(inputToken=NextToken, outputToken=NextToken, items=WorkflowVersions)`; `smithy_query(query="operation", operation="DeleteWorkflow")` / OPERATIONS.md document `WorkflowVersion` as deleting a single named version.

Handler: `src/ops/list_workflow_versions.rs` lines 265-277 — `workflow.versions.iter().position(|record| record.version == last)` with `.ok_or_else(|| ...invalid_page_token("ListWorkflowVersions"))`, i.e. a cursor whose version is gone becomes a 400. The removal path that makes this reachable is `src/ops/delete_workflow.rs` lines 266-290: `workflow.versions.retain(|rec| rec.version != version)` while the workflow record itself stays READY.

Test coverage: `tests/all/workflow_version_listing.rs::a_pagination_cursor_this_listing_did_not_issue_is_refused` only exercises FOREIGN cursors (non-hex, a ListWorkflows cursor, another workflow's cursor); no test covers a cursor this listing did issue whose version was subsequently deleted, so the gap is unasserted.

## Steps to Reproduce

1. CreateWorkflow `verProbe`, then call UpdateWorkflow three times with different DefinitionS3Location values so the workflow holds four versions
2. Call ListWorkflowVersions with MaxResults=2 and record `NextToken` (the cursor names the 2nd version in append order)
3. Call DeleteWorkflow for that workflow with `WorkflowVersion` set to the 2nd version (the workflow stays, three versions remain)
4. Call ListWorkflowVersions again with the recorded NextToken
5. Observe HTTP 400 ValidationException (invalid pagination token) instead of a page continuing with the remaining versions
