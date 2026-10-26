Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.overly-permissive-send-without-permission-denied sqs.queue-policy-source-arn-condition-denies-a-direct-caller sqs.queue-policy-statement-for-another-queue-grants-nothing sqs.queue-policy-principal-tag-condition-gates-send sqs.recreated-queue-does-not-inherit-the-deleted-queues-policy sqs.e2e-permission-boundary-then-revoke sqs.queue-policy-explicit-deny-blocks-send sqs.overly-permissive-cross-account-receive-denied sqs.queue-policy-send-grant-does-not-authorize-receive-or-delete sqs.overly-permissive-delete-queue-without-permission-denied sqs.queue-policy-action-names-outside-the-sqs-action-list-rejected sqs.queue-policy-naming-nonexistent-principal-rejected sqs.queue-policy-attribute-normalizes-principal-and-clears sqs.deploy-cfn-queue-policy-grants-role-and-update-revokes sqs.deploy-cfn-kms-queue-inline-policy-enforces-key-and-queue-grants
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.overly-permissive-send-without-permission-denied|_client_error#15|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: sqs.queue-policy-source-arn-condition-denies-a-direct-caller|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.queue-policy-statement-for-another-queue-grants-nothing|_error#18|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.queue-policy-principal-tag-condition-gates-send|_error#14|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.recreated-queue-does-not-inherit-the-deleted-queues-policy|_error#21|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.e2e-permission-boundary-then-revoke|_client_error#19|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sqs.queue-policy-explicit-deny-blocks-send|run#22|Timeout|time.sleep(5.0)
Issue-Key: sqs.overly-permissive-cross-account-receive-denied|_client_error#11|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: sqs.queue-policy-send-grant-does-not-authorize-receive-or-delete|_error#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.overly-permissive-delete-queue-without-permission-denied|_expect_denied#8|AssertionError|raise AssertionError(
Issue-Key: sqs.queue-policy-action-names-outside-the-sqs-action-list-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.queue-policy-naming-nonexistent-principal-rejected|_error#14|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sqs.queue-policy-attribute-normalizes-principal-and-clears|run#17|AssertionError|assert principal == root, (
Issue-Key: sqs.deploy-cfn-queue-policy-grants-role-and-update-revokes|run#14|KeyError|stored = json.loads(sqs.get_queue_attributes(QueueUrl=urls[k], AttributeNames=["Policy"])["Attributes"]["Policy"])
Issue-Key: sqs|CreateStack|ValidationError
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# SQS queue resource policies are stored but never enforced: every send, receive, delete and DeleteQueue is authorized regardless of the Policy attribute (7 causes)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
The Policy queue attribute round-trips, but no SQS operation is ever evaluated against it: calls that AWS denies are answered 200 with the effect applied. Smithy: the SQS model binds InvalidSecurity (403, awsQueryError InvalidSecurity) and SetQueueAttributes carries the Policy attribute in QueueAttributeMap; AWS's documented behaviour is that a request from a principal the queue policy does not allow — or that an explicit Deny matches — is refused with AccessDenied / AccessDeniedException at 403 and the operation has no effect. Recorded behaviour of every check below: the call was expected to be refused and instead returned HTTPStatusCode 200.

Cause 1 of 7: SendMessage is not authorized against the queue policy. AWS: a queue whose Policy grants sqs:SendMessage to nobody, grants it only under a Condition the caller does not satisfy (aws:SourceArn, aws:PrincipalTag), names a DIFFERENT queue in Resource, or carries an explicit Deny, refuses SendMessage with AccessDenied at 403 and enqueues nothing. Emulator: SendMessage returns 200 with a MessageId in every one of those shapes. Why: an emulated deployment cannot test its least-privilege posture at all — a policy regression is invisible, and a test asserting a denial passes vacuously. Fix: an authorization step in the SendMessage handler that evaluates the queue's Policy attribute before enqueueing. Traces: sqs.overly-permissive-send-without-permission-denied, sqs.queue-policy-source-arn-condition-denies-a-direct-caller, sqs.queue-policy-statement-for-another-queue-grants-nothing, sqs.queue-policy-principal-tag-condition-gates-send, sqs.recreated-queue-does-not-inherit-the-deleted-queues-policy, sqs.e2e-permission-boundary-then-revoke, sqs.queue-policy-explicit-deny-blocks-send (that last one never completed inside the 60 s harness cap).

Cause 2 of 7: ReceiveMessage and DeleteMessage are not authorized against the queue policy. AWS: a cross-account caller with no sqs:ReceiveMessage grant is refused AccessDenied 403, and a policy granting only sqs:SendMessage does not authorize ReceiveMessage or DeleteMessage. Emulator: ReceiveMessage returns 200 and hands back the message body in both cases. Why: a consumer that should be locked out reads the payload. Fix: the same authorization step in the ReceiveMessage and DeleteMessage handlers. Traces: sqs.overly-permissive-cross-account-receive-denied, sqs.queue-policy-send-grant-does-not-authorize-receive-or-delete.

