Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.custom-attribute-schema-is-append-only cognito-idp.get-csv-header-column-order-matches-schema-addition-order cognito-idp.user-attribute-empty-string-vs-absent-distinguished-on-get-user cognito-idp.user-attribute-schema-and-value-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|AddCustomAttributes|NotImplemented
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement cognito-idp AddCustomAttributes: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddCustomAttributes

## Description
`AddCustomAttributes` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/add_custom_attributes.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `AddCustomAttributes` call, so each one unblocks only when `AddCustomAttributes` works: `cognito-idp.custom-attribute-schema-is-append-only`, `cognito-idp.get-csv-header-column-order-matches-schema-addition-order`, `cognito-idp.user-attribute-empty-string-vs-absent-distinguished-on-get-user`, `cognito-idp.user-attribute-schema-and-value-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/add_custom_attributes.rs`. Replaying the corpus, the emulator refused the `AddCustomAttributes` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddCustomAttributes operation: operation AddCustomAttributes is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cognito-idp emulator
Call AddCustomAttributes and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
