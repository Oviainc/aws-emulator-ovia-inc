Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.lambda-sdk-subscribe-to-sns-topic lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-negative lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-rejected lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-rejected lambda__sns__sqs.lambda-sqs-sdk-missing-permission lambda__sns__sqs.cfn-deploy-lambda-execution-role-missing-sqs-permission-negative lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits
Issue-Key: lambda__sns__sqs.lambda-sdk-subscribe-to-sns-topic|run#10|AssertionError|assert result["ok"] is True, result
Issue-Key: lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-negative|run#15|AssertionError|assert bad_result["error_code"] == "AuthorizationError", bad_result
Issue-Key: lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-rejected|run#8|AssertionError|assert denied["error_code"] == "AuthorizationError", denied
Issue-Key: lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-rejected|run#8|AssertionError|assert denied["ok"] is False, denied
Issue-Key: lambda__sns__sqs.lambda-sqs-sdk-missing-permission|run#7|AssertionError|assert denied_payload.get("ok") is False, (
Issue-Key: lambda__sns__sqs.cfn-deploy-lambda-execution-role-missing-sqs-permission-negative|run#11|AssertionError|assert denied_payload["ok"] is False
Issue-Key: lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative|_invoke_json_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits|_drain_json#16|Timeout|resp = sqs.receive_message(
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# lambda__sns__sqs: the in-Lambda SDK has no SNS client, SQS calls are served without an IAM grant, and the ESM maximum-concurrency scenario never settles (3 causes)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
Three net-new root causes. The open issue 20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md carries three OTHER checks of this suite (list-subscriptions-by-topic, sns-sdk-publish, sns-sdk-missing-permission) and 20261029T183635Z-lambda-sns-sqs-sqs-event-source-mappings-never.md carries the ESM-delivery family; neither carries the replay keys below, and cause 2 is not described by either.

Cause 1 of 3: the in-process Lambda python runtime's AWS SDK has no SNS client, so a handler can neither subscribe nor be refused for subscribing.
AWS: a handler can construct an 'sns' client; Subscribe with sns:Subscribe granted answers 200 with a SubscriptionArn, and without the grant it is refused with the modeled AuthorizationErrorException — Smithy: com.amazonaws.sns#AuthorizationErrorException, errorKind client, httpError 403, awsQueryError {code: "AuthorizationError", httpResponseCode: 403} — whose message is AWS's authorization sentence "User: <principal> is not authorized to perform: SNS:Subscribe on resource: <topic ARN> because no identity-based policy allows the SNS:Subscribe action".
Emulator: the call never leaves the handler. Three checks record the same shape of failure: {'ok': False, 'error_code': 'UnknownServiceError'} for the granted subscribe, {'ok': False, 'error_code': 'UnknownServiceError', 'message': None} where `AuthorizationError` was expected, and {'ok': False, 'error_code': None} where the UnknownServiceError carries no response at all. Elsewhere in the same run this runtime states its registry: "Unknown service: '<name>'. Valid service names are: sqs, events, eventbridge, dynamodb".
Why: SNS is unreachable from function code, so both the success path and the AuthorizationError path are untestable; the client sees an SDK construction error (error_code 'UnknownServiceError' or None) that is not an AWS error code and cannot be branched on.
Fix: the in-process Lambda python runtime's bundled AWS SDK service-client registry (add the SNS client so the call is signed and routed), after which SNS Subscribe's handler and the authorization layer answer it.
Traces: lambda__sns__sqs.lambda-sdk-subscribe-to-sns-topic, .lambda-sdk-subscribe-missing-permission-negative, .lambda-sdk-subscribe-missing-permission-rejected.

