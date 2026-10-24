Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 3a66b075702e523a87f5e33a7353c602a74f8e6d

# ListWorkflowVersions answers an unmodeled ResourceNotFoundException for a workflow ARN that does not exist

_Filed: 2026-10-24T19:15:32Z_
_Operation: ListWorkflowVersions | Class: error-response_

> _Note: this issue was identified against the code base at commit `3a66b075702e523a87f5e33a7353c602a74f8e6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a pattern-valid `WorkflowArn` the store does not hold, the handler refuses the call with `ResourceNotFoundException` (served at HTTP 400 via `crate::state::lookup_miss`). `ResourceNotFoundException` is NOT in this operation's modeled error set.

Expected (per the model's error set for this operation): HTTP 200 with an empty `WorkflowVersions` list, or one of the errors the operation actually declares (ValidationException) — never an error shape outside the declared set, which an AWS SDK client cannot map to a modeled exception type and surfaces as a generic/unknown error.

Actual: HTTP 400 with `__type: com.amazonaws.mwaaserverless#ResourceNotFoundException` plus `ResourceId`/`ResourceType` members.

Note this is the sibling of the already-filed ListWorkflowRuns issue (`20261024T153223Z-mwaa-serverless-listworkflowruns-error-response`), which explicitly observes that the same omission applies to ListWorkflowVersions; it is a separate handler and a separate (operation, class) pair, so it is filed separately here.

## Evidence

Smithy model: `smithy_query(query="operation", operation="ListWorkflowVersions")` lists errors = AccessDeniedException (403), InternalServerException (500), OperationTimeoutException (504), ThrottlingException (429), ValidationException (400) — no ResourceNotFoundException. By contrast every single-resource operation taking the same ARN (GetWorkflow, DeleteWorkflow, StartWorkflowRun, GetWorkflowRun, ListTagsForResource) DOES declare it, so the model describes a deliberate difference for the child-resource list operations.

Handler: `services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/list_workflow_versions.rs` lines 239-248 return `crate::state::lookup_miss(ListWorkflowVersionsErrorModeled::ResourceNotFoundException(crate::state::unresolved(UnresolvedEntity::Workflow, &input.workflow_arn)))`; the error enum is widened with `extra_errors = ["ResourceNotFoundException"]` because the operation's own error list omits it.

Test coverage: `tests/all/workflow_version_listing.rs::a_workflow_the_store_does_not_hold_is_a_lookup_miss_not_an_empty_page` asserts the widened behavior (it checks `ResourceType` and a null `WorkflowVersions`), i.e. the current test pins the divergence rather than the model's contract and would need updating with the fix.

## Steps to Reproduce

1. Boot the emulator and call CreateWorkflow once to learn the ARN shape used for the account/region
2. Call ListWorkflowVersions with a pattern-valid WorkflowArn for a workflow that was never created, e.g. `arn:aws:airflow-serverless:us-east-1:123456789012:workflow/ghost-abcdefghij`
3. Observe HTTP 400 carrying `ResourceNotFoundException`, an error shape absent from this operation's modeled error list, instead of 200 with an empty `WorkflowVersions` list (or a modeled ValidationException)
