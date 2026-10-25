Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.put-index-policy-then-describe logs.put-index-policy-oversized-document-rejected logs.op-contract-delete-resource-policy-stale-revision-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.put-index-policy-then-describe|run#17|AssertionError|assert message == "Specified resource does not exist!", f"unexpected message: {message!r}"
Issue-Key: logs.put-index-policy-oversized-document-rejected|run#21|AssertionError|assert delete_msg == "Specified resource does not exist!", f"got {delete_msg!r}"
Issue-Key: logs.op-contract-delete-resource-policy-stale-revision-rejected|run#37|AssertionError|assert message == (
Issue-Commit: 949cf2f2a2f42313b725a3f96914da0fa4cf14da

# CloudWatch Logs error messages are invented: DeleteIndexPolicy's not-found and DeleteResourcePolicy's stale-revision refusals use emulator-authored text (2 causes)

> _Note: this issue was identified against the code base at commit `949cf2f2a2f42313b725a3f96914da0fa4cf14da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIndexPolicy

## Description
Two net-new root causes in the CloudWatch Logs emulator. Both refusals carry the RIGHT error code and HTTP status but a message the emulator authors itself instead of the text AWS returns for that error class, so a client that matches on the message (or a recorded-response comparison) diverges. Neither is covered by an open issue: the closest open items are 20261023T113357Z-logs-cloudwatch-logs-startlivetail-and-getlogobject-answer-501-three.md (other operations' not-found paths) and 20261024T150703Z-logs-cloudwatch-logs-metric-filters-emit-no-cloudwatch-metric.md, and neither names DeleteIndexPolicy or the DeleteResourcePolicy revision guard.

=== Cause 1 of 2: DeleteIndexPolicy's ResourceNotFoundException message ===
AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json): DeleteIndexPolicy declares ResourceNotFoundException, errorKind client, httpError 400, doc "The specified resource does not exist." The model does not state message strings, so the wording is recorded behavior only: deleting an index policy that is not there is refused with HTTP 400, code ResourceNotFoundException and the message "Specified resource does not exist!". That message is ENTIRELY fixed text for this error class — it interpolates nothing from the request or from state (no log group name, no policy name, no resource kind).
Emulator: same code and status, but the message is "The specified index policy does not exist." — built by crate::state::resource_not_found_message("index policy"), i.e. a template that splices an emulator-chosen resource-kind word into emulator-chosen prose.
Why: a client (or an SDK waiter/retry predicate, or a recorded-response assertion) that distinguishes "policy already gone" by the service's not-found message never matches, and anything that logs or surfaces the message shows text AWS never emits. The same helper is used for the log-group-absent path of this and other Logs operations, so every refusal routed through it reports non-AWS prose.
Fix: the DeleteIndexPolicy handler's not-found construction and the shared message helper it calls — the refusal must carry AWS's fixed ResourceNotFoundException text for this service rather than a per-resource-kind sentence.
Traces: logs.put-index-policy-then-describe (AssertionError: unexpected message: 'The specified index policy does not exist.'; the check asserts message == "Specified resource does not exist!" after deleting an already-deleted index policy, with code ResourceNotFoundException already matching) and logs.put-index-policy-oversized-document-rejected (same assertion, same actual text; its earlier legs — InvalidParameterException for a >51200-byte policyDocument, the small-policy round-trip and DescribeIndexPolicies — all passed, so the ONLY divergence is this message). Emulator code: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/delete_index_policy.rs lines 148-156 (the not_found closure) and line 172 (return Err(not_found("index policy"))).

=== Cause 2 of 2: DeleteResourcePolicy's stale-expectedRevisionId OperationAbortedException message ===
AWS: Smithy: DeleteResourcePolicy input carries expectedRevisionId (ExpectedRevisionId, length min 1) and declares OperationAbortedException, errorKind client, httpError 400, doc "Multiple concurrent requests to update the same resource were in conflict." Recorded behavior: a resource-scoped delete (resourceArn alone, policyName omitted) presenting a revision id that was real but has since been superseded is refused with HTTP 400, code OperationAbortedException and the message "A conflicting operation is currently in progress against this resource. Please try again." — again entirely fixed text for the error class, interpolating no revision id, policy name or ARN.
Emulator: correct code and status, but the message is "expectedRevisionId does not match the current revision of this resource policy" — emulator-authored prose that names the request member instead of AWS's conflict sentence.
Why: AWS reports a lost revision race as a conflict a caller should re-read-and-retry; a caller keying its retry/abort decision on that message (the only thing distinguishing it from other 400s once code handling is shared) takes the wrong branch, and recorded-response comparison fails.
Fix: the DeleteResourcePolicy handler's revision-guard branch — keep the guard and the OperationAbortedException classification, emit AWS's fixed OperationAbortedException message.
Traces: logs.op-contract-delete-resource-policy-stale-revision-rejected (AssertionError: unexpected message: 'expectedRevisionId does not match the current revision of this resource policy'; the code assertion OperationAbortedException passed first). Emulator code: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/delete_resource_policy.rs lines 258-270.

## Evidence
Cause 1 — expected (AWS, recorded): HTTP 400, code ResourceNotFoundException, message "Specified resource does not exist!"; actual (emulator): HTTP 400, code ResourceNotFoundException, message "The specified index policy does not exist." Observed twice: logs.put-index-policy-then-describe -> AssertionError: unexpected message: 'The specified index policy does not exist.'; logs.put-index-policy-oversized-document-rejected -> AssertionError: got 'The specified index policy does not exist.' (assert delete_msg == "Specified resource does not exist!").

Cause 2 — expected (AWS, recorded): HTTP 400, code OperationAbortedException, message "A conflicting operation is currently in progress against this resource. Please try again."; actual (emulator): HTTP 400, code OperationAbortedException, message "expectedRevisionId does not match the current revision of this resource policy". Observed in logs.op-contract-delete-resource-policy-stale-revision-rejected -> AssertionError: unexpected message: 'expectedRevisionId does not match the current revision of this resource policy'.

In both causes the error code and the 400 status already match, and in both the AWS message is fixed text with no request- or state-derived parts; the emulator substitutes its own sentence.

## Steps to Reproduce
Cause 1 (either check reproduces it):
1. logs.CreateLogGroup(logGroupName=<group>).
2. logs.PutIndexPolicy(logGroupIdentifier=<group>, policyDocument='{"Fields":["requestId"]}') and confirm DescribeIndexPolicies returns it.
3. logs.DeleteIndexPolicy(logGroupIdentifier=<group>) — succeeds; DescribeIndexPolicies is now empty.
4. logs.DeleteIndexPolicy(logGroupIdentifier=<group>) again. Expect 400 ResourceNotFoundException with message "Specified resource does not exist!"; the emulator answers "The specified index policy does not exist."

Cause 2:
1. logs.CreateLogGroup(logGroupName=<group>); note the log group ARN.
2. logs.PutResourcePolicy scoped with resourceArn=<log group ARN>; read revisionId via DescribeResourcePolicies (= old_revision).
3. logs.PutResourcePolicy again on the same resourceArn so the stored revisionId changes (= new_revision != old_revision).
4. logs.DeleteResourcePolicy(resourceArn=<log group ARN>, expectedRevisionId=old_revision) with policyName OMITTED. Expect 400 OperationAbortedException with message "A conflicting operation is currently in progress against this resource. Please try again."; the emulator answers "expectedRevisionId does not match the current revision of this resource policy".
5. Positive control: the identical call with expectedRevisionId=new_revision is accepted, confirming the guard itself works and only the message diverges.
