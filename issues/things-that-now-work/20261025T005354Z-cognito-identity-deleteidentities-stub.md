Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.concurrent-delete-identities-overlapping-batches-consistent-final-state cognito-identity.e2e-identity-population-delete-merge-unlink-final-state cognito-identity.list-identities-dedup-across-pagination-with-deleted-identities
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|DeleteIdentities|NotImplemented
Issue-Commit: d0dc720ebbf11903c1fa9407d1c82e53eafd4b75

# Implement cognito-identity DeleteIdentities: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d0dc720ebbf11903c1fa9407d1c82e53eafd4b75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIdentities

## Description
`DeleteIdentities` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/delete_identities.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DeleteIdentities` call, so each one unblocks only when `DeleteIdentities` works: `cognito-identity.concurrent-delete-identities-overlapping-batches-consistent-final-state`, `cognito-identity.e2e-identity-population-delete-merge-unlink-final-state`, `cognito-identity.list-identities-dedup-across-pagination-with-deleted-identities`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/delete_identities.rs`. Replaying the corpus, the emulator refused the `DeleteIdentities` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteIdentities operation: operation DeleteIdentities is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cognito-identity emulator
Call DeleteIdentities and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
