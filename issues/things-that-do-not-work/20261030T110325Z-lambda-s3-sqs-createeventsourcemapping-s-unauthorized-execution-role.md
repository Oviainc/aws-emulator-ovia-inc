Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.deploy-missing-esm-source-arn-permission-negative
Issue-Key: lambda__s3__sqs.deploy-missing-esm-source-arn-permission-negative|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: bd90440bd38277082eb102c17511325b543b3bd6

# lambda__s3__sqs: CreateEventSourceMapping's unauthorized-execution-role refusal uses the wrong message text (1 cause)

> _Note: this issue was identified against the code base at commit `bd90440bd38277082eb102c17511325b543b3bd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause in the lambda__s3__sqs group: the message of the refusal Lambda emits when the invoked function's execution role may not read the named event source is worded differently from AWS's.

Cause 1 of 1: CreateEventSourceMapping, 400 InvalidParameterValueException — wrong error message text for the unauthorized-execution-role refusal.

AWS: Smithy lambda#CreateEventSourceMapping (POST /2015-03-31/event-source-mappings, 202) binds InvalidParameterValueException (400, body {Type, message}) among its errors; the model documents the error class but not its text, so the message is recorded behavior only. For an SQS event source whose invoking function's execution role carries no sqs:ReceiveMessage on that queue, AWS refuses the create synchronously with 400 / InvalidParameterValueException and the message 'The function execution role does not have permissions to call ReceiveMessage on SQS'. That message is wholly FIXED text for this error class — nothing from the request or from state is interpolated into it (the action name 'ReceiveMessage' and the source word 'SQS' are the fixed per-source-type parts; no role ARN, queue ARN or function name appears). No mapping is persisted: ListEventSourceMappings for that function answers an empty EventSourceMappings list.

Emulator: the status (400) and the code (InvalidParameterValueException) are right, but the message reads 'The provided execution role does not have permissions to call ReceiveMessage on SQS' — the fixed leading phrase is 'The provided execution role' where AWS says 'The function execution role'. Replay assertion: expected message 'The function execution role does not have permissions to call ReceiveMessage on SQS', got 'The provided execution role does not have permissions to call ReceiveMessage on SQS'.

Why: the message is the only part of this refusal that distinguishes a missing-permission rejection from the other InvalidParameterValueException rejections of the same operation (unrecognized source, nonexistent queue, visibility-timeout conflict), all of which share status 400 and that one code. Deployment tooling, retry wrappers and operator runbooks match on its fixed text to decide 'fix the execution role' rather than 'fix the source ARN'; against the emulator that match fails and the real cause is misattributed or the refusal is retried blindly.

Fix: belongs in the lambda service's event-source authorization refusal text, not in the operation's control flow — crate::state::unreadable_event_source / event_source_authorization in services/service-com-amazonaws-lambda-awsgirapiservice/src/state.rs, whose message is returned through invalid_parameter(..) by ops/create_event_source_mapping.rs at the unreadable_event_source branch (around lines 568-575). The refusal itself, the 400 status, the code and the no-mapping-persisted behaviour are already correct; only the fixed phrase must match AWS's wording.

Related, and NOT the same cause: open issue 20261029T233239Z-lambda-event-source-mappings-are-created-for-sources.md covers CreateEventSourceMapping failing to validate the SOURCE and the FilterCriteria (wrong status/code and missing Type on those paths); it does not touch the execution-role authorization message, and this check was not reported as owned by it.

Traces: lambda__s3__sqs.deploy-missing-esm-source-arn-permission-negative.

## Evidence
Behavioral check lambda__s3__sqs.deploy-missing-esm-source-arn-permission-negative [mismatch]:
  AssertionError: expected message 'The function execution role does not have permissions to call ReceiveMessage on SQS', got 'The provided execution role does not have permissions to call ReceiveMessage on SQS'

Expected (AWS): HTTP 400, Error.Code = InvalidParameterValueException, Error.Message = 'The function execution role does not have permissions to call ReceiveMessage on SQS' (fixed text, no interpolated values), and ListEventSourceMappings(FunctionName=<fn>) = [].
Actual (emulator): HTTP 400, Error.Code = InvalidParameterValueException (both correct), Error.Message = 'The provided execution role does not have permissions to call ReceiveMessage on SQS'. The check's earlier code assertion passed and the mapping-not-persisted assertion was never reached, so only the message text diverges.

## Steps to Reproduce
1. CreateRole with a trust policy for lambda.amazonaws.com and an inline policy granting only logs:CreateLogGroup/CreateLogStream/PutLogEvents on '*' (no sqs:* at all).
2. CreateFunction (python3.12, ZipFile) with that role; wait for State Active.
3. CreateQueue and read its QueueArn via GetQueueAttributes.
4. Call lambda CreateEventSourceMapping(EventSourceArn=<that queue ARN>, FunctionName=<that function>, BatchSize=1).
5. Observe the refusal: expect 400 InvalidParameterValueException with message 'The function execution role does not have permissions to call ReceiveMessage on SQS'; the emulator answers the same status and code with 'The provided execution role does not have permissions to call ReceiveMessage on SQS'.
Replay key: behavioral check lambda__s3__sqs.deploy-missing-esm-source-arn-permission-negative.
