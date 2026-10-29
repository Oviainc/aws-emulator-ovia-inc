Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-with-web-identity-oidc-provider-deleted-blocked sts.userid-condition-matches-the-role-id-and-session-name-composite sts.principal-arn-condition-resolves-to-the-role-arn-not-the-session-arn
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.assume-role-with-web-identity-oidc-provider-deleted-blocked|run#20|AssertionError|assert deleted == present, (
Issue-Key: sts.userid-condition-matches-the-role-id-and-session-name-composite|run#51|AssertionError|assert role_id_only_code == "AccessDenied", (
Issue-Key: sts.principal-arn-condition-resolves-to-the-role-arn-not-the-session-arn|_poll#48|Timeout|time.sleep(interval_s)
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sts: 3 net-new behavioral root causes — the deleted-OIDC-provider refusal is emulator prose, aws:userid is the bare RoleId without the session name, and aws:PrincipalArn never decides anything

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRoleWithWebIdentity

## Description
Three net-new root causes in the STS session/identity surface. They are numbered so a fixer can close them one at a time; all three must be closed before submitting.

Related but NOT covering any of these: open issue 20261029T120000Z-sts-3-of-the-8-session-scoping-causes-remain-open.md. Its Issue-Trace-Ids list does not carry any of the three check ids below, and its causes are the session-POLICY scoping half (inline Policy / PolicyArns / boundary / principal-tag and principal-type conditions). The two condition keys here (aws:userid, aws:PrincipalArn) are a different derivation: they are produced from the session credential's own identity, not from a policy document attached to it, so neither is closed by that report's shared-session-directory work on its own.

