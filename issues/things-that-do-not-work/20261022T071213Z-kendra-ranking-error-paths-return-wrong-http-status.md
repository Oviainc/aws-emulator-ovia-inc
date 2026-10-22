Issue-Source: trace-sweep-divergence
Issue-Trace: kendra-ranking-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kendra-ranking-error-paths kendra-ranking-plan-lifecycle-create-update-delete
Issue-Shape-Id: com.amazonaws.kendraranking#AWSKendraRerankingFrontendService
Issue-Key: kendra-ranking|DescribeRescoreExecutionPlan|4xx->4xx|status
Issue-Key: kendra-ranking|UpdateRescoreExecutionPlan|4xx->4xx|$.Message
Issue-Key: kendra-ranking|UpdateRescoreExecutionPlan|4xx->4xx|status
Issue-Key: kendra-ranking|DeleteRescoreExecutionPlan|4xx->4xx|status
Issue-Key: kendra-ranking|Rescore|4xx->4xx|status
Issue-Key: kendra-ranking|Rescore|4xx->4xx|$.Message
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# kendra-ranking error paths return wrong HTTP status and wrong validation precedence

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRescoreExecutionPlan

## Description
Five net-new causes in the kendra-ranking (AWSKendraRerankingFrontendService, awsJson1_0) error paths. Causes 1-4 are one shared fault: the emulator answers client errors with the modeled @httpError status (404) where AWS answers 400. Cause 5 is a validation-precedence fault in Rescore document validation. All five MUST be closed before this issue is submitted.

Cause 1 of 5: DescribeRescoreExecutionPlan on a well-formed but nonexistent rescore execution plan id.
AWS: recorded behavior only. The recorded response is HTTP 400; the error code in the response body matched the emulator's (only `status` diverged in the replay), so the divergence is purely the transport status. Smithy says DescribeRescoreExecutionPlan models ResourceNotFoundException with @httpError 404 and ValidationException with @httpError 400, and the service's protocols are awsJson1_0 / rpcv2Cbor. Hypothesis: as an RPC JSON 1.0 service, kendra-ranking returns client-side errors over the wire with HTTP 400 and discriminates by the error type in the body, so the modeled 404 is not what the wire carries.
Emulator: HTTP 404.
Why: a client that classifies by HTTP status rather than by the body's error type sees a transport/routing 404 rather than a modeled service error; 404 also drives different retry and error-mapping behavior in SDK and proxy layers than AWS's 400, so a caller's "plan not found" branch is never entered.
Fix: the service's awsJson1_0 error serializer / error-to-status mapping for this service - it should emit the protocol's client-error status (400) rather than each error shape's modeled @httpError.
Traces: kendra-ranking-error-paths#task#3 (cluster also covers kendra-ranking-plan-lifecycle-create-update-delete#teardown#7).

Cause 2 of 5: UpdateRescoreExecutionPlan on a nonexistent plan id.
AWS: recorded behavior only - HTTP 400. Smithy: UpdateRescoreExecutionPlan errors include ResourceNotFoundException (@httpError 404) and ValidationException (@httpError 400). The recorded `Message` is 'The specified Resource Execution Plan ID does not exist.' - no rule in the model or AWS docs produces that text, so treat it as an EXAMPLE, not a requirement; the emulator's 'Rescore execution plan <id> was not found.' is not on its own a defect. The requirement here is the status.
Emulator: HTTP 404.
Why: same as cause 1 - the not-found path of an update is reported with a transport status AWS never sends.
Fix: same error-to-status mapping as cause 1, reached from the UpdateRescoreExecutionPlan handler.
Traces: kendra-ranking-error-paths#task#4.

Cause 3 of 5: DeleteRescoreExecutionPlan on a nonexistent plan id.
AWS: recorded behavior only - HTTP 400, same error code as the emulator returns (only `status` diverged). Smithy: DeleteRescoreExecutionPlan errors ResourceNotFoundException (@httpError 404), ValidationException (@httpError 400).
Emulator: HTTP 404.
Why: as cause 1, for the delete path; idempotent-delete logic keyed on the HTTP status misbehaves.
Fix: same error-to-status mapping, reached from the DeleteRescoreExecutionPlan handler.
Traces: kendra-ranking-error-paths#task#5.

Cause 4 of 5: Rescore against a nonexistent rescore execution plan id.
AWS: recorded behavior only - HTTP 400, same error code as the emulator (only `status` diverged). Smithy: Rescore errors ResourceNotFoundException (@httpError 404), ValidationException (@httpError 400), ConflictException (409).
Emulator: HTTP 404.
Why: the data-plane call's not-found path is misclassified exactly as the control-plane ones, so a ranking client cannot distinguish a missing plan from a bad endpoint/path.
Fix: same error-to-status mapping, reached from the Rescore handler.
Traces: kendra-ranking-error-paths#task#6.

