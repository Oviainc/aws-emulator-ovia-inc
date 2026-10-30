Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.esm-creation-fails-for-nonexistent-queue-arn lambda__sns__sqs.sqs-to-lambda-esm-missing-permission
Issue-Key: lambda__sns__sqs.esm-creation-fails-for-nonexistent-queue-arn|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: lambda__sns__sqs.sqs-to-lambda-esm-missing-permission|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 14fcb532a6243119e1963a7bcc0017c1001c0e15

# lambda__sns__sqs: CreateEventSourceMapping's SQS refusal messages diverge — a non-existent queue is reported as a permission failure, and the permission refusal's lead-in is reworded (2 causes)

> _Note: this issue was identified against the code base at commit `14fcb532a6243119e1963a7bcc0017c1001c0e15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two net-new root causes in the lambda CreateEventSourceMapping refusal path for SQS event sources. Both surviving checks are listed in the Issue-Trace-Ids of the open issue 20261029T183635Z-lambda-sns-sqs-sqs-event-source-mappings-never.md (its Cause 2 "accepts a nonexistent queue" and Cause 3 "does not authorize the execution role / wrong error code"), but the emulator now REFUSES both calls with the modelled InvalidParameterValueException at HTTP 400 — so that issue's replay keys (`_client_error#9|AssertionError|raise AssertionError(... it succeeded ...)` and `_client_error#13|...`) no longer describe these failures and neither check is marked owned or already-filed by the replay report. What remains is the refusal MESSAGE, which is a distinct defect and is filed here. The identical wording defect for a different suite is already open as 20261030T105143Z-lambda-secretsmanager-sqs-createeventsourcemapping-s-permission-refusal-uses.md (Cause 2 below is the same fixed-text divergence, reached from this suite's check); Cause 1 is a different defect (a not-found source classified and worded as a permission failure) and is not covered anywhere.

Cause 1 of 2: a well-formed SQS EventSourceArn naming a queue that does not exist is refused with the EXECUTION-ROLE PERMISSION message instead of AWS's queue-does-not-exist message.
AWS: Smithy (lambda CreateEventSourceMapping, POST /2015-03-31/event-source-mappings, success code 202) models InvalidParameterValueException as a client error with httpError 400 and ResourceNotFoundException as 404; the model fixes no message text, so recorded behavior only — AWS refuses the create with HTTP 400, Error.Code `InvalidParameterValueException` and Error.Message `Error occurred while ReceiveMessage. SQS Error Code: AWS.SimpleQueueService.NonExistentQueue. SQS Error Message: The specified queue does not exist.` In that message everything is fixed text except the variable parts taken from the attempted source read: the SQS API action Lambda attempted (`ReceiveMessage`) and the SQS error code/message it propagated verbatim from the source service (`AWS.SimpleQueueService.NonExistentQueue` / `The specified queue does not exist.`). AWS reports the source's own failure, i.e. that the queue is absent — not a permission verdict.
Emulator: the status (400) and Error.Code (`InvalidParameterValueException`) match, but Error.Message is `The provided execution role does not have permissions to call ReceiveMessage on SQS`. The absent queue is evaluated by the role-authorization path (the role's inline policy names only the real queue's ARN, so the fake ARN fails that check) and the authorization message is emitted, so an absent source is diagnosed as an under-privileged role. Hypothesis: the existence/mappability check for the named source does not run before, or does not short-circuit, the execution-role authorization check.
Why: the two conditions have opposite fixes. A client (or an operator reading a deploy failure) that gets "the execution role does not have permissions" for a mistyped queue name goes and widens the IAM policy — which can never make the call succeed — instead of fixing the queue name. Tooling and bootstrap loops that branch on the SQS error code embedded in AWS's message (`AWS.SimpleQueueService.NonExistentQueue`) never see it, so a permanent misconfiguration is retried as a transient permission-propagation one.
Fix: the CreateEventSourceMapping handler's validation ordering and refusal construction (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_event_source_mapping.rs, with crate::state::names_event_source / event_source_is_mappable_by_this_account vs crate::state::event_source_authorization): resolve the EventSourceArn in the SQS store FIRST and, when it resolves to nothing, emit the propagated-source refusal `Error occurred while <action>. SQS Error Code: AWS.SimpleQueueService.NonExistentQueue. SQS Error Message: The specified queue does not exist.` rather than falling through to the authorization message.
Traces: lambda__sns__sqs.esm-creation-fails-for-nonexistent-queue-arn (the same check's code assertion on InvalidParameterValueException passes; only the message assertion fails).

Cause 2 of 2: the missing-source-read-permission refusal uses the fixed lead-in "The provided execution role" where AWS writes "The function execution role".
AWS: recorded behavior only (the Smithy model fixes no message text for InvalidParameterValueException) — a CreateEventSourceMapping whose function execution role lacks sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the named queue is refused with HTTP 400, Error.Code `InvalidParameterValueException` and Error.Message `The function execution role does not have permissions to call ReceiveMessage on SQS`. The whole sentence is fixed text — the lead-in is literally "The function execution role does not have permissions to call" — except two variable parts: the action the role is missing for the source (`ReceiveMessage`) and the source service the ARN names (`SQS`).
Emulator: HTTP 400 and `InvalidParameterValueException` are correct, but Error.Message is `The provided execution role does not have permissions to call ReceiveMessage on SQS` — the fixed lead-in reads "The provided execution role". The variable parts (`ReceiveMessage`, `SQS`) are right.
Why: a client that keys off the refusal text — ESM wiring/retry loops matching AWS's exact sentence, deploy tooling that surfaces or asserts the message, tests pinned to AWS's wording — sees a string AWS never emits, so a correct refusal is classified or reported wrongly even though status and code are right.
Fix: the same handler's authorization-refusal construction (crate::state::event_source_authorization's message) — emit AWS's fixed lead-in "The function execution role does not have permissions to call <action> on <source service>".
Traces: lambda__sns__sqs.sqs-to-lambda-esm-missing-permission (its InvalidParameterValueException code assertion passes; the message assertion on line 79 fails).

