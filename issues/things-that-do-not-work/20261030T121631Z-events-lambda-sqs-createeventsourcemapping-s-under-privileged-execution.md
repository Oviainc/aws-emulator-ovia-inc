Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll events__lambda__sqs.lambda-without-sqs-permission-esm-creation-fails events__lambda__sqs.sqs-lambda-event-source-mapping-missing-role-permission-rejected
Issue-Key: events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll|run#9|AssertionError|assert error_message == expected_message, (
Issue-Key: events__lambda__sqs.lambda-without-sqs-permission-esm-creation-fails|run#11|AssertionError|assert err_message == expected_message, (
Issue-Key: events__lambda__sqs.sqs-lambda-event-source-mapping-missing-role-permission-rejected|run#8|AssertionError|assert message == expected_message, (
Issue-Commit: 8a62bc5d8898770ac63ef3e09ff3986bf7746acd

# events__lambda__sqs: CreateEventSourceMapping's under-privileged-execution-role refusal says "The provided execution role" instead of AWS's "The function execution role does not have permissions to call ReceiveMessage on SQS" (1 cause)

> _Note: this issue was identified against the code base at commit `8a62bc5d8898770ac63ef3e09ff3986bf7746acd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
This issue carries ONE net-new root cause. It must be closed before submitting; a fixer should re-replay all three check ids in trace_ids. The replay report left all three checks UNMARKED (no ALREADY-FILED marker and no OWNED marker), so no open issue carries their replay key.

=== Cause 1 of 1: the InvalidParameterValueException message for an SQS event source mapping whose function execution role cannot poll the source queue is emulator-authored wording ===
Failing check ids: events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll, events__lambda__sqs.lambda-without-sqs-permission-esm-creation-fails, events__lambda__sqs.sqs-lambda-event-source-mapping-missing-role-permission-rejected.

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json) models CreateEventSourceMapping as POST /2015-03-31/event-source-mappings with success code 202 and errors InvalidParameterValueException (errorKind client, httpError 400), ResourceConflictException (409), ResourceNotFoundException (404), ServiceException (500), TooManyRequestsException (429). The model carries no message text, so the wording is recorded behavior only. AWS validates the function's execution role against the SQS source at mapping-CREATION time and refuses with HTTP 400, error code InvalidParameterValueException and the message `The function execution role does not have permissions to call ReceiveMessage on SQS`. That sentence is WHOLLY FIXED TEXT for this refusal class: it names neither the role, the function, the queue nor the queue ARN. The only parts that vary across the family are the API name of the missing permission and the source service (here `ReceiveMessage` / `SQS`), and those are derived from the source type being wired, not copied from any request value. The refusal must also be a true refusal: no mapping is persisted, and granting the SAME role sqs:ReceiveMessage/sqs:DeleteMessage/sqs:GetQueueAttributes must make the IDENTICAL call succeed (202).

Emulator: the HTTP status (400), the error code (InvalidParameterValueException) and the permission discrimination are all correct — only the message is rewritten, as `The provided execution role does not have permissions to call ReceiveMessage on SQS`. The divergence is confined to the opening clause of the fixed text: AWS says `The function execution role`, the emulator says `The provided execution role`; the remainder (`does not have permissions to call ReceiveMessage on SQS`) matches exactly. All three checks fail on the same assertion with the same expected/actual pair.

Why: for this operation the error message is the only discriminator a client has. InvalidParameterValueException is shared by every CreateEventSourceMapping validation cause (bad BatchSize, unsupported source ARN, nonexistent queue, invalid FilterCriteria, under-privileged role), so IaC tooling, CI parity harnesses and support automation that pattern-match AWS's sentence to tell "grant the execution role sqs:ReceiveMessage" apart from the other causes never match, and report the wrong remediation or none at all.

Fix: the message template in CreateEventSourceMapping's execution-role/source validation step — the handler validation that probes the function execution role's permissions on the SQS source before the mapping is persisted. Emit AWS's fixed sentence verbatim, with the permission name and source service as the only varying parts of the family.

Related but NOT covering this: open issue 20261030T110426Z-events-lambda-sfn-sqs-createeventsourcemapping-s-under-privileged-role-refusal-says-the.md reports the same wording defect for the events__lambda__sfn__sqs check id, 20261030T110325Z-lambda-s3-sqs-createeventsourcemapping-s-unauthorized-execution-role.md for lambda__s3__sqs and 20261030T105143Z-lambda-secretsmanager-sqs-createeventsourcemapping-s-permission-refusal-uses.md for lambda__secretsmanager__sqs; none of them carries the three replay keys listed here, which the report left unmarked. Open issue 20261029T182228Z-events-lambda-sqs-eventbridge-sqs-targets-ignore-the.md owns neighbouring events__lambda__sqs checks but not these.

Traces: the three check ids above (assertions at check_events__lambda__sqs_lambda_role_missing_sqs_permissions_blocks_poll.py line 57, check_events__lambda__sqs_lambda_without_sqs_permission_esm_creation_fails.py line 54, check_events__lambda__sqs_sqs_lambda_event_source_mapping_missing_role_permission_rejected.py line 49).

## Evidence
All three checks, verdict mismatch, fail on the identical expected/actual pair.

Expected: CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<queue arn>, BatchSize=<n>) where the function's execution role holds no sqs:ReceiveMessage -> HTTP 400, error code `InvalidParameterValueException`, message exactly `The function execution role does not have permissions to call ReceiveMessage on SQS` (wholly fixed text for this class), and no mapping persisted.

Actual (replay tracebacks, identical message on all three):
- events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll — File "check_events__lambda__sqs_lambda_role_missing_sqs_permissions_blocks_poll.py", line 57, in run / `assert error_message == expected_message` / AssertionError: expected CreateEventSourceMapping's message 'The function execution role does not have permissions to call ReceiveMessage on SQS', got 'The provided execution role does not have permissions to call ReceiveMessage on SQS'
- events__lambda__sqs.lambda-without-sqs-permission-esm-creation-fails — File "check_events__lambda__sqs_lambda_without_sqs_permission_esm_creation_fails.py", line 54, in run / `assert err_message == expected_message` / AssertionError: expected CreateEventSourceMapping's message 'The function execution role does not have permissions to call ReceiveMessage on SQS', got 'The provided execution role does not have permissions to call ReceiveMessage on SQS'
- events__lambda__sqs.sqs-lambda-event-source-mapping-missing-role-permission-rejected — File "check_events__lambda__sqs_sqs_lambda_event_source_mapping_missing_role_permission_rejected.py", line 49, in run / `assert message == expected_message` / AssertionError: expected CreateEventSourceMapping's message 'The function execution role does not have permissions to call ReceiveMessage on SQS', got 'The provided execution role does not have permissions to call ReceiveMessage on SQS'

In every case the preceding assertion on the error CODE (`InvalidParameterValueException`) passes, so status and classification agree and the divergence is confined to the fixed opening clause of the message. Each check's remaining legs (no mapping persisted; the same role plus sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes making the identical call succeed and the mapping go Enabled) were never reached because of this assertion.

Model: CreateEventSourceMapping POST /2015-03-31/event-source-mappings, success 202; errors InvalidParameterValueException(400), ResourceConflictException(409), ResourceNotFoundException(404), ServiceException(500), TooManyRequestsException(429). No message text is modeled.

## Steps to Reproduce
1. sqs:CreateQueue -> queue URL; sqs:GetQueueAttributes(AttributeNames=[QueueArn]) -> source queue ARN.
2. iam:CreateRole with the standard lambda.amazonaws.com trust policy and NO SQS permissions (only logs:CreateLogGroup/CreateLogStream/PutLogEvents, or nothing at all).
3. lambda:CreateFunction(FunctionName=<fn>, Role=<that role>, Handler=<handler>, Runtime=<python>, Code=<zip returning {"ok": true}>); wait until Active.
4. lambda:CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<queue arn>, BatchSize=10).
   Expected: HTTP 400, code InvalidParameterValueException, message exactly `The function execution role does not have permissions to call ReceiveMessage on SQS`.
   Observed: HTTP 400, code InvalidParameterValueException, message `The provided execution role does not have permissions to call ReceiveMessage on SQS`.
5. lambda:ListEventSourceMappings(FunctionName=<fn>, EventSourceArn=<queue arn>) -> must be empty; the rejected call must persist nothing.
6. Positive control: iam:PutRolePolicy granting that SAME role sqs:ReceiveMessage, sqs:DeleteMessage and sqs:GetQueueAttributes on the queue ARN, then repeat step 4 unchanged -> must succeed (202) with a mapping UUID that reaches Enabled, attributing the step-4 refusal to the missing permission specifically.
Replay: events__lambda__sqs.lambda-role-missing-sqs-permissions-blocks-poll, events__lambda__sqs.lambda-without-sqs-permission-esm-creation-fails, events__lambda__sqs.sqs-lambda-event-source-mapping-missing-role-permission-rejected.
