Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager__sqs.error-propagation-queue-not-exist lambda__secretsmanager__sqs.error-propagation-secret-not-found lambda__secretsmanager__sqs.lambda-sqs-missing-permission-denied lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied lambda__secretsmanager__sqs.sqs-resource-scoped-denies-other-queue
Issue-Key: lambda__secretsmanager__sqs.error-propagation-queue-not-exist|run#13|AssertionError|assert "NonExistentQueue" in (result["payload"]["code"] or ""), (
Issue-Key: lambda__secretsmanager__sqs.error-propagation-secret-not-found|run#8|AssertionError|assert control["payload"]["ok"] is True, f"positive control failed: {control}"
Issue-Key: lambda__secretsmanager__sqs.lambda-sqs-missing-permission-denied|_invoke_until#12|AssertionError|raise AssertionError(
Issue-Key: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: lambda__secretsmanager__sqs.sqs-resource-scoped-denies-other-queue|run#12|AssertionError|assert result["payload"]["ok"] is False, (
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# lambda__secretsmanager__sqs: the in-Lambda SDK reports SQS's error shape name instead of its wire error code, has no Secrets Manager client, skips IAM authorization, and two permission scenarios never settle (4 causes)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
Four net-new root causes. The open issue 20261105T081139Z-lambda-secretsmanager-sqs-the-in-process-lambda-python.md carries five OTHER checks of this suite; it does not carry any of the replay keys below, and cause 1 and cause 3 are not the runtime-SDK gap it describes.

