Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume
Issue-Key: cognito-identity|GetCredentialsForIdentity|UnknownService
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Unsigned cognito-identity requests are answered UnknownService, so GetCredentialsForIdentity never vends credentials and the CloudTrail audit legs are unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
ONE root cause (cause 1 of 1).

AWS: GetCredentialsForIdentity (and the other Cognito Identity basic/enhanced-flow entry points) is called WITHOUT SigV4 credentials — an unauthenticated mobile/web client calls it with a login token only. AWS resolves the target service from the endpoint and answers either 200 with Credentials, or a modeled error such as InvalidIdentityPoolConfigurationException / NotAuthorizedException when the pool's role is wrong for that identity. The STS-side effect is auditable as an AssumeRoleWithWebIdentity event.
Emulator: the unsigned call never reaches Cognito Identity — the request is refused with error 'UnknownService' (the router cannot resolve a service without a SigV4 credential scope, X-Amz-Target or Host hint), so the checks retry for 120s and report "GetCredentialsForIdentity did not vend credentials within 120s ... last error 'UnknownService'". No credentials are ever vended, so neither the audit leg nor the wrong-pool refusal leg can be observed.
Why: the entire unauthenticated Cognito credential-vending flow — the primary way mobile clients obtain AWS credentials — is unusable, and the error a client receives names no AWS service or operation.
Fix: the request router's service-resolution step for unsigned/anonymous requests (resolve from the endpoint/host or the operation's target when no credential scope is present).
Traces: cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity, cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume. Open issue 20261023T185902Z-cognito-identity-unsigned-anonymous-requests-answer-unknownservice-and.md records the same defect under service token cognito-identity; it does not carry these two checks' replay keys, so they are filed here and that issue is named for cross-reference. The same router behavior also blocks the unsigned STS entry point reported in the sts issue filed alongside this one.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1:

- cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity: "AssertionError: GetCredentialsForIdentity did not vend credentials within 120s of the role being assigned; last error 'UnknownService'".
- cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume: "AssertionError: control pool: GetCredentialsForIdentity did not vend within 120s; last error 'UnknownService'".

Expected: 200 with Credentials for the correctly configured pool (the control leg), and the modeled refusal for the pool whose role trusts another pool.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay both check ids in trace_ids.
3. Manual repro: create an identity pool, GetId/GetOpenIdToken, then call cognito-identity GetCredentialsForIdentity with an unsigned client (botocore Config(signature_version=UNSIGNED)) — the emulator answers UnknownService instead of vending credentials.