=== Cause 1 of 3: AssumeRoleWithWebIdentity answers a DIFFERENT InvalidIdentityToken message once the IAM OIDC provider is deleted ===
AWS: Smithy (com.amazonaws.sts#AssumeRoleWithWebIdentity): error InvalidIdentityTokenException, errorKind client, httpError 400, awsQueryError {code: InvalidIdentityToken, httpResponseCode: 400}. The model does not state the message, so recorded behavior only: AWS answers the SAME (code, message) pair for a web identity token it cannot verify whether or not an IAM OpenID Connect provider for that issuer exists in the account — error code `InvalidIdentityToken` with the message `Couldn't retrieve verification key from your identity provider, please reference AssumeRoleWithWebIdentity documentation for requirements`. That message is entirely FIXED text: it has no variable parts, and in particular it does not name the issuer URL, the role, or whether a provider is registered. Deleting the provider does not change the refusal, which is the property the check asserts (`deleted == present`).
Emulator: with the provider present it produces the AWS pair, but once the provider is deleted it answers error code `InvalidIdentityToken` with the emulator-authored message `No OpenIDConnect provider found in your account for https://example.com/oidc<suffix>` — fixed text `No OpenIDConnect provider found in your account for ` plus one variable part, the issuer URL taken from the request's token. AWS never produces that sentence.
Why: a client that matches on AWS's message (or an SDK/test asserting the refusal is stable across provider lifecycle) sees the refusal text change underneath it when an unrelated control-plane delete happens, and the emulator's text discloses account configuration — whether an OIDC provider is registered for a given issuer — that AWS deliberately does not reveal through this error.
Fix: the token-verification path of the AssumeRoleWithWebIdentity handler in the STS crate (its ops/assume_role_with_web_identity.rs). The provider-lookup MISS must fall through to the same verification-key failure the present-but-unverifiable case already produces, rather than being reported as its own distinct condition.
Traces: sts.assume-role-with-web-identity-oidc-provider-deleted-blocked

=== Cause 2 of 3: aws:userid for an AssumeRole session is the bare RoleId, so a policy scoped to ONE session admits EVERY session of the role ===
AWS: the Smithy model says nothing about condition keys, so this is documented IAM behavior plus recorded behavior. IAM User Guide, "IAM policy elements: Variables and tags" -> Principal key values: for an assumed role, aws:userid is `role-id:caller-specified-role-name`, where role-id is the role's unique id and caller-specified-role-name is the RoleSessionName passed to AssumeRole. Consequently a session policy whose ONLY Allow is gated on `StringEquals aws:userid "<RoleId>"` — the role id ALONE, with no session-name half — matches nothing and grants nothing, and the subsequent s3:GetObject is refused with error code exactly `AccessDenied`. GetCallerIdentity's UserId for the same credentials is that same `<RoleId>:<RoleSessionName>` composite, so the identity API and the authorization engine must agree.
Emulator: the call SUCCEEDS. The replay reports `aws:userid matched the bare RoleId '<RoleId>', so the session name is missing from the value the authorization engine sees: expected AccessDenied, got '<call succeeded>'`. The authorizer is comparing against the role id only; the `:<RoleSessionName>` half is absent from the value it resolves.
Why: this is over-permissive in the most direct way. The deployed idiom for pinning a grant to one named session is `StringEquals aws:userid "<RoleId>:<SessionName>"` (and `StringLike "<RoleId>:*"` for the whole role). On this emulator those two collapse to the same thing, so a policy written to admit exactly one session admits every session ever minted from that role, and no test can observe the difference.
Fix: the request-context builder that derives the authorization condition keys from a session credential — the aws:userid value for an assumed-role principal must be composed as `<RoleId>:<RoleSessionName>` from the session record STS minted, in the shared authorization context used by the data-plane gate (services/common authorization support, fed by the STS session state), not in any one service's handler.
Traces: sts.userid-condition-matches-the-role-id-and-session-name-composite

=== Cause 3 of 3: aws:PrincipalArn is never evaluated for a session credential, so the condition decides nothing and the check never completes ===
AWS: documented IAM behavior (IAM User Guide, "AWS global condition context keys" -> aws:PrincipalArn: "For IAM roles, the request context returns the ARN of the role, not the ARN of the user that assumed the role", and "Do not specify the assumed role session ARN as a value for this condition key"); the Smithy model states nothing about condition keys. So for a call made with AssumeRole session credentials, aws:PrincipalArn resolves to the IAM ROLE arn `arn:aws:iam::<account>:role/<name>` and NOT to the assumed-role session arn `arn:aws:sts::<account>:assumed-role/<name>/<session>` that GetCallerIdentity returns. A role inline policy whose only Allow carries `StringEquals aws:PrincipalArn == <the session arn>` therefore grants that session nothing, and the following s3:GetObject is refused with error code `AccessDenied` (implicit deny, HTTP 403).
Emulator: that refusal never arrives. The check establishes a positive control, then polls for the AccessDenied transition and exhausts the harness cap — `Error: Check timed out after 60.0s`. No divergent response body is captured because the emulator never produces one: the condition has no effect on the outcome of any later call.
Why: every grant written in terms of aws:PrincipalArn is inert here, in both directions — a policy intended to admit only the role arn admits anything, and a policy mistakenly written against the session arn (which AWS treats as a grant to nobody) silently appears to work, so an emulator-validated policy is wrong in production.
Fix: the same request-context derivation as cause 2 — aws:PrincipalArn must be populated with the role arn (not the session arn) for an assumed-role principal — plus the S3 data-plane gate actually consulting it. The condition-evaluation engine itself already exists in the tree (services/common/src/iam_authorization.rs); what is missing is this key's value for a session principal and the data plane asking for a decision.
Traces: sts.principal-arn-condition-resolves-to-the-role-arn-not-the-session-arn

## Evidence
Cause 1 — sts.assume-role-with-web-identity-oidc-provider-deleted-blocked, [mismatch]:
  AssertionError: expected the same rejection with the provider deleted as with it present (('InvalidIdentityToken', "Couldn't retrieve verification key from your identity provider, please reference AssumeRoleWithWebIdentity documentation for requirements")), got ('InvalidIdentityToken', 'No OpenIDConnect provider found in your account for https://example.com/oidcf8cd2ff5870a')
  Expected code/message: InvalidIdentityToken / "Couldn't retrieve verification key from your identity provider, please reference AssumeRoleWithWebIdentity documentation for requirements" (HTTP 400, awsQueryError InvalidIdentityToken). Actual code is right, message is emulator prose. The issuer URL suffix in the actual message is a per-run generated value, not part of any requirement.

Cause 2 — sts.userid-condition-matches-the-role-id-and-session-name-composite, [mismatch], at check line 321:
  assert role_id_only_code == "AccessDenied"
  AssertionError: aws:userid matched the bare RoleId 'AROABGXEQES5NRCCJJWAN', so the session name is missing from the value the authorization engine sees: expected AccessDenied, got '<call succeeded>'
  (The RoleId shown is a per-run generated id; the requirement is the composite FORM `<RoleId>:<RoleSessionName>`, not that literal value.)

Cause 3 — sts.principal-arn-condition-resolves-to-the-role-arn-not-the-session-arn, [mismatch]:
  Error: Check timed out after 60.0s — the bounded poll for the expected AccessDenied never converges, because the s3:GetObject under the session whose role policy Allows only `StringEquals aws:PrincipalArn == <session arn>` keeps succeeding.

## Steps to Reproduce
1. Boot the emulator and replay the three check ids in trace_ids.
2. Cause 1: iam.create_open_id_connect_provider(Url='https://example.com/oidc<rand>', ClientIDList=[...], ThumbprintList=[...]); call sts.assume_role_with_web_identity(RoleArn=<role>, RoleSessionName='probe', WebIdentityToken=<an unverifiable JWT for that issuer>) and record (Code, Message). Then iam.delete_open_id_connect_provider(...) and repeat the same AssumeRoleWithWebIdentity call. The two (Code, Message) pairs must be identical; the emulator's second call returns 'No OpenIDConnect provider found in your account for <url>'.
3. Cause 2: create a role the caller may assume, grant it s3:GetObject on one object, then sts.assume_role(RoleArn=<role>, RoleSessionName='sess-a', Policy=<inline session policy whose only Allow is s3:GetObject on that object with Condition {StringEquals: {'aws:userid': '<RoleId>'}}>) using the role's bare RoleId with NO ':<session name>' suffix. s3.get_object with those credentials must be refused AccessDenied; the emulator serves the object. Repeat with 'aws:userid': '<RoleId>:sess-a' to confirm the composite form is the one AWS matches.
4. Cause 3: assume the same role, then replace the role's inline policy with one whose only Allow carries Condition {StringEquals: {'aws:PrincipalArn': '<the assumed-role SESSION arn from GetCallerIdentity>'}} and poll s3.get_object with the already-vended session credentials for AccessDenied. AWS converges on AccessDenied (aws:PrincipalArn is the ROLE arn, so the condition never matches); the emulator keeps answering 200 until the 60s cap.
