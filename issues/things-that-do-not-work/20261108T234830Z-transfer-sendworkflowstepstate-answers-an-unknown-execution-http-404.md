Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-workflow-definition-and-execution-introspection
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-workflow-definition-and-execution-introspection
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|SendWorkflowStepState|4xx->4xx|status
Issue-Commit: fbc2bd771b8337584b553819b599ea14c7bec6f2

# Transfer SendWorkflowStepState answers an unknown execution HTTP 404 instead of the recorded 400

> _Note: this issue was identified against the code base at commit `fbc2bd771b8337584b553819b599ea14c7bec6f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendWorkflowStepState

## Description
ONE net-new root cause in the transfer emulator. It is the only divergence in this replay of simparity-wire-aws-v1 that the report marked neither blocked-by-a-stub nor OWNED by an open issue.

=== Cause 1 of 1: SendWorkflowStepState's ResourceNotFoundException is served at the model's @httpError(404) while the recorded Transfer wire answers this service's ResourceNotFoundException with HTTP 400 ===

AWS: Smithy com.amazonaws.transfer#SendWorkflowStepState input {WorkflowId @required len 19..19 pattern ^w-([a-z0-9]{17})$, ExecutionId @required len 36..36 UUID pattern, Token @required len 1..64 pattern ^\w+$, Status @required CustomStepStatus}; output EMPTY; modeled errors AccessDeniedException 403, InternalServiceError 503, InvalidRequestException 400, ResourceNotFoundException 404, ServiceUnavailableException 503, ThrottlingException 429. The model's @httpError(404) on ResourceNotFoundException does NOT describe the Transfer wire: the recording for this step shows the service answering a callback that resolves to no running execution with HTTP **400** carrying the awsJson1_1 ResourceNotFoundException body ($.__type 'ResourceNotFoundException', $.Message the fixed text 'Unknown execution', $.Resource the ExecutionId taken from the REQUEST, $.ResourceType 'Execution'). That is the same wire convention the rest of this corpus records for Transfer's not-found family — e.g. transfer-as2-agreement-lifecycle#task#11 records DescribeAgreement answering 400 with __type ResourceNotFoundException / Message 'Unknown agreement' / ResourceType 'Agreement' — so the status for a Transfer ResourceNotFoundException is 400, including on this operation. Recorded behavior only for the status (the model states 404 and is wrong here); the body is the model's own shape and the emulator already produces it correctly.

Emulator: the step answers HTTP 404. The response BODY matches the recording byte-for-byte (the replay reports no body-key divergence at all — only 'status: expected 400, got 404'), so this is purely the status line. The emulator does this deliberately and by an explicit carve-out: services/service-com-amazonaws-transfer-transferservice/src/state.rs defines RESOURCE_NOT_FOUND_HTTP_STATUS = 400 and the helper state::not_found() that re-stamps a modeled not-found onto it, but its doc comment scopes the override away from this operation ('SendWorkflowStepState's callback ... returns the error WITHOUT this override'), and src/ops/send_workflow_step_state.rs accordingly returns SendWorkflowStepStateErrorModeled::ResourceNotFoundException(st::unknown_execution(..)) directly, so the modeled 404 is used. The recording contradicts that assumption.

Why: a client branches on the HTTP status before it ever looks at __type. Code written against real Transfer Family treats 4xx-with-400 as this service's client-refusal status and routes on the error code in the body; an emulator that answers 404 makes a generic HTTP layer (an SDK retry/classification policy, an API-gateway or proxy mapping, a test asserting response['ResponseMetadata']['HTTPStatusCode'] == 400) classify the callback refusal differently than production does. For the callback-token flow specifically, 'this token/execution is gone' is the signal a step target uses to decide a callback was already consumed, so the status it arrives under is load-bearing for retry logic.

Fix: the status decision in the SendWorkflowStepState handler — services/service-com-amazonaws-transfer-transferservice/src/ops/send_workflow_step_state.rs — must return its ResourceNotFoundException through the service's shared not-found status path (crate::state::not_found / RESOURCE_NOT_FOUND_HTTP_STATUS) like every other Transfer operation, i.e. HTTP 400 with the body unchanged: __type 'ResourceNotFoundException', Message exactly 'Unknown execution' (fixed text, no variable part), Resource = the ExecutionId from the request, ResourceType 'Execution'. Both not-found returns in that handler (the unresolved (WorkflowId, ExecutionId) execution and the already-consumed token) are the same condition and must take the same status; the scope note on RESOURCE_NOT_FOUND_HTTP_STATUS in src/state.rs should be corrected to include this operation.

Related already-filed issue that does NOT cover this: open behavioral issue 20261108T015247Z-transfer-the-in-order-sendworkflowstepstate-a-custom-workflow.md concerns the LIVE-token leg of the same handler (the first in-order callback never succeeding, so an execution never completes); it explicitly treats the replayed-token refusal as already correct 'at the status the model puts on the shape', which is the very thing this wire recording contradicts. It carries no key for this fixture step and this replay did not mark this divergence as owned by it.

Traces: transfer-workflow-definition-and-execution-introspection#task#7.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 56 fixtures replayed, 309 steps, 273 passed, 36 mismatched, 0 errored, 0 blocked, score 88.3%, emulator_up true (/__admin/health answered 200 and the :35013 data listener accepts), bootstrap_signal false. 24 divergence clusters; 23 of them carry a non-empty owned_by and are skipped. This is the only unowned cluster.

[mismatch] fixture transfer-workflow-definition-and-execution-introspection, step task#7, service transfer, operation SendWorkflowStepState
  divergence_key: service=transfer|SendWorkflowStepState|status=400->404|keys=status
  expected_status: 400
  actual_status:   404
  detail (verbatim, the complete detail for the cluster): "status: expected 400, got 404"
  affected: ["transfer-workflow-definition-and-execution-introspection#task#7"] (affected_count 1)

No response-body key is listed as missing, extra or value_mismatched, so the emulator's awsJson1_1 error body already equals the recorded one — __type 'ResourceNotFoundException', Message 'Unknown execution' (entirely fixed text), Resource = the ExecutionId the request carried, ResourceType 'Execution'. The ONLY divergence is the HTTP status line: 404 served, 400 recorded.

Same-corpus corroboration that 400 is Transfer's recorded status for this error class (these clusters are owned by other open issues and are not re-filed here): transfer-as2-agreement-lifecycle#task#11 records DescribeAgreement answering expected_status 400 with $.__type 'ResourceNotFoundException', $.Message 'Unknown agreement', $.ResourceType 'Agreement'; transfer-resource-tagging-across-types#task#13 records TagResource answering expected_status 400 with ResourceType 'Server' / Message 'Unknown server'.

Code grounding in this checkout:
  services/service-com-amazonaws-transfer-transferservice/src/state.rs lines 337-351: doc comment states 'The model puts @httpError(404) on the shape, but the recorded Transfer wire answers an unknown server or user with HTTP 400 and __type ResourceNotFoundException', then 'pub const RESOURCE_NOT_FOUND_HTTP_STATUS: u16 = 400;' — and lines 346-350 deliberately exclude SendWorkflowStepState from the override.
  src/state.rs lines 935-942: 'pub fn not_found<E>(error: E) -> Scaffolded<E> { Scaffolded::modeled_with_status(error, RESOURCE_NOT_FOUND_HTTP_STATUS) }' — the shared path this handler bypasses.
  src/ops/send_workflow_step_state.rs lines 198-215 and 230-243: both ResourceNotFoundException returns go out as the bare modeled variant (st::unknown_execution(&input.execution_id)), i.e. at the model's 404, with the comment at lines 201-206 asserting the 404 is what was recorded for this operation. The replay shows the recording says 400.
  src/state.rs lines 413-418 / 708-712: NOT_FOUND_EXECUTION { resource_type: 'Execution', message: 'Unknown execution' } and unknown_execution(execution_id) — the body text, which is already correct.

Nothing asserted here is per-run or account-specific: no request id, clock value, host, port or generated id is involved — the finding is a single integer in the status line.

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from this checkout (this run: branch issues/finder-wi-d5f72e6238a84256bd84edf29ccbde08-1794180970089; /__admin/health 200, data listener on :35013).
2. Replay the simparity-wire-aws-v1 wire corpus and look at the one divergence cluster with an empty owned_by: fixture transfer-workflow-definition-and-execution-introspection, step task#7, operation SendWorkflowStepState — 'status: expected 400, got 404'.
3. By hand, against the emulator (region us-west-2, awsJson1_1 target TransferService.SendWorkflowStepState): transfer.create_workflow(Steps=[{'Type':'CUSTOM','CustomStepDetails':{'Name':'pause','Target':<some lambda arn>,'TimeoutSeconds':300,'SourceFileLocation':'${original.file}'}}]) and note the WorkflowId.
4. Call transfer.send_workflow_step_state(WorkflowId=<that id>, ExecutionId='00000000-0000-0000-0000-000000000000', Token='deadbeef', Status='SUCCESS') — a well-formed callback (every member satisfies its modeled length/pattern) naming an execution the workflow is not running.
5. Observe the response: HTTP 404 with body {'__type':'ResourceNotFoundException','Message':'Unknown execution','Resource':'<the ExecutionId sent>','ResourceType':'Execution'}. Real Transfer Family, and the recording for this step, answer the SAME body with HTTP 400.
6. For contrast, call transfer.describe_agreement(ServerId=<a server>, AgreementId='a-00000000000000005') or transfer.tag_resource on an unknown server ARN against the same emulator and see HTTP 400 for ResourceNotFoundException — the status this operation should also use.
7. Assert on response['ResponseMetadata']['HTTPStatusCode']: 400 is required, 404 is served.
