Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: identitystore.create-group-describe-fields identitystore.create-group-describe-reflects-fields identitystore.create-group-lifecycle identitystore.delete-already-deleted-group-is-idempotent identitystore.delete-group-cascades-membership-removal identitystore.delete-group-then-describe-not-found identitystore.recreate-group-after-delete-with-same-displayname
Issue-Shape-Id: com.amazonaws.identitystore#AWSIdentityStore
Issue-Key: identitystore|DeleteGroup|NotImplemented
Issue-Commit: 4cbced68d8f61275ceb5b427ff8478e9aa7bafe5

# Implement identitystore DeleteGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4cbced68d8f61275ceb5b427ff8478e9aa7bafe5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteGroup

## Description
`DeleteGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/delete_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `DeleteGroup` call, so each one unblocks only when `DeleteGroup` works: `identitystore.create-group-describe-fields`, `identitystore.create-group-describe-reflects-fields`, `identitystore.create-group-lifecycle`, `identitystore.delete-already-deleted-group-is-idempotent`, `identitystore.delete-group-cascades-membership-removal`, `identitystore.delete-group-then-describe-not-found`, `identitystore.recreate-group-after-delete-with-same-displayname`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/delete_group.rs`. Replaying the corpus, the emulator refused the `DeleteGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteGroup operation: operation DeleteGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the identitystore emulator
Call DeleteGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