Cause 5 of 5: Rescore document validation reports the wrong rule - validation precedence. The request carries a document that both omits Body/TokenizedBody and supplies Title together with TokenizedTitle.
AWS: recorded behavior only - HTTP 400 (status and error code agree with the emulator; only the message diverged). The recorded message is 'Document must have a Body or TokenizedBody.', i.e. AWS evaluates the "one of Body / TokenizedBody must be present" requirement on Rescore.Documents[].Document BEFORE the Title/TokenizedTitle mutual-exclusion check. Smithy documents both rules on the Document shape: Document.TokenizedBody - "You must choose to provide Body or TokenizedBody. You cannot provide both"; Document.TokenizedTitle - "You must choose to provide Title or TokenizedTitle. You cannot provide both". Neither Body nor Title is marked @required, so the presence requirement is recorded behavior; the message is a validation error naming the member and its constraint, so the RULE it states (body-presence checked first) is the requirement, while the exact wording is an example.
Emulator: HTTP 400 reporting the Title/TokenizedTitle mutual-exclusion rule instead ('Document d2 provides both Title and TokenizedTitle; provide one.' as recorded) - i.e. the body-presence check is either absent or ordered after the title check.
Why: a client fixing the reported problem (dropping one of Title/TokenizedTitle) still gets rejected, because the defect AWS reports - no Body and no TokenizedBody - was never surfaced; and a document with neither body form but an unambiguous title may be accepted by the emulator where AWS rejects it.
Fix: the Rescore handler's request validation for Documents[] - add/reorder the one-of Body|TokenizedBody presence check ahead of the Title|TokenizedTitle exclusivity check.
Traces: kendra-ranking-error-paths#task#7.

No open issue covers any of these (no existing kendra issues at all). No cluster carried a stub_path or owned_by, and state_reset_failures was 0, so these are not state-bleed artifacts.

## Evidence
Replay of simparity-wire-aws-v1: 16 steps, 10 passed, 6 mismatched, 0 errored/blocked, score 62.5%, emulator_up=true, bootstrap_signal=false, state_reset_failures=0, divergence_count=5.

Cause 1 - divergence_key service=kendra-ranking|DescribeRescoreExecutionPlan|status=400->404|keys=status; expected status 400, actual 404; detail "status: expected 400, got 404"; affected_count 2 (kendra-ranking-error-paths#task#3, kendra-ranking-plan-lifecycle-create-update-delete#teardown#7).
Cause 2 - service=kendra-ranking|UpdateRescoreExecutionPlan|status=400->404|keys=$.Message,status; expected 400, actual 404; detail "status: expected 400, got 404\n$.Message: value_mismatch (expected 'The specified Resource Execution Plan ID does not exist.', got 'Rescore execution plan 00000000-0000-0000-0000-000000000000 was not found.')" - the status is the requirement; the message texts are examples.
Cause 3 - service=kendra-ranking|DeleteRescoreExecutionPlan|status=400->404|keys=status; expected 400, actual 404.
Cause 4 - service=kendra-ranking|Rescore|status=400->404|keys=status; expected 400, actual 404.
Cause 5 - service=kendra-ranking|Rescore|status=400->400|keys=$.Message; status matches (400) and only the rule reported differs: expected 'Document must have a Body or TokenizedBody.', actual 'Document d2 provides both Title and TokenizedTitle; provide one.'

Model evidence (services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/smithy-model/com-amazonaws-kendraranking-awskendrarerankingfrontendservice.json): sdkId "Kendra Ranking", protocols awsJson1_0 + rpcv2Cbor; ResourceNotFoundException @httpError 404; ValidationException @httpError 400; Document.TokenizedBody/TokenizedTitle docs state the one-of rules; Rescore requires RescoreExecutionPlanId, SearchQuery, Documents.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it; confirm /__admin/health answers 200.
2. Replay fixture kendra-ranking-error-paths (the whole fixture reproduces all five causes).
   - step task#3: DescribeRescoreExecutionPlan with a well-formed, nonexistent 36-char plan id -> emulator answers HTTP 404, recorded AWS answers HTTP 400 with the same error code.
   - step task#4: UpdateRescoreExecutionPlan with that same nonexistent id -> emulator 404, recorded 400.
   - step task#5: DeleteRescoreExecutionPlan with that id -> emulator 404, recorded 400.
   - step task#6: Rescore against that id -> emulator 404, recorded 400.
   - step task#7: Rescore with a document that has neither Body nor TokenizedBody and supplies both Title and TokenizedTitle -> emulator 400 reporting the Title/TokenizedTitle exclusivity rule, recorded 400 reporting the missing Body/TokenizedBody rule.
3. Also replay kendra-ranking-plan-lifecycle-create-update-delete, whose teardown#7 re-describes the deleted plan and hits cause 1 (404 instead of 400).
4. Re-run replay_wire_corpus; all 16 steps must pass.
