Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__sns.xsvc-iam-2-sns-identity-policy-only-authorizes-publish iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant iam__sns.xsvc-iam-2-sns-topic-policy-only-authorizes-publish iam__sns.xsvc-iam-2-sns-topic-policy-deny-beats-identity-allow-denied
Issue-Key: iam__sns.xsvc-iam-2-sns-identity-policy-only-authorizes-publish|_settle#9|AssertionError|raise AssertionError(
Issue-Key: iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant|_settle#16|AssertionError|raise AssertionError(
Issue-Key: iam__sns.xsvc-iam-2-sns-topic-policy-only-authorizes-publish|_settle#9|AssertionError|raise AssertionError(
Issue-Key: iam__sns.xsvc-iam-2-sns-topic-policy-deny-beats-identity-allow-denied|_settle#14|AssertionError|raise AssertionError(
Issue-Commit: 73a8baeac3f8a94bf357d3a10d4553d70859ebf3

# iam__sns: sns:Publish is authorized for everyone — neither an IAM identity policy nor the topic access policy (including an explicit Deny) is consulted

> _Note: this issue was identified against the code base at commit `73a8baeac3f8a94bf357d3a10d4553d70859ebf3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
This issue carries 1 root cause, grounded in 4 failing behavioral checks of the iam__sns group (all of GROUP auth_enforcement).

Related OPEN issue that names the same area but does NOT carry these replay keys and therefore does not cover them: 20261029T183635Z-iam-sns-sns-publish-is-never-authorized-against.md (same family, filed when the checks hung without a verdict; these four now reach a verdict and report the opposite outcome from AWS). 20261025T025317Z-sns-authorizes-nothing-on-the-data-plane-topic.md covers the sns-only topic-policy checks, not these.

Cause 1 of 1: SNS Publish performs no authorization at all. A freshly created IAM user with NO identity policy, publishing to a topic that carries only AWS's default access policy, is served 200 with a MessageId; and a user whose identity policy allows sns:Publish while the topic policy carries an explicit Deny for that principal is likewise served 200.
Failing check ids: iam__sns.xsvc-iam-2-sns-identity-policy-only-authorizes-publish, iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant, iam__sns.xsvc-iam-2-sns-topic-policy-only-authorizes-publish, iam__sns.xsvc-iam-2-sns-topic-policy-deny-beats-identity-allow-denied.

AWS: Smithy (com.amazonaws.sns) models Publish -> error AuthorizationErrorException, errorKind client, httpError 403, awsQueryError code "AuthorizationError", HTTP response code 403. AWS evaluates the request against the union of the caller's identity-based policies and the topic's resource policy: with neither a grant, and with an explicit Deny anywhere, the call is refused 403 AuthorizationError; a same-account identity policy allowing sns:Publish on the topic ARN is by itself sufficient when the topic carries the default policy (recorded behavior, and the SNS API-permissions reference the checks cite). The refusal message's fixed text for a caller no policy allows is "User: <principal-arn> is not authorized to perform: SNS:Publish on resource: <topic-arn> because no identity-based policy allows the SNS:Publish action"; for an explicit deny the trailing clause is "with an explicit deny in a resource-based policy" (topic policy) / "...in an identity-based policy" (identity policy). Everything outside the substitutions is fixed text; the variable parts are the caller's own ARN (from the signing credentials) and the TopicArn from the request. An authorized Publish returns a UUID MessageId (36 chars, 4 hyphens).

Emulator: every probe returns success. The checks poll for three consecutive refusals over 120 s and record 'last observations [OK, OK, OK, OK, OK, OK, OK, OK, OK, OK, OK, OK]' — i.e. HTTP 200 with a MessageId on every attempt — in all four states: (a) policy-less user, default topic policy; (b) user with neither an identity nor a topic-policy grant; (c) policy-less user against the default topic policy before any topic-policy grant; (d) identity Allow overridden by a topic-policy explicit Deny. No AuthorizationError is ever produced, so the grant combinations are indistinguishable from each other.

Why: a client relying on AWS's behavior gets no access control on the SNS data plane. Any credential in the account (or any principal at all, since the resource policy is also ignored) can publish to any topic; an explicit Deny written to revoke access has no effect. Negative tests and least-privilege reviews that assert a principal is refused pass vacuously, and a deployment that is supposed to close a topic silently leaves it open.

Fix: the Publish handler's authorization step in the SNS service (it must evaluate the caller's IAM identity policies and the topic's stored access-policy document, with explicit Deny winning, and refuse with 403 AuthorizationErrorException/awsQueryError AuthorizationError carrying the message text above), plus whatever shared request-authorization layer supplies the caller's effective policies to SNS.

Traces: iam__sns.xsvc-iam-2-sns-identity-policy-only-authorizes-publish; iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant; iam__sns.xsvc-iam-2-sns-topic-policy-only-authorizes-publish; iam__sns.xsvc-iam-2-sns-topic-policy-deny-beats-identity-allow-denied.

## Evidence
Expected (AWS): Publish signed by an IAM user with no identity policy, to a topic whose GetTopicAttributes Policy has Id "__default_policy_ID", is refused with HTTP 403, Error.Code "AuthorizationError" and message "User: <user-arn> is not authorized to perform: SNS:Publish on resource: <topic-arn> because no identity-based policy allows the SNS:Publish action"; after put_user_policy grants sns:Publish on that exact topic ARN the same call returns 200 with a 36-character UUID MessageId. With a topic-policy explicit Deny for the principal, the call stays refused 403 AuthorizationError even though the identity policy allows it.

Actual (emulator): all four checks fail in their settle loop with, e.g.
  AssertionError: Publish by a policy-less user before any identity policy exists: wanted 'AuthorizationError' 3x in a row within 120s; last observations ['OK','OK','OK','OK','OK','OK','OK','OK','OK','OK','OK','OK']
  AssertionError: Publish by a user with neither an identity nor a topic-policy grant: wanted 'AuthorizationError' 3x in a row within 120s; last observations ['OK', ... ]
  AssertionError: Publish by a policy-less user against the default topic policy: wanted 'AuthorizationError' 3x in a row within 120s; last observations ['OK', ... ]
  AssertionError: Publish by a user whose identity Allow is overridden by a topic Deny: wanted 'AuthorizationError' 3x in a row within 120s; last observations ['OK', ... ]
'OK' is the harness's marker for a 2xx response, so every Publish was accepted with a MessageId.

Failing checks (verdict: mismatch): iam__sns.xsvc-iam-2-sns-identity-policy-only-authorizes-publish, iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant, iam__sns.xsvc-iam-2-sns-topic-policy-only-authorizes-publish, iam__sns.xsvc-iam-2-sns-topic-policy-deny-beats-identity-allow-denied.

## Steps to Reproduce
1. sns:CreateTopic Name=<topic> (no Attributes) and confirm GetTopicAttributes Attributes["Policy"] has Id "__default_policy_ID".
2. iam:CreateUser <user>; iam:CreateAccessKey for it; build an SNS client signing with those credentials. Attach NO policy.
3. Publish(TopicArn=<topic>, Message="probe") with the user's credentials. Expected: 403 Error.Code=AuthorizationError. Observed: 200 with a MessageId, repeatedly for 120 s.
4. iam:PutUserPolicy on <user> allowing {"Effect":"Allow","Action":"sns:Publish","Resource":"<topic>"} and Publish again. Expected (and observed): 200 with a UUID MessageId — so the pre-grant and post-grant states are identical.
5. sns:SetTopicAttributes(AttributeName="Policy") with a document whose statement is {"Effect":"Deny","Principal":{"AWS":"<user-arn>"},"Action":"sns:Publish","Resource":"<topic>"} while the identity Allow of step 4 stays in place; Publish again. Expected: 403 AuthorizationError (explicit Deny wins). Observed: 200 with a MessageId.
