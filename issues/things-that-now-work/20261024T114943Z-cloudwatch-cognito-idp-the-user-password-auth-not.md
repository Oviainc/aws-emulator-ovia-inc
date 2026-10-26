Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__cognito-idp.obs-cognito-idp-2-sign-in-rejected-for-disabled-flow-is-a-zero-sample
Issue-Key: cloudwatch__cognito-idp.obs-cognito-idp-2-sign-in-rejected-for-disabled-flow-is-a-zero-sample|run#14|AssertionError|assert refusal == want, (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# cloudwatch__cognito-idp: the USER_PASSWORD_AUTH-not-enabled refusal message is rewritten and echoes the app client id (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Cause 1 of 1 — InitiateAuth on an app client whose ExplicitAuthFlows do not include the requested flow is refused with the right status and the right code, but with a message AWS does not produce, and that message discloses the app client id.

AWS: Smithy (cognito-idp) InitiateAuth models InvalidParameterException (httpError 400) for a flow the client does not allow. The message is a fixed string for this condition, measured against real AWS (us-west-2) by the check itself: 'USER_PASSWORD_AUTH flow not enabled for this client' — it names the flow and deliberately not the client.

Emulator: ('InvalidParameterException', 'USER_PASSWORD_AUTH is not enabled for the client 4b5a335f5c3842bdab52352210') — reworded and with the ClientId interpolated. The 400 and the error code match.

Why: the message is the only place the reason is stated, and clients and log pipelines match the AWS wording to distinguish a client-misconfiguration refusal from a credential refusal; interpolating the client id also puts an identifier into an error returned on an unauthenticated path. Separately, because the check asserts this refusal before measuring anything, its real subject — that a flow-rejected sign-in adds nothing to AWS/Cognito SignInSuccesses Sum yet still records one ZERO-VALUED sample in that series, with the repaired client's sign-in publishing the only non-zero sample — is never exercised, so the metric-accounting behavior remains unverified.

Fix: the message construction for the ExplicitAuthFlows check in the InitiateAuth handler's validation (shared with AdminInitiateAuth).

Traces: cloudwatch__cognito-idp.obs-cognito-idp-2-sign-in-rejected-for-disabled-flow-is-a-zero-sample.

## Evidence
AssertionError: USER_PASSWORD_AUTH attempt 1/3 on a client that does not allow the flow was refused with ('InvalidParameterException', 'USER_PASSWORD_AUTH is not enabled for the client 4b5a335f5c3842bdab52352210'), expected ('InvalidParameterException', 'USER_PASSWORD_AUTH flow not enabled for this client').
The check's own header records the expectation as measured against real AWS (us-west-2): 3 flow-rejected attempts + 1 repaired sign-in settled at (Sum 1.0, SampleCount 4.0) on AWS/Cognito SignInSuccesses. Status 400 and code InvalidParameterException already match; only the message diverges. The check carried no 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateUserPool, CreateUserPoolClient with ExplicitAuthFlows that EXCLUDE ALLOW_USER_PASSWORD_AUTH, AdminCreateUser + AdminSetUserPassword(Permanent=True).
3. InitiateAuth(AuthFlow='USER_PASSWORD_AUTH', ClientId=<that client>, AuthParameters={'USERNAME','PASSWORD'}).
4. Expect ('InvalidParameterException', 'USER_PASSWORD_AUTH flow not enabled for this client'); observe the reworded message with the ClientId interpolated.
5. Re-replay cloudwatch__cognito-idp.obs-cognito-idp-2-sign-in-rejected-for-disabled-flow-is-a-zero-sample, which then proceeds to its SignInSuccesses Sum/SampleCount assertions.
