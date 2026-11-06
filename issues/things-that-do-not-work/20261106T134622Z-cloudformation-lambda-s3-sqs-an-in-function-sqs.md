Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__s3__sqs.lambda-sdk-send-sqs-missing-permission-negative cloudformation__lambda__s3__sqs.sqs-queue-policy-blocks-lambda-send-negative
Issue-Key: cloudformation__lambda__s3__sqs.lambda-sdk-send-sqs-missing-permission-negative|_poll_until#11|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__s3__sqs.sqs-queue-policy-blocks-lambda-send-negative|run#20|AssertionError|assert denied_payload is not None, (
Issue-Commit: 2b63d043efdd2fa9855b978e8c81153e1c6b6b87

# cloudformation__lambda__s3__sqs: an in-function sqs:SendMessage is authorized by neither the execution role's identity policy nor the queue's resource policy (2 causes)

> _Note: this issue was identified against the code base at commit `2b63d043efdd2fa9855b978e8c81153e1c6b6b87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
Two net-new root causes, both in the authorization path that an SQS data-plane call from a Lambda handler has to pass. Neither is covered by an open issue; the nearest open neighbours (20261105T094842Z-cloudformation-lambda-s3-sqs-2-net-new-root-7fb457.md, 20261029T193114Z-cloudformation-lambda-s3-sqs-5-net-new-causes.md) are about the runtime's missing SDK clients and about ESM delivery, not about authorization being skipped.

Cause 1 of 2: the execution role's identity policy is not evaluated for an SDK call made from inside the function.
AWS: a Lambda function calls AWS with its execution role's credentials and every such call is authorized by IAM. After the sqs:SendMessage grant is removed from the role (DeleteRolePolicy / a policy revision that no longer allows it), SendMessage on that queue is refused — the model does not enumerate an authorization error for SQS SendMessage (Smithy: SendMessage errors are InvalidMessageContents, UnsupportedOperation, QueueDoesNotExist, RequestThrottled, KmsDisabled... ), so AWS's documented IAM implicit-deny applies: HTTP 403, error code `AccessDenied`, and the message template "User: <principal> is not authorized to perform: sqs:SendMessage on resource: <queue ARN> because no identity-based policy allows the sqs:SendMessage action". Fixed text is everything outside the angle brackets; <principal> is the caller's assumed-role session ARN and <queue ARN> is the queue named in the request, both taken from the request and from IAM state — never a literal from one run.
Emulator: the send keeps succeeding after the grant is revoked. The handler returns HTTP 200 with payload {"statusCode": 200} for the whole 45 s the check polls, so the revocation has no observable effect at all.
Why: a client that uses least-privilege roles to prove a blast radius gets a false negative — code that must not be able to write to a queue writes to it, and a test suite that asserts denial passes against the emulator and fails against AWS.
Fix: the SQS data-plane authorization check for SendMessage (the operation's handler must consult the signing principal's identity policies before mutating queue state), plus the credential/principal plumbing that gives a function's in-process SDK calls the execution role as their caller identity.
Traces: cloudformation__lambda__s3__sqs.lambda-sdk-send-sqs-missing-permission-negative — "AssertionError: SDK send denied after sqs:SendMessage is revoked not met within 45.0s. Last result: {'status_code': 200, 'payload': {'statusCode': 200}}".

Cause 2 of 2: a queue resource policy whose statement Denies the caller does not block SendMessage.
AWS: the queue's Policy attribute is a resource-based policy; an explicit Deny in it overrides any identity-based Allow, so once a Deny statement naming the Lambda role as Principal for Action sqs:SendMessage is set via SetQueueAttributes, every subsequent SendMessage from that role is refused HTTP 403 `AccessDenied` with the resource-policy wording "User: <principal> is not authorized to perform: sqs:SendMessage on resource: <queue ARN> with an explicit deny in a resource-based policy". Smithy models the attribute (SetQueueAttributes / QueueAttributeName `Policy`) but not the evaluation, so the enforcement rule itself is AWS's documented policy-evaluation logic.
Emulator: the Deny is stored and then ignored. Over the 30 s window the check watches, 11 consecutive sends from the denied role are accepted, each answering {"statusCode": 200}.
Why: resource policies are the only control for cross-account and cross-service access to a queue; an emulator that stores them but never evaluates them reports every queue as world-writable, so policy regressions are invisible.
Fix: policy evaluation in the SQS queue-attribute/state layer — the `Policy` attribute must be parsed and consulted (explicit Deny first) by the SendMessage/ReceiveMessage/DeleteMessage handlers, not merely round-tripped by GetQueueAttributes.
Traces: cloudformation__lambda__s3__sqs.sqs-queue-policy-blocks-lambda-send-negative — "AssertionError: a queue-policy Deny targeting the Lambda role did not block sqs:SendMessage within 30.0s (11 send(s) got through): {'statusCode': 200}".

## Evidence
Cause 1 — expected: the SendMessage issued by the handler is refused after the role's sqs:SendMessage grant is revoked (HTTP 403, code AccessDenied, message "User: <assumed-role session ARN> is not authorized to perform: sqs:SendMessage on resource: <queue ARN> because no identity-based policy allows the sqs:SendMessage action"). Actual: HTTP 200, payload {'statusCode': 200}, repeated for the full 45.0 s poll window — "AssertionError: SDK send denied after sqs:SendMessage is revoked not met within 45.0s".

Cause 2 — expected: after SetQueueAttributes installs a resource policy with an explicit Deny on sqs:SendMessage for the Lambda role, every send from that role is refused HTTP 403 AccessDenied ("...with an explicit deny in a resource-based policy"). Actual: 11 sends in 30 s all accepted, each answering {'statusCode': 200} — "AssertionError: a queue-policy Deny targeting the Lambda role did not block sqs:SendMessage within 30.0s (11 send(s) got through)".

## Steps to Reproduce
1. Deploy the stack the checks deploy: an SQS queue, a Lambda function whose handler calls sqs.send_message on it, and an execution role.
2. Cause 1: grant the role sqs:SendMessage, invoke and observe success; then revoke the grant and invoke again in a loop. The emulator keeps answering 200 instead of refusing with AccessDenied.
3. Cause 2: with the identity grant in place, SetQueueAttributes a Policy containing {"Effect":"Deny","Principal":{"AWS":"<lambda role ARN>"},"Action":"sqs:SendMessage","Resource":"<queue ARN>"}; invoke repeatedly. Every send is accepted.
4. Replay cloudformation__lambda__s3__sqs.lambda-sdk-send-sqs-missing-permission-negative and cloudformation__lambda__s3__sqs.sqs-queue-policy-blocks-lambda-send-negative.