Cause 3 of 7: DeleteQueue is not authorized against the queue policy. AWS: a principal without sqs:DeleteQueue is refused AccessDenied 403 and the queue survives. Emulator: DeleteQueue returns 200 and the queue is gone. Why: destructive. Fix: the authorization step in the DeleteQueue handler. Traces: sqs.overly-permissive-delete-queue-without-permission-denied.

Cause 4 of 7: the policy document itself is not validated on write. AWS: SetQueueAttributes with a Policy whose Action names an operation outside the SQS action set, or whose Principal names a principal that does not exist, is refused with InvalidParameterValue/InvalidAttributeValue at 400 and the attribute is unchanged. Emulator: both calls return 200 and the bad policy is stored. Why: a typo in an Action or Principal is silently accepted and then (per cause 1) never evaluated, so nothing ever surfaces it. Fix: Policy-document validation in the SetQueueAttributes handler. Traces: sqs.queue-policy-action-names-outside-the-sqs-action-list-rejected, sqs.queue-policy-naming-nonexistent-principal-rejected.

Cause 5 of 7: policy principals are not normalized on read-back. AWS: a Policy written with a bare account id as Principal reads back from GetQueueAttributes as the account root ARN arn:aws:iam::<account>:root. Emulator: it reads back as the bare 12-character account id. Why: a client that diffs its intended policy against the stored one sees a permanent spurious difference. Fix: the Policy normalization performed when the attribute is stored/serialized. Traces: sqs.queue-policy-attribute-normalizes-principal-and-clears.

Cause 6 of 7: a CloudFormation-deployed AWS::SQS::QueuePolicy never lands on the queue. AWS: after the stack reaches CREATE_COMPLETE, GetQueueAttributes(AttributeNames=['Policy']) returns the rendered document, and a stack update that removes the grant revokes it. Emulator: the Attributes map has no 'Policy' key at all (the check raises KeyError: 'Policy'). Why: any stack that grants queue access declaratively deploys without the grant. Fix: the AWS::SQS::QueuePolicy resource provider in the CloudFormation crate, which must call SetQueueAttributes on the referenced queue. Traces: sqs.deploy-cfn-queue-policy-grants-role-and-update-revokes.

Cause 7 of 7: AWS::SQS::QueueInlinePolicy is not a recognized resource type. AWS: CreateStack with that type succeeds and the inline policy is applied to the queue. Emulator: CreateStack is refused with code ValidationError, 400, 'Template format error: Unrecognized resource types: [AWS::SQS::QueueInlinePolicy]', so the whole scenario aborts at setup. Why: the stack cannot be deployed at all. Fix: register the type in the CloudFormation crate's resource-type registry (related, but not covered by, the open issue 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md, which does not name this type). Traces: sqs.deploy-cfn-kms-queue-inline-policy-enforces-key-and-queue-grants.

## Evidence
Expected (AWS): a request the queue policy does not allow is refused AccessDenied at HTTP 403 with no effect; an invalid policy document is refused at write time; a CFN AWS::SQS::QueuePolicy is readable as the Policy attribute after deploy.
Actual (emulator replay): 'AssertionError: expected the call to be denied, but it SUCCEEDED and returned {MD5OfMessageBody: ..., MessageId: ..., HTTPStatusCode: 200}' (SendMessage with no grant); 'expected the call to be refused, but it succeeded' (SourceArn condition, PrincipalTag condition, foreign-Resource statement, recreated queue inheriting nothing); 'expected the call to be denied, but it SUCCEEDED and returned {Messages: [{... Body: cross-account-guarded-body}]}' (cross-account receive); 'expected this DeleteQueue to be denied, but it SUCCEEDED'; 'expected the call to be refused, but it succeeded: {HTTPStatusCode: 200}' (bad Action name, nonexistent Principal); 'a bare account-id principal must read back as the account root ARN; got a bare account id of length 12'; 'KeyError: Policy' after a CFN QueuePolicy deploy; 'ClientError (ValidationError) when calling CreateStack: Template format error: Unrecognized resource types: [AWS::SQS::QueueInlinePolicy]'.

## Steps to Reproduce
1. CreateQueue, then SetQueueAttributes with a Policy whose only statement is an explicit Deny of sqs:SendMessage for '*'.
2. SendMessage to that queue -> 200 with a MessageId; AWS refuses AccessDenied 403.
3. SetQueueAttributes with a Policy granting only sqs:SendMessage, then ReceiveMessage -> 200 with the body; AWS refuses AccessDenied 403.
4. SetQueueAttributes with a Policy whose Action is 'sqs:NotARealAction' -> 200; AWS refuses at 400.
5. GetQueueAttributes(['Policy']) after writing a Policy with a bare account id Principal -> the bare id, not arn:aws:iam::<account>:root.
6. CreateStack with an AWS::SQS::Queue plus an AWS::SQS::QueuePolicy, wait for CREATE_COMPLETE, GetQueueAttributes(['Policy']) -> no 'Policy' key.
7. CreateStack with an AWS::SQS::QueueInlinePolicy resource -> ValidationError 'Unrecognized resource types'.
8. Re-replay the check ids listed in trace_ids.
