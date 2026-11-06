Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.lambda-sqs-sdk-missing-permission lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-rejected lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative lambda__sns__sqs.cfn-deploy-lambda-execution-role-missing-sqs-permission-negative lambda__sns__sqs.lambda-sdk-subscribe-to-sns-topic lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-rejected lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-negative
Issue-Key: lambda__sns__sqs.lambda-sqs-sdk-missing-permission|run#7|AssertionError|assert denied_payload.get("ok") is False, (
Issue-Key: lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-rejected|run#8|AssertionError|assert denied["ok"] is False, denied
Issue-Key: lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative|_invoke_json_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda__sns__sqs.cfn-deploy-lambda-execution-role-missing-sqs-permission-negative|run#11|AssertionError|assert denied_payload["ok"] is False
Issue-Key: lambda__sns__sqs.lambda-sdk-subscribe-to-sns-topic|run#10|AssertionError|assert result["ok"] is True, result
Issue-Key: lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-rejected|run#8|AssertionError|assert denied["error_code"] == "AuthorizationError", denied
Issue-Key: lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-negative|run#15|AssertionError|assert bad_result["error_code"] == "AuthorizationError", bad_result
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# lambda__sns__sqs: in-function AWS SDK calls are never authorized against the execution role, and the guest SDK resolves no `sns` client (2 net-new causes)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 2 net-new root causes. Both must be closed before submitting. Related but NOT covering: the open issue 20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md describes the older symptom (no boto3 at all, Runtime.ImportModuleError). The handler now runs and returns its own payload, so that issue's replay keys no longer match these checks and the suite reports them as unowned; what remains are the two causes below.

