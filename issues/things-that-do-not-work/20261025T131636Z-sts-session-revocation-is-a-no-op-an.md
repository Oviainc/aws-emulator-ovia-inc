Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.in-flight-session-denied-after-a-revoke-older-sessions-policy-is-attached-to-the-role
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.in-flight-session-denied-after-a-revoke-older-sessions-policy-is-attached-to-the-role|_poll#45|Timeout|time.sleep(interval_s)
Issue-Commit: 5ad3e3e0e6a65c8bd49bec2379ef6eb0552f033a

# sts: session revocation is a no-op — an explicit Deny conditioned on aws:TokenIssueTime (the AWSRevokeOlderSessions lever) never refuses an already-issued session's downstream call

> _Note: this issue was identified against the code base at commit `5ad3e3e0e6a65c8bd49bec2379ef6eb0552f033a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
ONE net-new root cause in the STS service (service token `sts`).

Cause 1 of 1: an identity-policy statement that Denies all actions under a `DateLessThan` condition on `aws:TokenIssueTime` — the shape AWS's own AWSRevokeOlderSessions remediation attaches to a role — has no effect on sessions already minted from that role. The emulator continues to serve those sessions' downstream calls, so there is no observable revocation transition at all.

AWS: `AssumeRole` mints a session whose issue instant is fixed at mint time and is derivable from the modeled response (Smithy: `AssumeRoleResponse.Credentials` -> `Credentials.Expiration`, with `AssumeRoleRequest.DurationSeconds` range 900..43200, so issue_instant = Expiration - DurationSeconds). The `aws:TokenIssueTime` global condition key is not a Smithy member — recorded behavior only — and it is evaluated SERVER-SIDE at every later request made with that session. Attaching to the role an inline identity policy of the form {"Effect":"Deny","Action":["*"],"Resource":["*"],"Condition":{"DateLessThan":{"aws:TokenIssueTime":"<cut-off>"}}} therefore makes every session issued BEFORE the cut-off start failing, while a session assumed AFTER the cut-off is unaffected and keeps working. The refusal arrives at the called service, not at STS: for `s3:GetObject` it is HTTP 403 with error code `AccessDenied` and the explicit-deny message form `User: <session arn> is not authorized to perform: s3:GetObject on resource: "<object arn>" with an explicit deny in an identity-based policy`. In that message the fixed text is `User: `, ` is not authorized to perform: `, ` on resource: `, and the trailing ` with an explicit deny in an identity-based policy` (which is the discriminator distinguishing an explicit Deny from the default-deny form `because no identity-based policy allows the ... action`); the variable parts are taken from the request and from state — the assumed-role session ARN (arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>), the action, and the double-quoted object ARN arn:aws:s3:::<bucket>/<key>. Removing the Deny restores the original session, whose issue instant never changed.

Emulator: the transition never happens. Replay establishes the positive control first — the session reads the object successfully before anything is revoked — then puts the revoke-shaped inline policy on the role and polls for the `AccessDenied` refusal; the refusal never arrives and the check runs to the 60s harness cap (`Error: Check timed out after 60.0s`). Hypothesis: `aws:TokenIssueTime` is not supplied as request-condition context and the role's inline identity policies are not consulted on data-plane calls made with session credentials, so the Deny statement is never matched; the suite captures no diverging response body for this check because the only divergence is the ABSENCE of the 403/AccessDenied answer.

Why: this is the single lever an operator has when temporary credentials leak. A developer who validates their incident-response runbook, or an automated revocation control, against the emulator sees the policy apply cleanly and concludes leaked sessions are cut off; in AWS the same code path is the one that must actually produce `AccessDenied`, and here it produces nothing — the compromised session keeps full access until its natural expiry. The AccessDenied branch of the caller's own error handling is also never exercised.

Fix: the session-scoped authorization evaluation shared by the data-plane handlers — it must consult the role's inline/attached identity policies for the session's role, populate `aws:TokenIssueTime` into the condition context from the session's recorded issue instant, evaluate `DateLessThan` against it, and let an explicit Deny win; plus the called service's error serializer must emit the explicit-deny message variant rather than the default-deny one. Related but NOT covering this check (its replay key is absent from both): open issue 20261024T065715Z-sts-sessions-carry-no-policy-scoping-packedpolicysize-oversized.md (session policy/boundary/tag scoping is never evaluated) and open issue 20261024T065715Z-iam-s3-sts-s3-requests-under-a-scoped-session-are-never.md — neither mentions `aws:TokenIssueTime` or role-identity-policy revocation of in-flight sessions.

Traces: sts.in-flight-session-denied-after-a-revoke-older-sessions-policy-is-attached-to-the-role.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1, check `sts.in-flight-session-denied-after-a-revoke-older-sessions-policy-is-attached-to-the-role` (verdict: mismatch).

Reported failure: `Error: Check timed out after 60.0s`.

Expected (AWS): after `iam.put_role_policy` attaches the Deny-with-DateLessThan-on-aws:TokenIssueTime document to the role, the pre-existing session's `s3:GetObject` is answered HTTP 403 / `AccessDenied` with message
  User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: s3:GetObject on resource: "arn:aws:s3:::<bucket>/<key>" with an explicit deny in an identity-based policy
(fixed text as quoted; account, role name, session name, bucket and key are request/state values — the recorded run's concrete ids are an instance of the template, not part of it). A session assumed after the cut-off reads the same object successfully, and deleting the inline policy makes the original session work again.

Actual (emulator): the pre-revocation read succeeds (positive control passes, so the credential is live and the grant propagated), and the subsequent bounded poll `_poll_denied(lambda: old_s3.get_object(...), "AccessDenied", timeout_s=180)` never observes any refusal — `get_object` keeps returning 200 with the object body — so the check exhausts the harness's 60s budget with no refusal ever produced. No 403/AccessDenied response is emitted at any point.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay check id `sts.in-flight-session-denied-after-a-revoke-older-sessions-policy-is-attached-to-the-role`.
3. Minimal manual repro:
   a. Create a bucket and put an object in it.
   b. `iam.create_role` with a trust policy allowing arn:aws:iam::<account>:root to sts:AssumeRole, then `iam.put_role_policy` granting Allow s3:GetObject on arn:aws:s3:::<bucket>/*.
   c. `sts.assume_role(RoleArn=..., RoleSessionName=..., DurationSeconds=900)`; compute the session's issue instant as Credentials.Expiration - 900s; confirm `s3.get_object` succeeds with those credentials.
   d. `iam.put_role_policy` on the same role with {"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":["*"],"Resource":["*"],"Condition":{"DateLessThan":{"aws:TokenIssueTime":"<issue instant + 10s, as %Y-%m-%dT%H:%M:%SZ>"}}}]}.
   e. Repeat `s3.get_object` with the SAME (pre-cut-off) credentials: the emulator keeps answering 200, where AWS answers 403 AccessDenied with the explicit-deny message.
   f. Control that must stay working: assume the role again (issue instant now past the cut-off) and read the same object — it must succeed. Then delete the revoke policy and confirm the original session reads normally again.
