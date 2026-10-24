Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# ListWorkflowRuns answers an unmodeled ResourceNotFoundException for a workflow ARN that does not exist

_Filed: 2026-10-24T15:32:23Z_
_Operation: ListWorkflowRuns | Class: error-response_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a syntactically valid `WorkflowArn` that the store does not hold, the handler refuses the call with `ResourceNotFoundException` served at HTTP 400. `ResourceNotFoundException` is NOT in this operation's modeled error set: `ListWorkflowRuns` declares only AccessDeniedException (403), InternalServerException (500), OperationTimeoutException (504), ThrottlingException (429) and ValidationException (400). The handler widens the error set on purpose (`extra_errors = ["ResourceNotFoundException"]`).

The omission is consistent across all three of this service's child-resource list operations (ListWorkflowRuns, ListWorkflowVersions, ListTaskInstances) while every single-resource operation that takes the same ARN (GetWorkflow, DeleteWorkflow, StartWorkflowRun, GetWorkflowRun, ListTagsForResource) DOES declare it — so the model is describing a deliberate difference, not an oversight.

Expected (per the model's error set): the list answers 200 with an empty `WorkflowRuns` list, or one of the errors it actually declares (ValidationException) — never an error shape outside its declared set, which an SDK client cannot map to a modeled exception type and surfaces as a generic/unknown error.

Actual: HTTP 400 with `__type: com.amazonaws.mwaaserverless#ResourceNotFoundException` and `ResourceId`/`ResourceType` members.

## Evidence

Smithy model (`smithy_query(query="operation", operation="ListWorkflowRuns")`): errors = AccessDeniedException, InternalServerException, OperationTimeoutException, ThrottlingException, ValidationException — no ResourceNotFoundException. Handler: `services/.../src/ops/list_workflow_runs.rs` lines 118-139 declare `#[smithy_macros::operation_error("com.amazonaws.mwaaserverless#ListWorkflowRuns", extra_errors = ["ResourceNotFoundException"])]` with an in-file comment acknowledging "This operation's own `errors` list omits it, so it is widened in here"; lines 236-243 return `crate::state::lookup_miss(ListWorkflowRunsErrorModeled::ResourceNotFoundException(...))`, i.e. the shape at `crate::state::LOOKUP_MISS_STATUS` = 400. Test coverage: `tests/all/workflow_run_listing.rs` asserts the widened behavior (`LOOKUP_MISS_TYPE = "ResourceNotFoundException"`), so the current test pins the divergence rather than the model's contract and would need updating with the fix.

## Steps to Reproduce

1. Boot the emulator and call CreateWorkflow to learn the ARN shape for the account/region
2. Call ListWorkflowRuns with a pattern-valid WorkflowArn for a workflow that was never created (e.g. `arn:aws:airflow-serverless:us-east-1:123456789012:workflow/ghost-abcdefghij`)
3. Observe HTTP 400 carrying ResourceNotFoundException, an error the operation's modeled error list does not contain, instead of a 200 with an empty WorkflowRuns list (or a modeled ValidationException)
