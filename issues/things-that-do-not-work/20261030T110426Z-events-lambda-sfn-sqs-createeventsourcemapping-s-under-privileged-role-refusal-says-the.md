Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-rejects-role-without-receive
Issue-Key: events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-rejects-role-without-receive|run#7|AssertionError|assert message == expected_message, (
Issue-Commit: a28b00726e6917844828701198c7c2f9a84a8d63

# CreateEventSourceMapping's under-privileged-role refusal says "The provided execution role" instead of AWS's "The function execution role does not have permissions to call ReceiveMessage on SQS"

> _Note: this issue was identified against the code base at commit `a28b00726e6917844828701198c7c2f9a84a8d63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
This issue carries ONE net-new root cause. It must be closed before submitting; a fixer should re-replay the single check id in trace_ids (events__lambda__sfn__sqs.sqs-to-lambda-esm-rejects-role-without-receive). The replay report left this check UNMARKED (no ALREADY-FILED and no OWNED marker), so no open issue carries its replay key.

=== Cause 1 of 1: the InvalidParameterValueException message for an SQS event source mapping whose function execution role lacks sqs:ReceiveMessage is emulator-authored wording ===
Failing check id: events__lambda__sfn__sqs.sqs-to-lambda-esm-rejects-role-without-receive.

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json) models CreateEventSourceMapping as POST /2015-03-31/event-source-mappings with success code 202 and errors InvalidParameterValueException (400), ResourceConflictException (409), ResourceNotFoundException (404), ServiceException (500) and TooManyRequestsException (429); the model does not carry message text, so the wording is recorded behavior. AWS validates the function's execution role against the SQS source at mapping-CREATION time and refuses with HTTP 400, error code InvalidParameterValueException and the message `The function execution role does not have permissions to call ReceiveMessage on SQS`. That message is WHOLLY fixed text for this refusal class: it names neither the role, the function, the queue nor the queue ARN, and the only thing that varies across the family is the API name of the missing permission and the source service (here ReceiveMessage / SQS, taken from the source type being wired, not from any request value). The refusal must also be a true refusal: no mapping is persisted (ListEventSourceMappings for that function+EventSourceArn is empty), and granting the SAME role sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes must make the IDENTICAL call succeed.

Emulator: the status (400), the error code (InvalidParameterValueException) and the discrimination on permissions are all correct — but the message is rewritten as `The provided execution role does not have permissions to call ReceiveMessage on SQS`. The fixed text diverges in its opening clause: AWS says `The function execution role`, the emulator says `The provided execution role`; the rest of the sentence (`does not have permissions to call ReceiveMessage on SQS`) matches. Replay: `AssertionError: expected CreateEventSourceMapping's message 'The function execution role does not have permissions to call ReceiveMessage on SQS', got 'The provided execution role does not have permissions to call ReceiveMessage on SQS'`.

Why: an error message is part of the contract for this refusal. A client that branches or asserts on the message — IaC tooling surfacing why an SQS trigger could not be wired, CI parity harnesses, and support automation that pattern-matches AWS's sentence to tell "grant the role sqs:ReceiveMessage" apart from other InvalidParameterValueException causes on the same operation (bad BatchSize, unsupported source ARN, nonexistent queue) — never matches, and because the code is shared across all of those causes the message is the ONLY discriminator available.

Fix: the message template in CreateEventSourceMapping's execution-role/source validation (the handler's validation step that probes the function execution role's permissions on the SQS source before the mapping is persisted). Emit AWS's fixed sentence verbatim, with the permission name and source service as the only varying parts of the family.

Related but NOT covering this: open issue 20261029T233239Z-lambda-event-source-mappings-are-created-for-sources.md bundles other CreateEventSourceMapping validation causes (missing function, unsupported source ARN, nonexistent queue, invalid FilterCriteria, missing `Type` in the envelope) from the wire corpus — none of its causes is the execution-role/ReceiveMessage message, and it does not carry this check's replay key. Open issue 20261029T182228Z-events-lambda-sns-sqs-eventbridge-and-sns-never.md reports an ESM that CONSUMES despite a role without receive permission, which is the opposite behavior from the one graded here (this check's refusal does happen; only its wording diverges).

Traces: events__lambda__sfn__sqs.sqs-to-lambda-esm-rejects-role-without-receive (assertion at check_events__lambda__sfn__sqs_sqs_to_lambda_esm_rejects_role_without_receive.py line 47).

## Evidence
Cause 1 — events__lambda__sfn__sqs.sqs-to-lambda-esm-rejects-role-without-receive, verdict mismatch.

Expected: CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<queue arn>) where the function's execution role has NO sqs:ReceiveMessage -> HTTP 400, error code `InvalidParameterValueException`, message exactly `The function execution role does not have permissions to call ReceiveMessage on SQS` (wholly fixed text), and no mapping persisted.

Actual (replay traceback): File "check_events__lambda__sfn__sqs_sqs_to_lambda_esm_rejects_role_without_receive.py", line 47, in run / `assert message == expected_message` / AssertionError: expected CreateEventSourceMapping's message 'The function execution role does not have permissions to call ReceiveMessage on SQS', got 'The provided execution role does not have permissions to call ReceiveMessage on SQS'.

The preceding assertion on the error CODE (`InvalidParameterValueException`) passes, so status and classification agree; the divergence is confined to the fixed opening clause of the message (`The function execution role` vs the emulator's `The provided execution role`). The check's later legs — that no mapping was created, and that granting the same role sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes makes the identical call succeed — were never reached because of this assertion.

Model: CreateEventSourceMapping POST /2015-03-31/event-source-mappings, success 202; errors InvalidParameterValueException(400), ResourceConflictException(409), ResourceNotFoundException(404), ServiceException(500), TooManyRequestsException(429).

## Steps to Reproduce
1. sqs:CreateQueue -> queue URL; sqs:GetQueueAttributes(AttributeNames=[QueueArn]) -> source queue ARN.
2. iam:CreateRole with the standard lambda.amazonaws.com trust policy and NO SQS permissions attached (no sqs:ReceiveMessage).
3. lambda:CreateFunction(FunctionName=<fn>, Role=<that role>, Handler=lambda_function.lambda_handler, Runtime=<python>, Code=<zip returning {"ok": true}>) and wait until Active.
4. lambda:CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<queue arn>, BatchSize=<n>).
   Expected: HTTP 400, code InvalidParameterValueException, message exactly `The function execution role does not have permissions to call ReceiveMessage on SQS`.
   Observed: HTTP 400, code InvalidParameterValueException, message `The provided execution role does not have permissions to call ReceiveMessage on SQS`.
5. lambda:ListEventSourceMappings(FunctionName=<fn>, EventSourceArn=<queue arn>) -> must be empty (nothing persisted by the rejected call).
6. Positive control: iam:PutRolePolicy granting that SAME role sqs:ReceiveMessage, sqs:DeleteMessage and sqs:GetQueueAttributes on the queue ARN, then repeat step 4 unchanged -> must now succeed (202) and return a mapping UUID, attributing the refusal in step 4 to the missing permission specifically.
Replay: events__lambda__sfn__sqs.sqs-to-lambda-esm-rejects-role-without-receive.
