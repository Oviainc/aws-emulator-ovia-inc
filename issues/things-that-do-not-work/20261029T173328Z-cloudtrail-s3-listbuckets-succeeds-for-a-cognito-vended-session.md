Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation
Issue-Key: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation|_client_error#24|AssertionError|raise AssertionError(f"{what} succeeded, expected a ClientError")
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# S3 ListBuckets SUCCEEDS for a Cognito-vended session whose identity-pool role has no identity-based policy — authorization is never evaluated for web-identity sessions

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListBuckets

## Description
Cause 1 of 1: a temporary session vended by a Cognito identity pool, whose assumed role carries NO permission policy granting s3:ListAllMyBuckets, is allowed to call S3 ListBuckets. The emulator answers HTTP 200 with a bucket list instead of refusing the call.

AWS: recorded behavior only for the message template; the rule is IAM's documented implicit deny. A sigv4-signed request from an assumed-role session whose role has no identity-based policy allowing the action is refused with HTTP 403 and error code AccessDenied, message "User: {session_arn} is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action". The FIXED text of that message is "User: ", " is not authorized to perform: ", " because no identity-based policy allows the " and " action"; the VARIABLE parts are taken from the request and the emulator's state — {session_arn} is the caller's assumed-role session ARN (arn:aws:sts::<account>:assumed-role/<role-name>/CognitoIdentityCredentials, built from the pool role the session was vended for) and the action string is the IAM action the called operation maps to (ListBuckets -> s3:ListAllMyBuckets). No value from any single run is part of the requirement.

Emulator: the call is ACCEPTED and returns successfully, so the check's assertion that a ClientError is raised fails outright: "ListBuckets from a vended session whose role has no permission policy succeeded, expected a ClientError". Because the call is never refused, the downstream contract the check exists for — that CloudTrail audits the DENIED call with errorCode AccessDenied under the session's own federated identity — can never be reached either.

Why: a client relying on AWS behavior uses the unprivileged-pool-role path as its guest/least-privilege boundary. An emulator that grants it means a test suite proving "a guest session cannot enumerate my buckets" passes against the emulator and fails in production the other way round: the emulator hides a privilege escalation rather than reproducing the refusal. It is over-permissive, which is the worst direction for an authorization surface.

Fix: authorization evaluation on the S3 request path for sessions vended by web-identity/Cognito (the request-signing principal resolution plus the IAM policy evaluation the operation's handler consults before serving ListBuckets), and the AccessDenied serializer that must render the message template above.

Related but NOT covering this: open issue 20261023T135552Z-cloudtrail-s3-s3-getobject-succeeds-for-a-role-whose-policy.md is about object-level GetObject authorization for an explicitly-denying role, and 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md is about IAM-user/role sessions; neither carries this check's replay key and neither covers the bucket-level ListBuckets action for a Cognito-vended session with an empty policy.

Traces: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation

## Evidence
Replay report (suite simparity-behavioral-aws-v1):

[mismatch] cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-denied-list-buckets-audited-with-its-federation
  File "check_...vended_session_denied_list_buckets_audited_with_its_federation.py", line 188, in run
    err = _client_error(
  File "...", line 90, in _client_error
    raise AssertionError(f"{what} succeeded, expected a ClientError")
  AssertionError: ListBuckets from a vended session whose role has no permission policy succeeded, expected a ClientError

Expected (AWS): HTTP 403, Error.Code "AccessDenied", Error.Message "User: <assumed-role session ARN> is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action" (the check carries this exact template as _DENIED_MESSAGE, formatted with the session ARN it just vended).
Actual (emulator): HTTP 200 with a successful ListBuckets response — no refusal at all. Consequently the check's second half (the denied call must be audited with errorCode AccessDenied under sessionContext.webIdFederationData) is unreachable.

All setup succeeded: CreateIdentityPool, CreateRole with a trust policy for cognito-identity.amazonaws.com and an inline policy, SetIdentityPoolRoles, GetId and GetCredentialsForIdentity all returned normally; only the authorization decision diverges.

## Steps to Reproduce
1. cognito-identity: CreateIdentityPool (allow unauthenticated identities), create an IAM role trusted by cognito-identity.amazonaws.com, and SetIdentityPoolRoles to that role.
2. Remove/omit the role's permission policy so the role has NO identity-based policy allowing s3:ListAllMyBuckets (the check deletes its inline policy named sim2real-xsvc-grant for this leg).
3. GetId, then GetCredentialsForIdentity, to vend temporary credentials for the pool role.
4. Build an s3 client signed with exactly those temporary credentials (access key id, secret, session token passed explicitly) and call ListBuckets.
5. Expected: HTTP 403 AccessDenied with the message "User: <assumed-role session ARN> is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action". Observed: the call succeeds with HTTP 200.
