Issue-Source: trace-sweep-divergence
Issue-Trace: logs-b2-policy-suite-with-data-protection
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-b2-policy-suite-with-data-protection logs-resource-and-account-policy-management logs-log-group-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DeleteResourcePolicy|4xx->4xx|$.message
Issue-Key: logs|PutRetentionPolicy|4xx->4xx|$.message
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# DeleteResourcePolicy and PutRetentionPolicy refusals use emulator-authored messages

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteResourcePolicy

## Description
Two CloudWatch Logs refusals carry messages whose producing rule IS known -- one is built from the request, the other from the member's documented value set -- yet the emulator substitutes its own generic text. Status and error code match on both; only the message diverges, and in both cases it diverges from a rule, not merely from a recorded string. This bundle has 2 causes; both must be closed.

Cause 1 of 2 -- DeleteResourcePolicy's not-found message does not name the policy that was not found.
AWS: Smithy -- DeleteResourcePolicy.policyName targets PolicyName; the operation models InvalidParameterException, OperationAbortedException, ResourceNotFoundException and ServiceUnavailableException, all client errors at 400. Recorded: deleting a resource policy that does not exist is refused at 400 with a message built from the request -- it names the requested policy in brackets and states that policy does not exist. Because the message interpolates a request member, the rule producing it is known: the refusal must identify the policy name the caller asked for.
Emulator: same 400 and same error code, but a fixed generic string that names nothing.
Why: when a client deletes policies in a loop, the service's message tells it WHICH policy was missing; the emulator's does not, so a caller cannot attribute the failure to a specific policy and cannot log an actionable reason. Tooling that extracts the policy name from the refusal gets nothing.
Fix: the DeleteResourcePolicy handler's not-found path -- build the message from the requested policyName.
Traces: logs-b2-policy-suite-with-data-protection#task#9 (also logs-resource-and-account-policy-management#teardown#6).

Cause 2 of 2 -- PutRetentionPolicy's invalid-retention message does not state the valid retention values.
AWS: Smithy -- PutRetentionPolicy.retentionInDays targets Days (required), whose own documentation enumerates the permitted values: "Possible values are: 1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, and 3653." The operation models InvalidParameterException, OperationAbortedException, ResourceNotFoundException and ServiceUnavailableException. Recorded: an unsupported retention value is refused at 400 with a message that declares the value invalid and then lists the valid set -- a rule the model states directly, so the message's content is a requirement, not just a recorded string.
Emulator: same 400 and code, but a differently structured message that leads with the submitted value and the emulator's own phrasing rather than the service's "invalid retention value / valid values are [...]" form.
Why: the valid-values list in the refusal is the only machine-readable hint a caller gets about which retention periods it may use; a client or operator that reads the message to pick a legal value cannot do so from the emulator's phrasing, and message-shape assertions in client test suites fail.
Fix: the PutRetentionPolicy input validation -- emit the service's invalid-retention message form, enumerating the Days value set from the model.
Traces: logs-log-group-configuration-lifecycle#task#8 (also the same fixture's task#11).

## Evidence
Cause 1 (logs-b2-policy-suite-with-data-protection#task#9, 2 affected steps; status 400 and error code match on both sides): $.message expected 'Policy with name [<requested policy name>] does not exist.', got 'The specified resource policy does not exist'. The requirement is that the message is built from the request and names the requested policy; the bracketed recorded text is an example.
Cause 2 (logs-log-group-configuration-lifecycle#task#8, 2 affected steps; status 400 and error code match on both sides): $.message expected 'Invalid retention value. Valid values are: [1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, 3653]', got 'retentionInDays value 4 is not one of 1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922, 3288, 3653'.
Smithy citations: DeleteResourcePolicy input {policyName -> PolicyName, resourceArn -> Arn, expectedRevisionId}, errors [InvalidParameterException, OperationAbortedException, ResourceNotFoundException, ServiceUnavailableException]; PutRetentionPolicy.retentionInDays -> Days (required), whose doc enumerates the 22 permitted values; errors [InvalidParameterException, OperationAbortedException, ResourceNotFoundException, ServiceUnavailableException].

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay logs-b2-policy-suite-with-data-protection step task#9: DeleteResourcePolicy for a policy name that was never created. The 400 and the error code match the recording, but $.message is a fixed generic string instead of one naming the requested policy.
3. Replay logs-log-group-configuration-lifecycle#task#8: PutRetentionPolicy with retentionInDays=4. The 400 and the error code match, but $.message does not use the service's 'Invalid retention value. Valid values are: [...]' form.
