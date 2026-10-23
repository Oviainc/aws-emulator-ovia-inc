Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-identity.obs-cognito-identity-1-set-identity-pool-roles-audit-records-carry-each-calls-role-map cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity
Issue-Key: cognito-identity|SetIdentityPoolRoles|NotImplemented
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# Implement cognito-identity SetIdentityPoolRoles: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetIdentityPoolRoles

## Description
`SetIdentityPoolRoles` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/set_identity_pool_roles.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `SetIdentityPoolRoles` call, so each one unblocks only when `SetIdentityPoolRoles` works: `cloudtrail__cognito-identity.obs-cognito-identity-1-set-identity-pool-roles-audit-records-carry-each-calls-role-map`, `cloudtrail__cognito-identity__sts.obs-cognito-identity-1-role-trusting-another-pool-is-rejected-and-audits-no-failed-assume`, `cloudtrail__cognito-identity__sts.obs-cognito-identity-1-vended-credentials-are-audited-as-assume-role-with-web-identity`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/set_identity_pool_roles.rs`. Replaying the corpus, the emulator refused the `SetIdentityPoolRoles` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SetIdentityPoolRoles operation: operation SetIdentityPoolRoles is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cognito-identity emulator
Call SetIdentityPoolRoles and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
