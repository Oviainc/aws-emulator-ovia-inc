Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3.sep-policy-composition-s3-identity-or-bucket-policy-each-suffice-denied-with-neither
Issue-Key: iam__s3.sep-policy-composition-s3-identity-or-bucket-policy-each-suffice-denied-with-neither|_settle#27|Timeout|time.sleep(interval_s)
Issue-Commit: df75a05586985baf68c72ff4d4d2cf208170da4a

# S3 GetObject performs no IAM authorization: a principal granted by neither an identity policy nor the bucket policy is never refused AccessDenied, so the identity-OR-resource composition can never settle

> _Note: this issue was identified against the code base at commit `df75a05586985baf68c72ff4d4d2cf208170da4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One net-new root cause in the s3 emulator's authorization path. Nearest open neighbours, neither of which covers it: 20261025T000319Z-iam-s3-2-net-new-root-causes-a.md (a deleted IAM user principal in a bucket policy, and a presigned GET signed with a deleted user's key) and 20261024T065715Z-iam-s3-sts-s3-requests-under-a-scoped-session-are-never.md (requests under a SCOPED STS SESSION, a different principal class and a different check-id service token). This cause is about plain same-account IAM USER principals and the identity-policy-OR-bucket-policy union on GetObject, and the check was not marked as owned by either issue.

Cause 1 of 1: S3 never refuses a GetObject that no policy authorizes, so no poll for a denial ever settles.
AWS: Smithy s3#GetObject (Bucket and Key both required httpLabels) models only InvalidObjectState (403) and NoSuchKey (404); the authorization refusal is S3's standard AccessDenied, so the refusal text is recorded behavior only. For a same-account principal EITHER the identity policy OR the bucket policy suffices, and with NEITHER the request is refused with code `AccessDenied` and the message "User: <principal arn> is not authorized to perform: s3:GetObject on resource: \"<object arn>\" because no identity-based policy allows the s3:GetObject action" — fixed text "User: " / " is not authorized to perform: s3:GetObject on resource: " / " because no identity-based policy allows the s3:GetObject action", with the resource ARN double-quoted; the variable parts are the caller's principal ARN and the object ARN (arn:aws:s3:::<bucket>/<key>), both taken from the caller's identity and the request. Adding a bucket policy that names only a second principal must flip that principal to a successful read of the exact object bytes while leaving the other two principals' outcomes unchanged, and S3's enforcement must agree with SimulatePrincipalPolicy given the same bucket policy as ResourcePolicy.
Emulator: the check times out after 60s with no assertion failure, inside a poll loop (_settle) that re-calls one operation until it returns the awaited outcome on consecutive tries, giving up only at its own 150s/240s budget. The awaited outcomes, in order, are (a) the identity-policy-only user's GetObject returning "OK", (b) the user with NO identity policy and no bucket policy being refused with exactly the code "AccessDenied", and (c) after a bucket policy naming that second user is added, its GetObject returning "OK". The SimulatePrincipalPolicy legs that precede the first poll asserted successfully (identity-only user allowed via "IAM Policy"; the un-granted user implicitDeny), so the divergence is in ENFORCEMENT, not simulation. Hypothesis: S3 authorizes every request unconditionally, so (b) can never be observed and the loop spins to the harness cap. The check deliberately settles on the EXACT code "AccessDenied" so that a not-yet-live access key (InvalidAccessKeyId) cannot pass as a denial, which rules out credential propagation as the explanation.
Why: this is an authorization bypass on the S3 data plane. Any principal can read any object regardless of identity policy or bucket policy, so the emulator is strictly more permissive than the account it stands in for — code validated here will be refused in production, and every client branch that handles AccessDenied from S3 (fall back, re-request a grant, surface a permissions error) is silently dead. Because the refusal is also how a bucket policy's effect is observable, no bucket-policy or least-privilege change can be validated against this emulator, and a client that polls for its own denial to confirm a grant was withdrawn never terminates.
Fix: the authorization gate in front of the s3 object data plane. The GetObject handler (and its siblings) must evaluate the caller's IAM identity policies union the bucket policy before serving bytes, and answer HTTP 403 AccessDenied with AWS's message template — filled from the caller's principal ARN and the requested object ARN — when neither allows the action, returning no object body.
Traces: iam__s3.sep-policy-composition-s3-identity-or-bucket-policy-each-suffice-denied-with-neither.

## Evidence
Suite simparity-behavioral-ga-prod-v1; iam and s3 are both bootstrapped. [mismatch]:

  Error: Check timed out after 60.0s

No assertion failure and no error from the emulator — the check is stuck in its own poll loop (_settle: poll until the probe returns the wanted value on `streak` consecutive calls, interval 2s, budget 150s, raised to 240s for one stage). On a correct target each stage settles in seconds, so 60s without progress means an awaited outcome was never observed.

Already passed before the first poll (so NOT the divergence): SimulatePrincipalPolicy for s3:GetObject on the object ARN reported the identity-policy-only user ("allowed", MatchedStatements SourcePolicyType == ["IAM Policy"]) and the user with no grant "implicitDeny".

The outcomes polled for, and so the candidates for what the emulator never produces:
  1. State 1, no bucket policy — GetObject by the identity-policy-only user -> "OK" and the exact object bytes.
  2. State 1 — GetObject by a user with NO identity policy -> refused with exactly code "AccessDenied" and message "User: <that user's arn> is not authorized to perform: s3:GetObject on resource: \"arn:aws:s3:::<bucket>/<key>\" because no identity-based policy allows the s3:GetObject action". Fixed text: "User: " / " is not authorized to perform: s3:GetObject on resource: " (resource double-quoted) / " because no identity-based policy allows the s3:GetObject action". Variable parts: the caller's principal ARN and the object ARN, from the caller's identity and the request.
  3. State 2, after PutBucketPolicy names ONLY that second user (Effect Allow, Principal {AWS: <its arn>}, Action s3:GetObject, Resource arn:aws:s3:::<bucket>/*) — its GetObject -> "OK" with the exact bytes, a third user granted by neither still refused with exactly the message in 2, and the identity-only user unaffected; S3's three enforced outcomes must equal the simulator's three verdicts.

The check settles on the EXACT error code "AccessDenied", precisely so an InvalidAccessKeyId from a not-yet-live key cannot be mistaken for a denial — so a slow-to-propagate access key cannot explain the stall. Noted limitation: the check's internal poll budgets (150s and 240s) exceed the harness's 60s per-check cap, so the report shows a timeout rather than the stalled stage's assertion text; the stage is identified above from the check source, and none of these stages can be slow on a local emulator that answers correctly.

## Steps to Reproduce
Boot the emulator and sign requests as a principal permitted to call IAM and S3.
1. CreateBucket and PutObject a small known body at key K. Do NOT put any bucket policy yet.
2. CreateUser IDENT with an inline identity policy allowing s3:GetObject on arn:aws:s3:::<bucket>/*; CreateAccessKey for it.
3. CreateUser RES with NO identity policy, and CreateUser NONE with no identity policy; CreateAccessKey for each.
4. Signing as IDENT, GetObject(Bucket, Key=K). Expect the exact bytes.
5. Signing as RES, GetObject(Bucket, Key=K). Expect HTTP 403 AccessDenied with message "User: <RES arn> is not authorized to perform: s3:GetObject on resource: \"arn:aws:s3:::<bucket>/<K>\" because no identity-based policy allows the s3:GetObject action". Observe instead that the emulator does not refuse.
6. PutBucketPolicy allowing s3:GetObject on arn:aws:s3:::<bucket>/* to Principal {AWS: <RES arn>} only.
7. Signing as RES, GetObject -> expect the exact bytes. Signing as NONE, GetObject -> expect the same AccessDenied message as step 5 with NONE's ARN. Signing as IDENT, GetObject -> expect the exact bytes still.
8. Compare these three enforced outcomes with SimulatePrincipalPolicy for s3:GetObject on the object ARN with the bucket policy passed as ResourcePolicy; they must agree. Observe that enforcement allows the principal the simulator calls implicitDeny.
Replay: iam__s3.sep-policy-composition-s3-identity-or-bucket-policy-each-suffice-denied-with-neither
