Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: partnercentral-account.connection-preferences-defaults partnercentral-account.missing-permission-denied-get-connection-preferences partnercentral-account.resource-not-found-error-semantics partnercentral-account.start-verification-invalid-country-code-rejected partnercentral-account.tag-connection-preferences-round-trip partnercentral-account.update-connection-preferences-concurrent-writers partnercentral-account.update-connection-preferences-excluded-participants-persist partnercentral-account.update-connection-preferences-invalid-access-type-rejected partnercentral-account.update-connection-preferences-round-trip partnercentral-account.update-connection-preferences-stale-revision-rejected
Issue-Shape-Id: com.amazonaws.partnercentralaccount#PartnerCentralAccount
Issue-Key: partnercentral-account|GetConnectionPreferences|NotImplemented
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# Implement partnercentral-account GetConnectionPreferences: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnectionPreferences

## Description
`GetConnectionPreferences` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-partnercentralaccount-partnercentralaccount/src/ops/get_connection_preferences.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `GetConnectionPreferences` call, so each one unblocks only when `GetConnectionPreferences` works: `partnercentral-account.connection-preferences-defaults`, `partnercentral-account.missing-permission-denied-get-connection-preferences`, `partnercentral-account.resource-not-found-error-semantics`, `partnercentral-account.start-verification-invalid-country-code-rejected`, `partnercentral-account.tag-connection-preferences-round-trip`, `partnercentral-account.update-connection-preferences-concurrent-writers`, `partnercentral-account.update-connection-preferences-excluded-participants-persist`, `partnercentral-account.update-connection-preferences-invalid-access-type-rejected`, `partnercentral-account.update-connection-preferences-round-trip`, `partnercentral-account.update-connection-preferences-stale-revision-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-partnercentralaccount-partnercentralaccount/src/ops/get_connection_preferences.rs`. Replaying the corpus, the emulator refused the `GetConnectionPreferences` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetConnectionPreferences operation: operation GetConnectionPreferences is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the partnercentral-account emulator
Call GetConnectionPreferences and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
