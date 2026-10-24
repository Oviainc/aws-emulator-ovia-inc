Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.create-user-pool-client-client-credentials-requires-secret cognito-idp.id-token-and-m2m-token-are-refused-where-a-user-access-token-is-required cognito-idp.op-contract-deleteResourceServer cognito-idp.op-contract-list-resource-servers cognito-idp.op-contract-update-resource-server cognito-idp.resource-server-lifecycle cognito-idp.resource-server-scope-update-does-not-cascade-to-client
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|CreateResourceServer|NotImplemented
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement cognito-idp CreateResourceServer: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateResourceServer

## Description
`CreateResourceServer` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/create_resource_server.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `CreateResourceServer` call, so each one unblocks only when `CreateResourceServer` works: `cognito-idp.create-user-pool-client-client-credentials-requires-secret`, `cognito-idp.id-token-and-m2m-token-are-refused-where-a-user-access-token-is-required`, `cognito-idp.op-contract-deleteResourceServer`, `cognito-idp.op-contract-list-resource-servers`, `cognito-idp.op-contract-update-resource-server`, `cognito-idp.resource-server-lifecycle`, `cognito-idp.resource-server-scope-update-does-not-cascade-to-client`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/create_resource_server.rs`. Replaying the corpus, the emulator refused the `CreateResourceServer` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateResourceServer operation: operation CreateResourceServer is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cognito-idp emulator
Call CreateResourceServer and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
