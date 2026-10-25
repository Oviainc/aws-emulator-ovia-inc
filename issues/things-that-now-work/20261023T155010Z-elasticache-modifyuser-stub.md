Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.user-access-string-append-and-password-mode-switch elasticache.user-access-string-malformed-rejected elasticache.user-lifecycle
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|ModifyUser|NotImplemented
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# Implement elasticache ModifyUser: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyUser

## Description
`ModifyUser` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_user.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ModifyUser` call, so each one unblocks only when `ModifyUser` works: `elasticache.user-access-string-append-and-password-mode-switch`, `elasticache.user-access-string-malformed-rejected`, `elasticache.user-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_user.rs`. Replaying the corpus, the emulator refused the `ModifyUser` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ModifyUser operation: operation ModifyUser is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the elasticache emulator
Call ModifyUser and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
