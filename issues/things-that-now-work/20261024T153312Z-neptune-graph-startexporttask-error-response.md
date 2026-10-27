Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# StartExportTask never authorizes roleArn, so a cross-account or unauthorized pass-role starts a task instead of being denied

_Filed: 2026-10-24T15:33:12Z_
_Operation: StartExportTask | Class: error-response_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartExportTask takes a required `roleArn` the service must assume to read the graph and write to `destination`. AWS makes the pass-role/authorization decision BEFORE any task record is created: a role ARN the caller may not pass, or one belonging to another account, is refused with the service's denial shape AccessDeniedException (HTTP 403) and no export task comes into existence (a subsequent GetExportTask/ListExportTasks finds nothing).

The emulator performs no authorization decision at all on this member. The only check `roleArn` ever meets is the modelled `@pattern` (^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$) applied by the generic boundary validator, so:
  * a well-formed role ARN in a FOREIGN account (e.g. arn:aws:iam::111111111111:role/x, where the caller's account is the emulator's own) is accepted, answered 201, and a task record is persisted and readable through GetExportTask; AWS answers 403 and persists nothing;
  * a malformed value (e.g. 'not-an-arn') is refused as ValidationException/400 by the boundary pattern, where AWS answers AccessDeniedException/403 because it authorizes the member ahead of shape checks.

Expected: the authorization decision runs first in the handler and returns the 403 AccessDenied shape, leaving the export-task store untouched. Actual: 201 + a stored task, or a 400 for the malformed case.

## Evidence

Spec: smithy_query(operation="StartExportTask") shows `roleArn` as a required input constrained only by the IAM-role pattern; the operation models ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429 and ValidationException 400 — i.e. AccessDeniedException is NOT in its modelled list, so AWS answers this path with the service's common denial shape (exactly as recorded for the sibling StartImportTask, where the captured messages are 'User: <callerArn> is not authorized to perform: null' and 'Cross-account pass role is not allowed.').
Handler: services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/start_export_task.rs — the handler validates `destination` as an S3 URI (crate::state::is_s3_uri), bounds `tags`, resolves the graph, gates on graph status AVAILABLE and guards the task ceiling, then stores `role_arn: input.role_arn` verbatim (lines ~279-307). There is no authorization branch and no AccessDenied construction anywhere in the file.
Test coverage: tests/all/export_task_lifecycle.rs exercises accept/reject on destination form, graph status and task lookup; no test sends a foreign-account or unauthorized `roleArn`, and none asserts a 403 on this operation.
Backlog: the open issue '20261023T175427Z-neptune-graph-7-refusal-path-fidelity-causes-wrong' records this authorization-ordering requirement for StartImportTask ONLY (its cause 5, trace neptune-graph.import-task-invalid-role-arn-rejected, and its fix text names src/ops/start_import_task.rs). The identical gap on StartExportTask — a different operation, with its own handler and its own store write — is not covered there.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. CreateGraph(graphName=export-probe, provisionedMemory=16) and poll GetGraph until status is AVAILABLE.
3. StartExportTask(graphIdentifier=<id>, destination='s3://my-bucket/export/', format='CSV', kmsKeyIdentifier='arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012', roleArn='arn:aws:iam::111111111111:role/foreign-export-role').
4. Observe HTTP 201 and a task id; GetExportTask(taskIdentifier=<taskId>) then reports the stored task. Expected: AccessDeniedException (HTTP 403) for the cross-account pass-role, and no export task created (GetExportTask on any newly minted id finds nothing).
5. Repeat with roleArn='not-an-arn': observe ValidationException (400) from the boundary pattern; expected AccessDeniedException (403), i.e. authorization decided ahead of the shape check.
