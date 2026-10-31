Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: omics.create-workflow-version-missing-workflow-not-found-rejected
Issue-Shape-Id: com.amazonaws.omics#Omics
Issue-Key: omics.create-workflow-version-missing-workflow-not-found-rejected|run#9|AssertionError|assert list_code == "ResourceNotFoundException", (
Issue-Commit: f4f656b1bd7a513d31ed52598c20b4db3f989ef9

# omics: ListWorkflowVersions on a nonexistent workflow answers 501 NotImplemented instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `f4f656b1bd7a513d31ed52598c20b4db3f989ef9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListWorkflowVersions

## Description
Cause 1 of 1: ListWorkflowVersions never answers for any input — it returns the scaffold's 501 `NotImplemented` — so the not-found refusal a client uses to confirm that a rejected CreateWorkflowVersion left no version behind is unreachable.

AWS: Smithy (services/service-com-amazonaws-omics-omics/smithy-model/com-amazonaws-omics-omics.json) binds ListWorkflowVersions to `GET /workflow/{workflowId}/version` with success code 200, input member `workflowId` (@httpLabel, required, @length 1..18, @pattern ^[0-9]+$), and models `ResourceNotFoundException` (HTTP 404, documented "The target resource was not found in the current Region") among its errors. For a well-formed `workflowId` that names no workflow in the account/Region, AWS refuses with HTTP 404, error code `ResourceNotFoundException`, message `Workflow <workflowId> not found` — the fixed text is `Workflow ` + ` not found` and the variable part is the `workflowId` value taken verbatim from the request (the recorded check asserts exactly this wording; recorded behavior plus the modeled error). For an existing workflow AWS returns 200 with `items` (and `nextToken` when more results exist), paginated by `startingToken`/`maxResults`.

Emulator: every ListWorkflowVersions request falls through to the scaffold stub and is refused with HTTP 501 and error code `NotImplemented` (message "operation ListWorkflowVersions is scaffolded but not implemented in this emulator"). The replayed check observed code `'NotImplemented'` where `ResourceNotFoundException` was required. Note that in the same check CreateWorkflowVersion DID correctly refuse the missing workflow with `ResourceNotFoundException` / `Workflow 9999999999 not found`, so only the version-listing read diverges here.

Why: a client cannot list or page a workflow's versions at all, and cannot distinguish "this workflow does not exist" (404, retry-never, surfaced to the user as a not-found) from "the service is broken" (501, which botocore classifies as neither a modeled client error nor retryable). Any control-plane flow that verifies version state after a rejected or successful CreateWorkflowVersion — and any paginator over versions — fails with an unmodeled error code instead of the modeled one.

Fix: implement the ListWorkflowVersions handler (its handle body and the workflow-version lookup it needs in the service state): resolve `workflowId` against stored workflows, return `Err(ListWorkflowVersionsErrorModeled::ResourceNotFoundException(..))` with the message template above when no such workflow exists, otherwise serialize the workflow's versions into `items` with `nextToken` honoring `startingToken`/`maxResults`; then remove the SCAFFOLDED marker. A separate open issue covers the bare stub as reached by omics.delete-workflow-with-version-conflict-rejected (ListWorkflowVersions stub, filed from the checkout) — that one does not carry this check's replay key or the 404 not-found wording requirement, so this issue records them.

Traces: omics.create-workflow-version-missing-workflow-not-found-rejected (mismatch).

## Evidence
Failing check: omics.create-workflow-version-missing-workflow-not-found-rejected [mismatch]

Assertion from the replay:
  File "check_omics_create_workflow_version_missing_workflow_not_found_rejected.py", line 58, in run
    assert list_code == "ResourceNotFoundException", (...)
  AssertionError: expected ListWorkflowVersions on a nonexistent workflow to reject with ResourceNotFoundException, got 'NotImplemented'

Expected (AWS): HTTP 404, Error.Code = "ResourceNotFoundException", Error.Message = "Workflow <workflowId> not found" (fixed text "Workflow " ... " not found"; <workflowId> is the value sent in the request path label).
Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation ListWorkflowVersions is scaffolded but not implemented in this emulator".

Source of the stub response: services/service-com-amazonaws-omics-omics/src/ops/list_workflow_versions.rs line 183 — `Err(crate::scaffolded::Scaffolded::not_implemented("ListWorkflowVersions"))`, with the SCAFFOLDED marker at lines 4-6.
Model: ListWorkflowVersions — http GET /workflow/{workflowId}/version, code 200; errors include ResourceNotFoundException (404), ValidationException (400); paginated inputToken=startingToken, outputToken=nextToken, pageSize=maxResults, items=items.

## Steps to Reproduce
1. Point an AWS client at the emulator's omics endpoint.
2. Call CreateWorkflowVersion with workflowId="9999999999" (a well-formed id that does not exist), a versionName, a definitionZip and a requestId — the emulator correctly refuses with ResourceNotFoundException / "Workflow 9999999999 not found".
3. Call ListWorkflowVersions(workflowId="9999999999").
4. Observe HTTP 501 with Error.Code "NotImplemented" instead of HTTP 404 with Error.Code "ResourceNotFoundException" and message "Workflow 9999999999 not found".
5. Also call ListWorkflowVersions with the id of a real, ACTIVE workflow created via CreateWorkflow: it likewise answers 501 instead of 200 with an items list.
