Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.queue-policy-source-arn-condition-denies-a-direct-caller sqs.deploy-cfn-queue-policy-grants-role-and-update-revokes
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.queue-policy-source-arn-condition-denies-a-direct-caller|_until_allowed#23|Timeout|time.sleep(interval_s)
Issue-Key: sqs.deploy-cfn-queue-policy-grants-role-and-update-revokes|_until_allowed#16|Timeout|time.sleep(interval_s)
Issue-Commit: 415bc37b6ebe55c6c7e638f2212d4379947a3e7c

# sqs: a queue resource policy granting a non-owner principal is still not honored — the grantee stays AccessDenied, so both queue-policy grant checks hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `415bc37b6ebe55c6c7e638f2212d4379947a3e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
REOPENS 20261025T065401Z-sqs-queue-resource-policies-are-stored-but-never.md: its fix left the call refused.

That resolved issue reported the opposite polarity — queue policies were stored and never consulted, so every call was authorized regardless of the Policy attribute. Authorization is now being performed, but it never credits the queue's own resource policy: a principal whose ONLY grant is an Allow in the queue Policy is refused for the whole propagation window instead of being allowed. The two checks below are marked neither ALREADY FILED nor OWNED in the replay report.

Cause 1 of 1: an SQS queue policy Allow for a non-owner, same-account principal is not sufficient on its own — the grantee's call is refused as if only identity-based policy counted, and it stays refused.

AWS: an SQS queue policy (the `Policy` queue attribute, set through SetQueueAttributes or deployed as AWS::SQS::QueuePolicy) is a resource-based policy and is sufficient on its own to authorize a same-account principal; no identity-based policy is required. A statement `{"Effect":"Allow","Principal":{"AWS":"<role arn>","Action":"sqs:GetQueueAttributes","Resource":"<queue arn>"}` lets that role's assumed-role session call GetQueueAttributes, and the equivalent sqs:SendMessage statement lets it send. Condition blocks on such a statement are evaluated, not ignored: an `ArnEquals` on `aws:SourceArn` does not match a direct API caller (no source ARN in the request context), so that send is refused with Error.Code `AccessDenied` and the message `User: {caller arn} is not authorized to perform: sqs:SendMessage on resource: {queue arn} because no identity-based policy allows the sqs:SendMessage action` — fixed text, with the assumed-role session ARN and the queue ARN taken from the request and from state, and the canonical API action name (`sqs:SendMessage`) as written in the policy, not lower-cased; rewriting the same key as `ArnEqualsIfExists` makes the same session's send succeed. Removing the policy (here by a stack UPDATE that drops the AWS::SQS::QueuePolicy) clears the `Policy` attribute on the queues and the grantee becomes AccessDenied again, while the queues and already-sent messages survive. Smithy: SendMessage and GetQueueAttributes model no dedicated authorization error, so the refusal is carried as AccessDenied; GetQueueAttributes/SetQueueAttributes model `InvalidAttributeValue` (client, 400) for an unacceptable Policy document.

Emulator: neither check gets past its FIRST authorization barrier, and both are cut by the harness with `Error: Check timed out after 60.0s`. In sqs.queue-policy-source-arn-condition-denies-a-direct-caller the first blocking call is a bounded loop (`_until_allowed`, timeout 120 s, interval 3 s) that retries ONLY while the call raises AccessDenied and returns on the first success — its probe is the grantee role's `GetQueueAttributes(AttributeNames=['QueueArn'])` under an unconditioned Allow for that role's ARN. The loop never exits inside the harness budget, so the emulator answered AccessDenied continuously for that whole window for a call the queue policy explicitly allows; the differential the check exists to prove (the conditioned statement denies a direct send, the IfExists rewrite admits it) is therefore never reached. sqs.deploy-cfn-queue-policy-grants-role-and-update-revokes has the same shape over a CloudFormation-deployed AWS::SQS::QueuePolicy on two queues and is cut the same way before its revoke arm. Open issue 20261027T034110Z-sqs-10-net-new-behavioral-root-causes-batchrequesttoolong.md states this same defect as its Cause 5 (there the refusal was directly observable: `AccessDenied` with `... is not authorized to perform: sqs:sendmessage on resource: ...`, action name lower-cased, through a CFN-deployed QueueInlinePolicy) but carries neither of the two replay keys below, so it does not cover them; closing it must re-replay these two as well.

Why: a queue policy is the normal — and for cross-account the only — way to let another role, another account or an AWS service use a queue. If the resource policy is not credited, every correctly configured deployment of that pattern is denied, so it cannot be exercised at all: producers cannot send, consumers cannot receive, and the inverse property (that revoking the policy denies the grantee again) cannot be verified either. Because Condition blocks are never reached, a policy that AWS would treat as not matching (aws:SourceArn absent) is indistinguishable from one it would admit, so an over-permissive or under-permissive document cannot be detected against the emulator.

