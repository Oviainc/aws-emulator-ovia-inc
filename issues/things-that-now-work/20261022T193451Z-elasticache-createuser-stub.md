Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.concurrent-create-user-same-id elasticache.concurrent-modify-user-group-membership-race elasticache.modify-user-conflicting-auth-params-rejected elasticache.modify-user-invalid-access-string-rejected elasticache.remove-default-user-from-group-rejected elasticache.replication-group-user-group-ids-reject-bare-user-id elasticache.user-access-string-append-and-password-mode-switch elasticache.user-access-string-malformed-rejected elasticache.user-group-default-required elasticache.user-group-modify-membership elasticache.user-group-not-found-semantics elasticache.user-id-uniqueness-and-not-found elasticache.user-lifecycle
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.concurrent-create-user-same-id|run#8|AssertionError|assert len(successes) == 1, f"expected exactly 1 success, got {len(successes)}: {results}"
Issue-Key: elasticache|CreateUser|NotImplemented
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Implement elasticache CreateUser: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUser

## Description
`CreateUser` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_user.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `CreateUser` call, so each one unblocks only when `CreateUser` works: `elasticache.concurrent-create-user-same-id`, `elasticache.concurrent-modify-user-group-membership-race`, `elasticache.modify-user-conflicting-auth-params-rejected`, `elasticache.modify-user-invalid-access-string-rejected`, `elasticache.remove-default-user-from-group-rejected`, `elasticache.replication-group-user-group-ids-reject-bare-user-id`, `elasticache.user-access-string-append-and-password-mode-switch`, `elasticache.user-access-string-malformed-rejected`, `elasticache.user-group-default-required`, `elasticache.user-group-modify-membership`, `elasticache.user-group-not-found-semantics`, `elasticache.user-id-uniqueness-and-not-found`, `elasticache.user-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_user.rs`. Replaying the corpus, the emulator refused the `CreateUser` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_elasticache_concurrent_create_user_same_id.py", line 80, in run
    assert len(successes) == 1, f"expected exactly 1 success, got {len(successes)}: {results}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected exactly 1 success, got 0: [(False, ('NotImplemented', 'operation CreateUser is scaffolded but not implemented in this emulator')), (False, ('NotImplemented', 'operation CreateUser is scaffolded but not implemented in this emulator')), (False, ('NotImplemented', 'operation CreateUser is scaffolded but not implemented in this emulator')), (False, ('NotImplemented', 'operation CreateUser is scaffolded but not implemented in this emulator')), (False, ('NotImplemented', 'operation CreateUser is scaffolded but not implemented in this emulator'))]

## Steps to Reproduce
Boot the elasticache emulator
Call CreateUser and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
