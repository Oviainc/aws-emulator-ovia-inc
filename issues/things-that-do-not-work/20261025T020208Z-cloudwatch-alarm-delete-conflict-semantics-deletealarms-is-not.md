Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.resource-not-found-error-family cloudwatch.double-delete-returns-not-found-not-success cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected cloudwatch.resource-conflict-error-precondition cloudwatch.cw-delete-alarms-two-composites-rejected-and-nothing-deleted cloudwatch.put-log-alarm-name-conflict-with-existing-metric-alarm cloudwatch.put-log-alarm-name-conflict-with-metric-alarm-rejected cloudwatch.put-log-alarm-resource-conflict
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch|DeleteAlarms|ResourceNotFound
Issue-Key: cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected|_error#23|AssertionError|raise AssertionError(f"{why}; the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.resource-conflict-error-precondition|_err#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: cloudwatch.cw-delete-alarms-two-composites-rejected-and-nothing-deleted|run#19|AssertionError|assert "only one composite alarm delete is allowed" in message, (
Issue-Key: cloudwatch.put-log-alarm-name-conflict-with-existing-metric-alarm|run#19|AssertionError|assert error.get("Code") == "ValidationError", f"expected ValidationError, got {error!r}"
Issue-Key: cloudwatch.put-log-alarm-name-conflict-with-metric-alarm-rejected|run#23|AssertionError|assert error.get("Code") == "ValidationError", f"expected ValidationError, got {error!r}"
Issue-Key: cloudwatch|PutLogAlarm|ResourceConflict
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# CloudWatch alarm delete/conflict semantics: DeleteAlarms is not idempotent, a composite's referenced children can be deleted, and name conflicts answer ResourceConflict instead of ValidationError (4 causes)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAlarms

## Description
Four net-new root causes in the CloudWatch alarm delete/conflict surface.

Cause 1 of 4: DeleteAlarms is not idempotent for names that do not exist.
AWS: Smithy models ResourceNotFound (404) and ResourceConflict (409) on DeleteAlarms, but the recorded behavior is that CloudWatch's delete family (DeleteAlarms / DeleteDashboards / DeleteMetricStream / DeleteAlarmMuteRule / DeleteInsightRules) treats a merely-absent name as a clean no-op success — a second delete of the same alarm, and a delete of a never-created name, both return 200 with an empty body. (ResourceNotFound is reserved for the genuinely precondition-enforcing delete, DeleteAnomalyDetector.)
Emulator: answers 404 ResourceNotFound with 'no alarm named <name>' for both the second delete and the never-created name.
Why: idempotent teardown is the normal cleanup idiom; every cleanup path, CloudFormation-style delete and retry now fails on a resource that is already gone, and a retried delete turns a success into an error.
Fix: not-found handling in the DeleteAlarms handler (and the same family's delete handlers).
Traces: cloudwatch.resource-not-found-error-family, cloudwatch.double-delete-returns-not-found-not-success.

Cause 2 of 4: DeleteAlarms of an alarm a live composite alarm depends on is allowed.
AWS: deleting a metric alarm named by a live composite alarm's AlarmRule — or set as its ActionsSuppressor — would leave the composite with a dangling reference and is refused 400 ValidationError; the alarm must still exist afterwards.
Emulator: the call returns HTTP 200 with an empty body and the leaf alarm is deleted, leaving the composite referencing a nonexistent child. The same acceptance occurs for the ActionsSuppressor reference.
Why: a client can silently corrupt a composite alarm, and the composite then evaluates against a child that no longer exists.
Fix: referential-integrity precondition in the DeleteAlarms handler against composite AlarmRule / ActionsSuppressor references in the alarm state.
Traces: cloudwatch.composite-alarm-rule-cycle-and-unknown-child-rejected, cloudwatch.resource-conflict-error-precondition.

Cause 3 of 4: the multi-composite DeleteAlarms refusal does not name the constraint it enforced.
AWS: one DeleteAlarms call may name at most one composite alarm; the refusal names that constraint in AWS's own wording.
Emulator: refuses, but with 'AlarmNames names 2 composite alarms, and one call can delete no more than one' — a rewritten text that does not state the enforced constraint in the AWS form.
Why: clients and operators key on the refusal text to understand that composite deletes must be serialized.
Fix: the refusal message in DeleteAlarms composite-count validation.
Traces: cloudwatch.cw-delete-alarms-two-composites-rejected-and-nothing-deleted.

Cause 4 of 4: an alarm-name collision across alarm types answers ResourceConflict instead of ValidationError.
AWS: creating an alarm whose AlarmName is already taken by an existing MetricAlarm of a different kind (e.g. PutLogAlarm over a name held by a metric alarm) is refused 400 ValidationError, and the existing alarm is left unchanged. Note Smithy models only LimitExceededFault on PutMetricAlarm/PutCompositeAlarm, so ResourceConflict is not an error this create path models at all.
Emulator: answers {'Code': 'ResourceConflict', 'QueryErrorCode': 'ResourceConflict', 'Type': 'Sender', 'Message': 'a MetricAlarm named <name> already exists in this Region'}.
Why: a client branching on ValidationError for a name collision never takes that branch, and a 409-shaped ResourceConflict is commonly treated as retryable when it is not.
Fix: error classification for name collisions in the alarm create handlers.
Traces: cloudwatch.put-log-alarm-name-conflict-with-existing-metric-alarm, cloudwatch.put-log-alarm-name-conflict-with-metric-alarm-rejected, cloudwatch.put-log-alarm-resource-conflict.

## Evidence
Cause 1: "AssertionError: DeleteAlarms: expected a nonexistent alarm name to be a no-op success, got {'Message': 'no alarm named sim2real-behav-cwprobe-alarm-missing-...', 'Code': 'ResourceNotFound', 'QueryErrorCode': 'ResourceNotFound', 'Type': 'Sender'}"; "botocore.errorfactory.ResourceNotFound: An error occurred (ResourceNotFound) when calling the DeleteAlarms operation: no alarm named sim2real-behav-cwprobe-alarm-dd-..." raised on the second, intentionally idempotent delete.
Cause 2: "AssertionError: <leaf> is named by <composite>'s rule, so deleting it would leave the composite with a dangling reference and must be refused; the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}"; "AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}" at `leaf_err = _err(lambda: cw.delete_alarms(AlarmNames=[leaf]))`.
Cause 3: "AssertionError: the refusal must name the constraint it enforced, got 'AlarmNames names 2 composite alarms, and one call can delete no more than one'".
Cause 4: "AssertionError: expected ValidationError, got {'Message': 'a MetricAlarm named sim2real-logalarm-conflict-... already exists in this Region', 'Code': 'ResourceConflict', 'QueryErrorCode': 'ResourceConflict', 'Type': 'Sender'}" (three checks, same shape).

## Steps to Reproduce
1. PutMetricAlarm(AlarmName=a); DeleteAlarms([a]); DeleteAlarms([a]) again -> 404 ResourceNotFound instead of a no-op 200. DeleteAlarms(['never-created']) -> same.
2. PutMetricAlarm(AlarmName=leaf); PutCompositeAlarm(AlarmName=c, AlarmRule="ALARM(leaf)"); DeleteAlarms([leaf]) -> 200 and the leaf is gone (AWS: 400 ValidationError, leaf retained). Repeat with leaf set as c's ActionsSuppressor.
3. DeleteAlarms naming two composite alarms -> refusal message does not state the AWS constraint.
4. PutMetricAlarm(AlarmName=n); create a log alarm with the same AlarmName=n -> ResourceConflict instead of ValidationError.
