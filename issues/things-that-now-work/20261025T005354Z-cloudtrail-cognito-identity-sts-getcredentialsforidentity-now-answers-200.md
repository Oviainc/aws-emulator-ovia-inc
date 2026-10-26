Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume
Issue-Key: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity|run#26|AssertionError|assert me["Arn"] == expected_session_arn, (
Issue-Key: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume|_error#23|AssertionError|raise AssertionError(
Issue-Commit: d0dc720ebbf11903c1fa9407d1c82e53eafd4b75

# cloudtrail__cognito-identity__sts: GetCredentialsForIdentity now answers 200 but vends the ambient emulator principal and accepts a role that trusts another pool, so neither STS audit leg can be observed (2 causes)

> _Note: this issue was identified against the code base at commit `d0dc720ebbf11903c1fa9407d1c82e53eafd4b75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
This issue bundles TWO distinct root causes. Both must be fixed and re-replayed before closing. Both checks previously failed with UnknownService (open issue 20261024T065715Z-cloudtrail-cognito-identity-sts-unsigned-cognito-identity-requests-are-answered-unknownservice-so.md, which carries these check ids for that earlier failure mode); routing works now — the call answers 200 — and these are the defects that remain. That issue does not describe either cause below.

=== Cause 1 of 2: the vended credentials are not an STS session, so no AssumeRoleWithWebIdentity happens to audit ===
AWS: GetCredentialsForIdentity vends the Credentials of an sts:AssumeRoleWithWebIdentity session for the pool's resolved role; sts:GetCallerIdentity under them answers arn:aws:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials, and the assume itself is auditable as an AssumeRoleWithWebIdentity event (Smithy GetCredentialsForIdentity: output Credentials; auth [] / optionalAuth true).
Emulator: the credentials resolve to arn:aws:iam::000000000000:user/emulator — the emulator's ambient IAM user — so no role session exists and there is no STS event for the audit leg to find.
Why: the audit trail of Cognito federation (which role a vended session is, and that it was obtained by web identity) does not exist, and every consumer that keys off the assumed-role ARN sees one shared principal.
Fix: the cognito-identity GetCredentialsForIdentity handler must mint the session through the emulator's STS AssumeRoleWithWebIdentity path (which is also what records the STS event).
Traces: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity.

=== Cause 2 of 2: a pool whose role trusts a DIFFERENT identity pool still vends credentials ===
AWS: the role's trust policy conditions the assume on cognito-identity.amazonaws.com:aud = <this pool>; when the attached role trusts another pool, the assume fails and GetCredentialsForIdentity is refused (recorded: a modeled client error, no Credentials in the response) — and no successful-assume event is recorded.
Emulator: HTTP 200 with keys ['Credentials', 'IdentityId', 'ResponseMetadata'] — the trust policy is never evaluated against the presenting pool.
Why: the aud condition is the mechanism that stops one identity pool from federating into another pool's role; with it unenforced, pool-to-pool isolation does not hold and the failed-assume audit path is unreachable.
Fix: evaluate the resolved role's trust policy (principal + condition keys, including cognito-identity.amazonaws.com:aud) in the assume path behind GetCredentialsForIdentity and surface its refusal.
Traces: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1:

Cause 1 — obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity: "AssertionError: the vended credentials must be a session of the pool's authenticated role: expected 'arn:aws:sts::000000000000:assumed-role/sim2real-xsvc-ctsts-auth-ff75b56dfa77/CognitoIdentityCredentials', actual 'arn:aws:iam::000000000000:user/emulator'".

Cause 2 — obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume, at ci.get_credentials_for_identity(IdentityId=bad_identity, Logins={'cognito-identity.amazonaws.com': bad_login['Token']}): "AssertionError: expected the call to be refused, but it SUCCEEDED and returned keys ['Credentials', 'IdentityId', 'ResponseMetadata']".

Model (smithy_query operation=GetCredentialsForIdentity): errors include InvalidIdentityPoolConfigurationException 400 and NotAuthorizedException 403; output carries Credentials.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay both check ids in trace_ids.
2. Cause 1: create a pool with an authenticated role trusting cognito-identity.amazonaws.com, obtain an identity + token, GetCredentialsForIdentity, then sts:GetCallerIdentity with the vended credentials -> expect arn:aws:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials; observe arn:aws:iam::000000000000:user/emulator.
3. Cause 2: create pools A and B; attach to pool B a role whose trust policy conditions cognito-identity.amazonaws.com:aud on pool A; call GetCredentialsForIdentity for a pool-B identity -> expect a refusal with no Credentials; observe 200 with Credentials.
