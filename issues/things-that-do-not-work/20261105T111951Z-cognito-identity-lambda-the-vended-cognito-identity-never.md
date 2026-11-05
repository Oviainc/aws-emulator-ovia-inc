Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__lambda.xsvc-cognito-identity-3-lambda-context-carries-the-vended-identity-and-none-for-an-iam-caller cognito-identity__lambda.xsvc-cognito-identity-3-lambda-context-identity-per-session-both-flows-absent-for-assumed-role
Issue-Key: cognito-identity__lambda.xsvc-cognito-identity-3-lambda-context-carries-the-vended-identity-and-none-for-an-iam-caller|run#30|AssertionError|assert out["cognito_identity_id"] == identity_id, (
Issue-Key: cognito-identity__lambda.xsvc-cognito-identity-3-lambda-context-identity-per-session-both-flows-absent-for-assumed-role|run#44|AssertionError|assert seen["cognito_identity_id"] == want, (
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# cognito-identity__lambda: the vended Cognito identity never reaches the Lambda context

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 — a function invoked with Cognito-vended credentials does not see the identity in its context, and the context's identity is not absent for an IAM/assumed-role caller as AWS makes it.
AWS: when Invoke is called with credentials obtained from cognito-identity:GetCredentialsForIdentity, the runtime populates context.identity with cognito_identity_id and cognito_identity_pool_id; for an IAM user or an assumed-role caller, context.identity carries no Cognito identity at all (the fields are absent/None), and each identity session is reflected independently.
Emulator: recorded behavior only — both checks fail in their body where they compare the observed context.identity with those two expectations.
Why: a handler that authorises on the caller's Cognito identity cannot distinguish an authenticated end user from an IAM caller, so per-user authorisation inside the function breaks.
Fix: the Invoke handler's propagation of the caller's Cognito identity into the invocation context, in the lambda runtime's context construction.
Traces: both check ids in trace_ids.

## Evidence
Error in the check body (run.py line 348 -> the check's own assertion) for cognito-identity__lambda.xsvc-cognito-identity-3-lambda-context-carries-the-vended-identity-and-none-for-an-iam-caller and .xsvc-cognito-identity-3-lambda-context-identity-per-session-both-flows-absent-for-assumed-role. Expected: context.identity.cognito_identity_id / cognito_identity_pool_id are set for a Cognito-credentialed invoke and absent for an IAM/assumed-role invoke.

## Steps to Reproduce
GetId + GetCredentialsForIdentity on an identity pool, Invoke a function with those credentials and have the handler report context.identity; repeat with IAM user credentials and with an assumed role.