Fix: the SQS request-authorization component — it must evaluate the queue's `Policy` attribute ALONGSIDE identity-based policy (a matching resource-policy Allow for a same-account principal authorizes the request on its own), evaluate each statement's Condition block against the request context (including the absent-key semantics that distinguish `ArnEquals` from `ArnEqualsIfExists`), and render the AccessDenied message with the canonical API action name rather than a lower-cased one. The CloudFormation AWS::SQS::QueuePolicy resource provider must write and clear that same `Policy` attribute on create/update/delete.

Traces: sqs.queue-policy-source-arn-condition-denies-a-direct-caller, sqs.deploy-cfn-queue-policy-grants-role-and-update-revokes.

## Evidence
From the behavioral replay report (suite simparity-behavioral-ga-prod-v1), both verdict [mismatch]:

  [mismatch] sqs.queue-policy-source-arn-condition-denies-a-direct-caller
  Error: Check timed out after 60.0s

  [mismatch] sqs.deploy-cfn-queue-policy-grants-role-and-update-revokes
  Error: Check timed out after 60.0s

Why a timeout localises the defect here: the check's first blocking construct is

  def _until_allowed(fn, *, timeout_s=120.0, interval_s=3.0):
      """Call ``fn`` until it stops raising AccessDenied (policy propagation), bounded."""
      ...  except ClientError as exc:
               if exc.response["Error"]["Code"] != "AccessDenied": raise

It returns on the first success and RE-RAISES any error other than AccessDenied. The harness cut it at 60 s inside that loop, so for ~60 s every attempt came back AccessDenied — i.e. the barrier call (the grantee role's GetQueueAttributes under an unconditioned `Allow {Principal: {AWS: <role arn>}, Action: sqs:GetQueueAttributes, Resource: <queue arn>}` stored via SetQueueAttributes) was refused throughout.

Expected (AWS), per the check: the barrier read succeeds and returns the queue's QueueArn; then `send_message` under the `ArnEquals aws:SourceArn` condition is refused with Error.Code 'AccessDenied' and Message 'User: arn:aws:sts::{account}:assumed-role/{role}/{session} is not authorized to perform: sqs:SendMessage on resource: {queue arn} because no identity-based policy allows the sqs:SendMessage action'; nothing is enqueued; and after the policy is rewritten with `ArnEqualsIfExists` the same session's send succeeds and the owner receives exactly that body.

Corroboration of the same root cause with a directly observable refusal (different check, already filed as Cause 5 of 20261027T034110Z-sqs-10-net-new-behavioral-root-causes-batchrequesttoolong.md): `An error occurred (AccessDenied) when calling the SendMessage operation: User: arn:aws:sts::{account}:assumed-role/{role}/{session} is not authorized to perform: sqs:sendmessage on resource: arn:aws:sqs:{region}:{account}:{queue} because no identity-based policy allows the sqs:sendmessage action` — the queue policy is not consulted and the action name is lower-cased.

No per-run value is part of the requirement: the account id, role name, session name, region, queue name and queue URL above are request/state values that fill the message template.

## Steps to Reproduce
1. Boot the emulator and replay the two check ids in trace_ids (both need more than the 60 s harness cap to run to completion: each carries its own 120 s/180 s propagation budgets and a 300 s revoke poll).
2. Minimal repro of the barrier that fails: sts GetCallerIdentity for the account; iam CreateRole with a trust policy allowing `arn:aws:iam::{account}:root` to sts:AssumeRole; sqs CreateQueue and read its QueueArn.
3. sqs SetQueueAttributes(QueueUrl, Attributes={'Policy': '{"Version":"2012-10-17","Statement":[{"Sid":"read","Effect":"Allow","Principal":{"AWS":"<role arn>"},"Action":"sqs:GetQueueAttributes","Resource":"<queue arn>"},{"Sid":"send","Effect":"Allow","Principal":{"AWS":"<role arn>"},"Action":"sqs:SendMessage","Resource":"<queue arn>","Condition":{"ArnEquals":{"aws:SourceArn":"<some sns topic arn>"}}}]}'}).
4. sts AssumeRole that role, and with those session credentials call sqs GetQueueAttributes(QueueUrl, AttributeNames=['QueueArn']). AWS returns the QueueArn (the resource policy alone authorizes it). The emulator answers AccessDenied, repeatedly, for at least 60 s.
5. With the same session call sqs SendMessage. AWS refuses with AccessDenied and the message 'User: {caller} is not authorized to perform: sqs:SendMessage on resource: {queue arn} because no identity-based policy allows the sqs:SendMessage action' (canonical action casing) and enqueues nothing; then SetQueueAttributes rewriting the send statement's condition key to 'ArnEqualsIfExists' and re-sending must succeed and the owner must receive exactly that body.
6. Deploy arm: CreateStack with two AWS::SQS::Queue resources and one AWS::SQS::QueuePolicy naming both queue ARNs with an ArnEquals condition on aws:PrincipalArn for a role ARN parameter; after CREATE_COMPLETE the role must be able to send to both queues. Then UpdateStack with the policy resource removed: GetQueueAttributes(AttributeNames=['All']) must no longer carry 'Policy' on either queue, the role's send must become AccessDenied, and the queues plus the already-sent messages must survive.
