Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.the-assume-role-allow-decision-is-served-from-cache-after-the-trust-policy-tightens
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.the-assume-role-allow-decision-is-served-from-cache-after-the-trust-policy-tightens|run#43|Timeout|time.sleep(_ASSUME_INTERVAL_S)
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# STS AssumeRole never converges to AccessDenied after UpdateAssumeRolePolicy tightens a role's trust policy, so the allow/deny transition can never be observed (1 cause)

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
This issue carries ONE net-new root cause in the STS service (service token `sts`). A fixer must close it before submitting.

Cause 1 of 1: after iam.UpdateAssumeRolePolicy replaces a role's trust policy with one whose single Allow is gated by a Condition the caller cannot satisfy, sts:AssumeRole on that role keeps returning fresh, working credentials indefinitely — the refusal never arrives, so the ALLOW -> DENY transition the operation is supposed to make is unobservable.

AWS: a role's trust policy is re-read and re-evaluated on every AssumeRole. Smithy (com.amazonaws.sts#AssumeRole) models only ExpiredTokenException (awsQueryError ExpiredTokenException, HTTP 400), MalformedPolicyDocumentException (MalformedPolicyDocument, 400), PackedPolicyTooLargeException (PackedPolicyTooLarge, 400) and RegionDisabledException (RegionDisabledException, 403) — so a trust-policy denial is the service-common client error AccessDenied with HTTP 403, serialized as an awsQuery XML ErrorResponse. Recorded behavior (us-west-2, 2026-09-17, in the check header): iam.GetRole reflected the tightened AssumeRolePolicyDocument after 0.3s, AssumeRole continued to return credentials for a further ~301s (60 consecutive ALLOWs), and then every subsequent AssumeRole was refused with exactly AccessDenied and stayed refused; the credentials vended during that window were real and carried the role's permissions (they read the object back from S3). The REQUIREMENT this grounds is the convergence and its terminal state: AssumeRole must eventually refuse with AccessDenied / HTTP 403 once the stored trust document no longer authorizes the caller, and must not start granting again afterwards. The ~300s length of AWS's decision cache is an eventual-consistency artifact of a live run, not a requirement — an emulator that enforces the tightened document immediately satisfies convergence and completes the check's polling loop well inside the suite's cap.

Emulator: the check reports `Error: Check timed out after 60.0s` with no assertion reached. Its own convergence budget is 600s of AssumeRole calls at 5s intervals (_CACHE_DEADLINE_S), entered only AFTER a positive control (role assumable, session reads the S3 object) and AFTER iam.GetRole has already returned the tightened document, so the budget is consumed by emulator-served AssumeRole calls that are never refused. Hypothesis: the stall is that convergence loop rather than one of the earlier polls, because every earlier poll is a path the emulator serves permissively (S3 reads under a scoped session are never denied, GetCallerIdentity under minted credentials succeeds) and would therefore return on its first probe. The non-refusal itself is corroborated directly by the sibling check sts.concurrent-role-trust-policy-update-and-assume-role-race-consistent, which did reach an assertion and reported `ambient caller unexpectedly assumed the gated-only role: {'success': True, 'error_code': None}`.

Why: a client relying on AWS's behavior gets a silent authorization hole. Revoking access by tightening a role's trust policy is the standard break-glass/off-boarding action, and against the emulator it never takes effect: the principal keeps minting fully-privileged sessions forever. Any negative test, runbook or incident-response procedure validated here ("tighten the trust policy, confirm AssumeRole is refused") passes against the emulator and fails against AWS, and a least-privilege trust boundary is reported as enforced when it is not.

