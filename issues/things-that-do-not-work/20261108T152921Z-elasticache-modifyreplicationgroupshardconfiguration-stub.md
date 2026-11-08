Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.concurrent-shard-config-modification-conflict elasticache.shard-reconfiguration-state-machine
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.concurrent-shard-config-modification-conflict|run#13|AssertionError|assert len(successes) == 1, f"expected exactly one winning reshard call, got {len(successes)}: {results}"
Issue-Key: elasticache|ModifyReplicationGroupShardConfiguration|NotImplemented
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# Implement elasticache ModifyReplicationGroupShardConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyReplicationGroupShardConfiguration

## Description
`ModifyReplicationGroupShardConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_replication_group_shard_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ModifyReplicationGroupShardConfiguration` call, so each one unblocks only when `ModifyReplicationGroupShardConfiguration` works: `elasticache.concurrent-shard-config-modification-conflict`, `elasticache.shard-reconfiguration-state-machine`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_replication_group_shard_configuration.rs`. Replaying the corpus, the emulator refused the `ModifyReplicationGroupShardConfiguration` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_elasticache_concurrent_shard_config_modification_conflict.py", line 68, in run
    assert len(successes) == 1, f"expected exactly one winning reshard call, got {len(successes)}: {results}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected exactly one winning reshard call, got 0: [('error', 3, 'NotImplemented', 'operation ModifyReplicationGroupShardConfiguration is scaffolded but not implemented in this emulator'), ('error', 4, 'NotImplemented', 'operation ModifyReplicationGroupShardConfiguration is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the elasticache emulator
Call ModifyReplicationGroupShardConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
