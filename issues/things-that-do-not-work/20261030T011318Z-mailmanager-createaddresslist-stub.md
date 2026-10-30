Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mailmanager.address-list-deletion-orphans-its-import-job mailmanager.address-normalization-lowercases-domain-preserves-local-part mailmanager.address-prefix-filter-scopes-membership-listing mailmanager.deregister-member-is-idempotent-for-unknown-and-removed-addresses mailmanager.member-lookup-requires-exact-post-normalization-casing mailmanager.members-of-address-list-pagination-is-complete-and-unique
Issue-Shape-Id: com.amazonaws.mailmanager#MailManagerSvc
Issue-Key: mailmanager|CreateAddressList|NotImplemented
Issue-Commit: 6499d5c21f60bffb920d67d38c35c702f5411e4b

# Implement mailmanager CreateAddressList: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6499d5c21f60bffb920d67d38c35c702f5411e4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAddressList

## Description
`CreateAddressList` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-mailmanager-mailmanagersvc/src/ops/create_address_list.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateAddressList` call, so each one unblocks only when `CreateAddressList` works: `mailmanager.address-list-deletion-orphans-its-import-job`, `mailmanager.address-normalization-lowercases-domain-preserves-local-part`, `mailmanager.address-prefix-filter-scopes-membership-listing`, `mailmanager.deregister-member-is-idempotent-for-unknown-and-removed-addresses`, `mailmanager.member-lookup-requires-exact-post-normalization-casing`, `mailmanager.members-of-address-list-pagination-is-complete-and-unique`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-mailmanager-mailmanagersvc/src/ops/create_address_list.rs`. Replaying the corpus, the emulator refused the `CreateAddressList` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAddressList operation: operation CreateAddressList is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the mailmanager emulator
Call CreateAddressList and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