Cause 1 of 4: a refused SQS call inside a handler surfaces the modeled error SHAPE name, not the wire error code.
AWS: SQS's QueueDoesNotExist is serialized with the query-compatible error code `AWS.SimpleQueueService.NonExistentQueue` at HTTP 400 (Smithy: com.amazonaws.sqs#QueueDoesNotExist, errorKind client, httpError 400, awsQueryError {code: "AWS.SimpleQueueService.NonExistentQueue", httpResponseCode: 400}), with message "The specified queue does not exist." An SDK inside a handler therefore reports Error.Code == "AWS.SimpleQueueService.NonExistentQueue"; the shape name QueueDoesNotExist is an internal identifier that never appears on the wire.
Emulator: the handler sees code 'QueueDoesNotExist' — "expected AWS.SimpleQueueService.NonExistentQueue, got {'ok': False, 'error': 'QueueDoesNotExist', 'code': 'QueueDoesNotExist', 'message': 'The specified queue does not exist.'}". The message is right; the code is the shape name. (The SQS endpoint itself does emit the query error code elsewhere in this run, so the substitution happens in the runtime's SDK error mapping.)
Why: production code branches on the error code string; a handler written against AWS (`if code == "AWS.SimpleQueueService.NonExistentQueue"`) takes the wrong branch here, and retry/alarm classification keyed on the code silently stops matching.
Fix: the error-deserialization path of the in-process Lambda runtime's AWS SDK client — map a refusal to its awsQueryError code, not the modeled shape name.
Traces: lambda__secretsmanager__sqs.error-propagation-queue-not-exist.

Cause 2 of 4: the runtime's SDK has no Secrets Manager client, so the check's positive control cannot run.
AWS: a python handler can construct a 'secretsmanager' client; GetSecretValue on an existing secret answers 200 with SecretString/SecretBinary, and a missing secret is refused ResourceNotFoundException (Smithy: GetSecretValue errors ResourceNotFoundException, InvalidParameterException, InvalidRequestException, DecryptionFailure, InternalServiceError).
Emulator: the call never leaves the handler — {'ok': False, 'error': 'UnknownServiceError', 'code': None, 'message': None}; elsewhere in this run the same runtime reports "Unknown service: '<name>'. Valid service names are: sqs, events, eventbridge, dynamodb", i.e. the SDK's registry covers only those four services.
Why: every Secrets Manager behavior reachable only from a handler — including the ResourceNotFoundException propagation this check is about — is unobservable, and the failure the client sees (UnknownServiceError with no code and no message) is not an AWS error at all.
Fix: the in-process Lambda python runtime's bundled SDK service-client registry.
Traces: lambda__secretsmanager__sqs.error-propagation-secret-not-found.

Cause 3 of 4: the execution role's identity policy is not evaluated for an in-function sqs:SendMessage.
AWS: with no sqs:SendMessage Allow for the queue, the handler's send is refused HTTP 403 `AccessDenied` with "User: <principal> is not authorized to perform: sqs:SendMessage on resource: <queue ARN> because no identity-based policy allows the sqs:SendMessage action" (fixed text outside the brackets; principal and queue ARN come from the request and IAM state). Not a modeled SQS error — AWS's documented IAM behavior.
Emulator: the send keeps succeeding for the full 45 s the check polls — "subject (no sqs:SendMessage) send is denied not met within 45.0s. Last result: {'status_code': 200, 'payload': {'ok': True}}".
Why: a role with no grant behaves like a privileged one, so least-privilege regressions are invisible.
Fix: the SQS SendMessage handler's authorization check plus the execution-role caller-identity plumbing for in-process handler calls.
Traces: lambda__secretsmanager__sqs.lambda-sqs-missing-permission-denied.

Cause 4 of 4: two permission-scoping scenarios never reach a verdict.
AWS: a handler without secretsmanager:GetSecretValue is refused AccessDeniedException ("User: <principal> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret ARN> because no identity-based policy allows the secretsmanager:GetSecretValue action"), and a role whose sqs grant is scoped to one queue ARN is refused on any other queue with the same template — both within the recording's budget.
Emulator: both checks are killed by the harness — "Error: Check timed out after 60.0s" — so neither refusal nor its message is observable.
Why: resource-scoped policies are the main blast-radius control; if the scenario never settles, a client cannot tell a correct scoping from a missing one.
Fix: the same authorization path as cause 3 plus the runtime SDK path of cause 2, which these scenarios traverse before they can assert.
Traces: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied, lambda__secretsmanager__sqs.sqs-resource-scoped-denies-other-queue.

## Evidence
Cause 1 — expected error code "AWS.SimpleQueueService.NonExistentQueue" (Smithy awsQueryError on com.amazonaws.sqs#QueueDoesNotExist, HTTP 400, message "The specified queue does not exist."). Actual: {'ok': False, 'error': 'QueueDoesNotExist', 'code': 'QueueDoesNotExist', 'message': 'The specified queue does not exist.'}.

Cause 2 — expected: the handler's secretsmanager client resolves the secret, and a missing one yields ResourceNotFoundException. Actual: "positive control failed: {'status_code': 200, 'payload': {'ok': False, 'error': 'UnknownServiceError', 'code': None, 'message': None}}".

Cause 3 — expected: the un-granted SendMessage is refused HTTP 403 AccessDenied with AWS's implicit-deny message. Actual: "subject (no sqs:SendMessage) send is denied not met within 45.0s. Last result: {'status_code': 200, 'payload': {'ok': True}}".

Cause 4 — expected: both scoped-denial scenarios produce the refusal and its message. Actual: "Error: Check timed out after 60.0s" for each.

## Steps to Reproduce
1. Create a secret, two SQS queues and a Lambda function whose handler calls secretsmanager:GetSecretValue and sqs:SendMessage and returns the error code it catches.
2. Cause 1: point the handler at a nonexistent queue URL; it reports code 'QueueDoesNotExist' instead of 'AWS.SimpleQueueService.NonExistentQueue'.
3. Cause 2: have the handler construct a 'secretsmanager' client; it raises UnknownServiceError instead of calling the service.
4. Cause 3: remove sqs:SendMessage from the role and invoke in a loop; the send keeps returning ok=True for 45 s.
5. Cause 4: run the secretsmanager-missing-permission and sqs-resource-scoped scenarios; both are killed at 60 s with no verdict.
6. Replay the five check ids in trace_ids.