Cause 1 of 2: an in-function SDK call to SQS succeeds no matter what the function's execution role allows — the role's identity policy is never consulted for calls the handler makes.
AWS: the managed python runtimes inject the execution role's temporary credentials into the execution environment, so a client built inside the handler is signed as `arn:aws:sts::<account-id>:assumed-role/<role-name>/<function-name>` and every call it makes is authorized against that role's identity policies. SQS models no IAM-denial error (Smithy: SendMessage / ReceiveMessage errors are only InvalidAddress, InvalidSecurity, InvalidMessageContents, KMS.*, AWS.SimpleQueueService.NonExistentQueue, RequestThrottled, AWS.SimpleQueueService.UnsupportedOperation), so the denial is the service-common one — recorded behavior: HTTP 403, error code `AccessDenied`, message `User: <assumed-role-principal> is not authorized to perform: <action> on resource: <resource-arn> because no identity-based policy allows the <action> action`. Fixed text: `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`. Variable parts, all taken from the request and emulator state: the assumed-role principal ARN (account id, the role name of the function's Role, the function name), the lower-cased action (`sqs:sendmessage`, `sqs:receivemessage`) and the queue ARN. With the grant present the same call succeeds, which the checks' positive controls assert.
Emulator: the call succeeds for a role that grants nothing beyond AWSLambdaBasicExecutionRole. `sqs.send_message` returns normally and the handler payload is `{'ok': True}` (lambda-sqs-sdk-missing-permission: `AssertionError: Lambda succeeded sending an SQS message without an IAM permission grant: {'status_code': 200, 'payload': {'ok': True}}`); `sqs.receive_message` returns the queued message to the unauthorized role (`AssertionError: {'ok': True, 'bodies': ['payload-<id>']}`). Where the check re-invokes until the denial appears, it never does and the check burns its 60 s budget (lambda-sdk-receive-message-missing-permission-negative). The same holds for a CloudFormation-deployed function whose role omits sqs:SendMessage (cfn-deploy-lambda-execution-role-missing-sqs-permission-negative: `AssertionError` on `assert denied_payload["ok"] is False`).
Why: a client relying on AWS's behavior loses the entire permission boundary of Lambda code. Least-privilege roles are untestable: an under-permissioned function appears to work in the emulator and fails in AWS with AccessDenied, and negative tests that assert a denial pass vacuously or hang. The effect is also real, not just a wrong status: the message is actually sent/received, so the "no effect" half of the denial is violated too.
Fix: the identity the Lambda execution engine hands to the guest SDK, and the authorization gate on the receiving side — the guest SDK's dispatch into the emulator's SQS front door must carry the function's execution-role principal, and the SQS operation handlers' authorization step must evaluate that principal against the role's identity policies before performing the operation (see the lambda crate's execution/guest-SDK module and the sqs crate's SendMessage/ReceiveMessage handlers and their shared authorization helper).
Traces: lambda__sns__sqs.lambda-sqs-sdk-missing-permission, lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-rejected, lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative, lambda__sns__sqs.cfn-deploy-lambda-execution-role-missing-sqs-permission-negative.

Cause 2 of 2: the SDK available inside the python runtime resolves no `sns` service client — `boto3.client("sns").subscribe(...)` raises botocore `UnknownServiceError` instead of calling SNS.
AWS: the managed python runtimes bundle a complete boto3/botocore, so every AWS service (including `sns`) has service data and a client can be constructed for it. Smithy: Subscribe(TopicArn required, Protocol required, Endpoint, ReturnSubscriptionArn) returns SubscriptionArn; for an SQS endpoint the subscription is immediately confirmed, so a later Publish on the topic reaches the queue. When the caller's role lacks the action, Subscribe fails with AuthorizationErrorException — httpError 403, awsQuery code `AuthorizationError` — and the recorded message is `User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<function-name> is not authorized to perform: SNS:Subscribe on resource: <topic-arn> because no identity-based policy allows the SNS:Subscribe action` (fixed text as in cause 1 plus the `SNS:Subscribe` action spelling; variable parts the principal ARN and the TopicArn from the request).
Emulator: the handler catches a botocore exception that carries no `response` document at all. The handler payload is `{'ok': False, 'error_code': 'UnknownServiceError', 'message': None}` (lambda-sdk-subscribe-missing-permission-rejected) or `{'ok': False, 'error_code': None}` where the handler reads `response.Error.Code` without a fallback (lambda-sdk-subscribe-missing-permission-negative), and the same error is returned on the PERMITTED path, where AWS succeeds: `AssertionError: {'ok': False, 'error_code': 'UnknownServiceError'}` for the role that does grant sns:Subscribe (lambda-sdk-subscribe-to-sns-topic). No subscription is created in either case, so neither the AuthorizationError refusal nor the end-to-end publish-through-the-new-subscription is reachable. Hypothesis: the guest SDK shim exposes a hand-written allow-list of services (sqs works in the same runtime, cause 1) and `sns` is absent from it, so client construction/first call raises UnknownServiceError rather than reaching the emulator's SNS front door.
Why: a client relying on AWS's behavior cannot run the most common Lambda-as-glue pattern of all — wiring or fanning out through SNS from function code. Every handler that builds an SNS client fails with an error AWS never produces (UnknownServiceError is a client-side botocore error, not an AWS response), so both the happy path and the permission-boundary negative are unobservable.
Fix: the Lambda execution engine's guest SDK surface — register the `sns` service in the guest botocore service catalogue and dispatch its operations to the emulator's SNS front door under the function's execution-role identity (same component as cause 1).
Traces: lambda__sns__sqs.lambda-sdk-subscribe-to-sns-topic, lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-rejected, lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-negative.

## Evidence
Cause 1 — IAM not enforced for in-function SQS calls:
  lambda__sns__sqs.lambda-sqs-sdk-missing-permission
    Expected (AWS): handler payload {'ok': False, 'error_code': 'AccessDenied', 'message': 'User: arn:aws:sts::<acct>:assumed-role/<role>/<fn> is not authorized to perform: sqs:sendmessage on resource: <queue-arn> because no identity-based policy allows the sqs:sendmessage action'}
    Actual: AssertionError: Lambda succeeded sending an SQS message without an IAM permission grant: {'status_code': 200, 'payload': {'ok': True}}
  lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-rejected
    Expected: {'ok': False, 'error_code': 'AccessDenied'} and the message still on the queue
    Actual: AssertionError: {'ok': True, 'bodies': ['payload-95419f22a788']}
  lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative
    Expected: a denial from the unauthorized role; Actual: Check timed out after 60.0s (the re-invoke loop never sees ok=False)
  lambda__sns__sqs.cfn-deploy-lambda-execution-role-missing-sqs-permission-negative
    Expected: denied_payload['ok'] is False; Actual: AssertionError (ok=True) at run line 130

Cause 2 — no `sns` client in the guest SDK:
  lambda__sns__sqs.lambda-sdk-subscribe-to-sns-topic (role GRANTS sns:Subscribe)
    Expected (AWS): {'ok': True, 'subscription_arn': '<topic-arn>:<uuid>'}, the subscription listed and confirmed, and a later publish delivered to the queue
    Actual: AssertionError: {'ok': False, 'error_code': 'UnknownServiceError'}
  lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-rejected
    Expected: 403 AuthorizationError with 'User: ... is not authorized to perform: SNS:Subscribe on resource: <topic-arn> because no identity-based policy allows the SNS:Subscribe action'
    Actual: AssertionError: {'ok': False, 'error_code': 'UnknownServiceError', 'message': None}
  lambda__sns__sqs.lambda-sdk-subscribe-missing-permission-negative
    Expected: error_code 'AuthorizationError'; Actual: AssertionError: {'ok': False, 'error_code': None}

## Steps to Reproduce
Replay the seven checks in trace_ids.

Cause 1 minimal repro: create a queue; create an IAM role trusted by lambda.amazonaws.com with only arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole attached; CreateFunction (Runtime=python3.12, Handler=index.handler) with a package whose handler does `boto3.client('sqs').send_message(QueueUrl=<url>, MessageBody='x')` inside try/except and returns the caught error code; Invoke (RequestResponse). AWS returns the handler payload reporting AccessDenied and the queue stays empty; the emulator returns {'ok': True} and the message is on the queue. Repeat with receive_message against a queue holding one message.

Cause 2 minimal repro: same role but with an inline Allow of sns:Subscribe on a topic; handler does `boto3.client('sns').subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint=<queue-arn>, ReturnSubscriptionArn=True)`; Invoke. AWS returns a SubscriptionArn and ListSubscriptionsByTopic shows the confirmed subscription; the emulator's handler catches botocore UnknownServiceError and the topic has no subscription.