Cause 2 of 3: SQS calls from inside a function are not authorized against the execution role.
AWS: a handler's SQS call is signed with the execution role and authorized by IAM. Without the grant, ReceiveMessage/SendMessage are refused HTTP 403 `AccessDenied` with "User: <principal> is not authorized to perform: <sqs action> on resource: <queue ARN> because no identity-based policy allows the <sqs action> action" (fixed text outside the brackets; principal, action and queue ARN come from the request and IAM state). SQS's Smithy model does not enumerate the authorization error, so this is AWS's documented IAM behavior. In the CFN arm, a deployed function whose execution role lacks the SQS grant must report the failure, so the probe's payload has ok == False.
Emulator: the calls succeed. ReceiveMessage without a grant returns the messages — {'ok': True, 'bodies': ['payload-...']}; SendMessage without a grant returns {'status_code': 200, 'payload': {'ok': True}} ("Lambda succeeded sending an SQS message without an IAM permission grant"); the CFN-deployed under-permissioned probe asserts denied_payload["ok"] is False and gets True. A fourth check of the same family never even reaches its assertion ("Error: Check timed out after 60.0s").
Why: the missing grant has no observable effect, so least-privilege roles cannot be validated and a negative test passes locally while the deployment fails in AWS; data a role must not read is handed back.
Fix: the authorization check in the SQS ReceiveMessage/SendMessage handlers, plus the plumbing that gives an in-process handler's SDK calls the execution role as caller identity.
Traces: lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-rejected, .lambda-sqs-sdk-missing-permission, .cfn-deploy-lambda-execution-role-missing-sqs-permission-negative, .lambda-sdk-receive-message-missing-permission-negative.

Cause 3 of 3: the ESM maximum-concurrency scenario never reaches a verdict.
AWS: an SQS event source mapping with ScalingConfig.MaximumConcurrency caps the number of concurrent batch processors, so with a deep queue the observed concurrent invocations never exceed that value. Smithy: CreateEventSourceMapping/UpdateEventSourceMapping carry ScalingConfig{MaximumConcurrency} and return EventSourceMappingConfiguration.
Emulator: "Error: Check timed out after 60.0s" — the check is killed before asserting, so the cap is unobservable.
Why: MaximumConcurrency is how a consumer is kept from overwhelming a downstream dependency; if the scenario never settles the setting cannot be verified at all.
Fix: the SQS event-source-mapping runtime's scaling/concurrency control.
Traces: lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits.

## Evidence
Cause 1 — expected: a granted Subscribe returns a SubscriptionArn; an un-granted one is refused 403 with code `AuthorizationError` (Smithy awsQueryError on com.amazonaws.sns#AuthorizationErrorException). Actual: {'ok': False, 'error_code': 'UnknownServiceError'}; {'ok': False, 'error_code': 'UnknownServiceError', 'message': None}; {'ok': False, 'error_code': None} — the runtime's SDK knows only "sqs, events, eventbridge, dynamodb".

Cause 2 — expected refusals (HTTP 403, AccessDenied, AWS's implicit-deny message). Actual: "{'ok': True, 'bodies': ['payload-...']}"; "Lambda succeeded sending an SQS message without an IAM permission grant: {'status_code': 200, 'payload': {'ok': True}}"; "assert denied_payload['ok'] is False → AssertionError"; and one check killed at 60 s.

Cause 3 — expected: concurrent invocations never exceed ScalingConfig.MaximumConcurrency. Actual: "Error: Check timed out after 60.0s" — no verdict.

## Steps to Reproduce
1. Create an SNS topic, an SQS queue and a Lambda function; grant/withhold sns:Subscribe and the SQS actions on the execution role as each arm requires.
2. Cause 1: have the handler construct an 'sns' client and call subscribe. It raises UnknownServiceError instead of subscribing or being refused AuthorizationError.
3. Cause 2: with no SQS grant, invoke handlers that call receive_message and send_message (directly and via the CFN-deployed probe). All succeed with ok=True.
4. Cause 3: create an SQS mapping with ScalingConfig.MaximumConcurrency, fill the queue and watch concurrency. The check is killed at 60 s.
5. Replay the eight check ids in trace_ids.
