Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager__sqs.sqs-esm-missing-permission-fails
Issue-Key: lambda__secretsmanager__sqs.sqs-esm-missing-permission-fails|run#10|AssertionError|assert error_message == expected_message, (
Issue-Commit: bd90440bd38277082eb102c17511325b543b3bd6

# lambda__secretsmanager__sqs: CreateEventSourceMapping's permission refusal uses the wrong message wording ("The provided execution role ..." instead of "The function execution role ...") (1 cause)

> _Note: this issue was identified against the code base at commit `bd90440bd38277082eb102c17511325b543b3bd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause. The open issue 20261029T193114Z-lambda-secretsmanager-sqs-2-net-new-causes-sqs.md (its Cause 2) covered CreateEventSourceMapping *accepting* a function whose execution role lacks the SQS read permissions; the call is now refused with the right code, but the refusal's message text diverges, so that issue's replay key no longer describes this failure and it is filed here as a distinct cause.

Cause 1 of 1: the refusal message for an event source mapping whose function execution role lacks the source-queue read permissions is worded differently from AWS.
AWS: Smithy (lambda CreateEventSourceMapping) models InvalidParameterValueException as a client error with httpError 400 on POST /2015-03-31/event-source-mappings (success code 202); the model does not fix message text, so recorded behavior only — AWS refuses the create with HTTP 400, Error.Code `InvalidParameterValueException` and Error.Message `The function execution role does not have permissions to call ReceiveMessage on SQS`. In that message the whole sentence is fixed text except the two variable parts taken from the request/source: the API action the role is missing for the source (`ReceiveMessage`) and the source service the ARN names (`SQS`). The lead-in is literally "The function execution role does not have permissions to call".
Emulator: the same request is refused with HTTP 400 and Error.Code `InvalidParameterValueException` (correct), but Error.Message is `The provided execution role does not have permissions to call ReceiveMessage on SQS` — the fixed lead-in reads "The provided execution role" where AWS writes "The function execution role". Only the fixed text diverges; the variable parts (`ReceiveMessage`, `SQS`) are right.
Why: a client that keys off the refusal text — ESM bootstrap/retry loops that match the exact AWS sentence (the harness itself retries while "does not have permissions" is seen), deployment tooling that surfaces or asserts the AWS message, and tests pinned to AWS's wording — sees an error string AWS never emits, so the refusal is reported or classified wrongly even though the status and code are right.
Fix: the lambda CreateEventSourceMapping handler's validation path, where the InvalidParameterValueException for a missing source-read permission is constructed — emit AWS's fixed lead-in "The function execution role does not have permissions to call <action> on <source service>".
Traces: lambda__secretsmanager__sqs.sqs-esm-missing-permission-fails

Note on service attribution: this check id's leading token is the composite `lambda__secretsmanager__sqs`, so the per-service bootstrap gate cannot place it by exact equality; every constituent service (lambda, secretsmanager, sqs) is bootstrapped. The defect is in the lambda CreateEventSourceMapping handler.

## Evidence
Failing check lambda__secretsmanager__sqs.sqs-esm-missing-permission-fails [mismatch]:

  File "check_lambda__secretsmanager__sqs_sqs_esm_missing_permission_fails.py", line 62, in run
    assert error_message == expected_message, (
  AssertionError: expected message 'The function execution role does not have permissions to call ReceiveMessage on SQS', got 'The provided execution role does not have permissions to call ReceiveMessage on SQS'

Expected (AWS, recorded): HTTP 400, Error.Code `InvalidParameterValueException`, Error.Message `The function execution role does not have permissions to call ReceiveMessage on SQS`.
Actual (emulator): HTTP 400, Error.Code `InvalidParameterValueException` (matches — the check's code assertion on line 57 passed), Error.Message `The provided execution role does not have permissions to call ReceiveMessage on SQS`.

Diverging part: the fixed lead-in only — "The provided execution role" vs AWS's "The function execution role". The variable parts of the message (the action `ReceiveMessage`, the source service `SQS`) are correct.

## Steps to Reproduce
1. CreateQueue to get an SQS queue and its QueueArn.
2. CreateRole trusting lambda.amazonaws.com with an inline policy granting only logs:CreateLogGroup/CreateLogStream/PutLogEvents (no sqs:ReceiveMessage, sqs:DeleteMessage or sqs:GetQueueAttributes).
3. CreateFunction (python3.12) with that role; wait until State=Active.
4. Call CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=<fn>, BatchSize=10, Enabled=true).
5. Inspect the error: the status (400) and Error.Code (InvalidParameterValueException) match AWS, but Error.Message begins "The provided execution role ..." where AWS emits "The function execution role does not have permissions to call ReceiveMessage on SQS".