Service attribution: this check id's leading token is the composite `lambda__sns__sqs`, so the per-service bootstrap gate cannot place it by exact equality; every constituent service (lambda, sns, sqs) is bootstrapped in this checkout. The defect is in the lambda CreateEventSourceMapping handler.

## Evidence
Cause 1 — failing check lambda__sns__sqs.esm-creation-fails-for-nonexistent-queue-arn [mismatch]:
  File "check_lambda__sns__sqs_esm_creation_fails_for_nonexistent_queue_arn.py", line 61, in run
    assert message == expected, f"expected message {expected!r}, got {message!r}"
  AssertionError: expected message 'Error occurred while ReceiveMessage. SQS Error Code: AWS.SimpleQueueService.NonExistentQueue. SQS Error Message: The specified queue does not exist.', got 'The provided execution role does not have permissions to call ReceiveMessage on SQS'
Expected (AWS, recorded): HTTP 400, Error.Code `InvalidParameterValueException`, Error.Message `Error occurred while ReceiveMessage. SQS Error Code: AWS.SimpleQueueService.NonExistentQueue. SQS Error Message: The specified queue does not exist.` — fixed text apart from the attempted action (`ReceiveMessage`) and the propagated SQS error code/message.
Actual (emulator): HTTP 400, Error.Code `InvalidParameterValueException` (matches), Error.Message `The provided execution role does not have permissions to call ReceiveMessage on SQS` — a permission verdict for an absent queue.

Cause 2 — failing check lambda__sns__sqs.sqs-to-lambda-esm-missing-permission [mismatch]:
  File "check_lambda__sns__sqs_sqs_to_lambda_esm_missing_permission.py", line 79, in run
    assert message == expected, f"expected message {expected!r}, got {message!r}"
  AssertionError: expected message 'The function execution role does not have permissions to call ReceiveMessage on SQS', got 'The provided execution role does not have permissions to call ReceiveMessage on SQS'
Expected (AWS, recorded): HTTP 400, `InvalidParameterValueException`, `The function execution role does not have permissions to call ReceiveMessage on SQS`.
Actual (emulator): HTTP 400, `InvalidParameterValueException` (matches), `The provided execution role does not have permissions to call ReceiveMessage on SQS`.
Diverging part in both causes: the fixed text of the message only; the variable parts are correct.

## Steps to Reproduce
Cause 1:
1. CreateQueue to get a real queue and its QueueArn.
2. CreateRole trusting lambda.amazonaws.com with an inline policy granting sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on THAT queue ARN only; CreateFunction (python3.12) with it; wait State=Active.
3. CreateEventSourceMapping(EventSourceArn=<that real queue arn>, FunctionName=<fn>, BatchSize=10, Enabled=true) — succeeds (positive control).
4. CreateEventSourceMapping(EventSourceArn='arn:aws:sqs:<region>:<account>:a-queue-that-was-never-created', FunctionName=<fn>, BatchSize=10, Enabled=true).
5. Inspect the error: status 400 and Error.Code InvalidParameterValueException match AWS, but Error.Message is 'The provided execution role does not have permissions to call ReceiveMessage on SQS' where AWS emits 'Error occurred while ReceiveMessage. SQS Error Code: AWS.SimpleQueueService.NonExistentQueue. SQS Error Message: The specified queue does not exist.'

Cause 2:
1. CreateQueue to get a queue and its QueueArn.
2. CreateRole trusting lambda.amazonaws.com with an inline policy granting only logs:CreateLogGroup/CreateLogStream/PutLogEvents (no sqs:* at all).
3. CreateFunction (python3.12) with that role; wait until State=Active.
4. CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=<fn>, BatchSize=10, Enabled=true).
5. Inspect the error: Error.Message begins 'The provided execution role ...' where AWS emits 'The function execution role does not have permissions to call ReceiveMessage on SQS'.
