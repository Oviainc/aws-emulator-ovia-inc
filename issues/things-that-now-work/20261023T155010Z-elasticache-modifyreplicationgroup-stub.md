Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.concurrent-modify-replication-group-state-conflict elasticache.replication-group-lifecycle
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.concurrent-modify-replication-group-state-conflict|run#11|AssertionError|assert len(successes) >= 1, f"neither concurrent modify call succeeded: {results}"
Issue-Key: elasticache|ModifyReplicationGroup|NotImplemented
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# Implement elasticache ModifyReplicationGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyReplicationGroup

## Description
`ModifyReplicationGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_replication_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ModifyReplicationGroup` call, so each one unblocks only when `ModifyReplicationGroup` works: `elasticache.concurrent-modify-replication-group-state-conflict`, `elasticache.replication-group-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_replication_group.rs`. Replaying the corpus, the emulator refused the `ModifyReplicationGroup` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_elasticache_concurrent_modify_replication_group_state_conflict.py", line 64, in run
    assert len(successes) >= 1, f"neither concurrent modify call succeeded: {results}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: neither concurrent modify call succeeded: [{'ok': False, 'code': 'NotImplemented', 'message': 'operation ModifyReplicationGroup is scaffolded but not implemented in this emulator', 'limit': 1}, {'ok': False, 'code': 'NotImplemented', 'message': 'operation ModifyReplicationGroup is scaffolded but not implemented in this emulator', 'limit': 2}]

## Steps to Reproduce
Boot the elasticache emulator
Call ModifyReplicationGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
