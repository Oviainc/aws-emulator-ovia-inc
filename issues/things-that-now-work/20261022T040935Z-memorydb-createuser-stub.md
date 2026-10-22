Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: memorydb.acl-membership-reflected-on-users memorydb.acl-membership-updates-rejected memorydb.delete-user-leaves-every-acl memorydb.describe-users-pages-cover-every-user memorydb.events-record-user-and-acl-lifecycle memorydb.resource-name-rules-rejected memorydb.resource-names-fold-to-lowercase memorydb.tag-mutations-merge-and-are-atomic memorydb.user-password-count-and-auth-mode-transitions memorydb.user-password-punctuation-accepted memorydb.user-password-rules-rejected
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|CreateUser|NotImplemented
Issue-Key: memorydb.user-password-rules-rejected|run#4|AssertionError|assert code == want_code, f"{suffix}: expected {want_code}, got {code}: {msg!r}"
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# Implement memorydb CreateUser: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
`CreateUser` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/create_user.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

11 recorded unit(s) fail on their `CreateUser` call, so each one unblocks only when `CreateUser` works: `memorydb.acl-membership-reflected-on-users`, `memorydb.acl-membership-updates-rejected`, `memorydb.delete-user-leaves-every-acl`, `memorydb.describe-users-pages-cover-every-user`, `memorydb.events-record-user-and-acl-lifecycle`, `memorydb.resource-name-rules-rejected`, `memorydb.resource-names-fold-to-lowercase`, `memorydb.tag-mutations-merge-and-are-atomic`, `memorydb.user-password-count-and-auth-mode-transitions`, `memorydb.user-password-punctuation-accepted`, `memorydb.user-password-rules-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/create_user.rs`. Replaying the corpus, the emulator refused the `CreateUser` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateUser operation: operation CreateUser is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the memorydb emulator
Call CreateUser and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