Fix: the AssumeRole handler's authorization step must resolve the role's CURRENT stored AssumeRolePolicyDocument on every call (never a document captured at create time) and evaluate its statements, including the Condition block, against the request context; when no statement authorizes the caller it must answer the common AccessDenied with HTTP 403 as an awsQuery XML ErrorResponse, and the refusal must be stable (no later call is granted again). Sessions already vended must keep working until their own Expiration. This cause is adjacent to — and NOT covered by — the open issue 20261024T154246Z-sts-assumerolewithwebidentity-is-unroutable-unknownservice-trust-policy-conditions.md, whose cause 2 covers the same Condition-evaluation gap but whose Issue-Trace-Ids do not include this check's replay key, so this check would otherwise never be re-replayed; the two should be closed together. It is also distinct from 20261024T065715Z-sts-assumerole-getsessiontoken-admit-requests-aws-refuses-no.md (role existence, cross-account, sts:ExternalId) and from 20261024T065715Z-sts-sessions-carry-no-policy-scoping-packedpolicysize-oversized.md (session-policy scoping).

Traces: sts.the-assume-role-allow-decision-is-served-from-cache-after-the-trust-policy-tightens.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the booted emulator. The failing check is not marked ALREADY FILED and not marked OWNED by any open issue.

[mismatch] sts.the-assume-role-allow-decision-is-served-from-cache-after-the-trust-policy-tightens
  Error: Check timed out after 60.0s

Expected (recorded, us-west-2 2026-09-17, from the check's own header): l_doc=0.3s (GetRole reflects the tightened AssumeRolePolicyDocument), l_deny=301.0s, cached_allows=60, first refusal exactly AccessDenied / HTTP 403, still AccessDenied on a final re-probe, and the credentials minted during the window reading the exact object bytes back from S3.

Actual: no refusal is ever served. The check's convergence loop (`_CACHE_DEADLINE_S = 600.0`, 5s interval, entered only after a positive control and after GetRole already returned the tightened document) runs until the suite's 60s cap with no AccessDenied observed. The assertions it would have reached are `assert cached_allows >= 1`, `assert l_deny > l_doc` and `assert still_refused == "AccessDenied"`; none is evaluated because the loop's exit condition (`outcome == "AccessDenied"`) never occurs.

Corroborating, from the same service and already filed under a different check: sts.concurrent-role-trust-policy-update-and-assume-role-race-consistent reported `AssertionError: ambient caller unexpectedly assumed the gated-only role: {'success': True, 'error_code': None}` against a role whose only Allow is gated by a Condition the caller does not satisfy — i.e. the Condition gate is absent rather than merely slow.

Smithy (services/service-com-amazonaws-sts-awssecuritytokenservicev20110615): AssumeRole errors = ExpiredTokenException(400), MalformedPolicyDocument(400), PackedPolicyTooLarge(400), RegionDisabledException(403); a trust-policy denial is therefore the service-common AccessDenied at HTTP 403, not any modeled operation error.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (run_behavioral_suite).
2. Re-replay the check id in trace_ids; it is self-contained and establishes a positive control before asserting.
3. Minimal repro:
   a. s3.create_bucket + s3.put_object(Key='cached-allow', Body=b'...').
   b. iam.create_role with AssumeRolePolicyDocument = Allow / Principal {"AWS": "arn:aws:iam::<account>:root"} / Action sts:AssumeRole, and an inline policy allowing s3:GetObject on that object's ARN.
   c. sts.assume_role on the role, then read the object with the returned credentials — confirm the positive control succeeds.
   d. iam.update_assume_role_policy on the same role, keeping Principal at the account root but adding Condition {"StringEquals": {"aws:PrincipalArn": "arn:aws:iam::<account>:user/does-not-exist"}} (a Condition VALUE is never validated for existence, so the write is accepted; putting the fabricated ARN in Principal instead is refused outright by IAM with MalformedPolicyDocument).
   e. Poll iam.get_role until Role.AssumeRolePolicyDocument contains the fabricated ARN — the control plane reflects the tightening.
   f. Call sts.assume_role on the role repeatedly from that instant.
4. Observed: every call keeps returning credentials, indefinitely, and those credentials keep reading the object. Expected: the calls converge to AccessDenied / HTTP 403 and stay refused; sessions vended before convergence remain valid until their own Expiration.
