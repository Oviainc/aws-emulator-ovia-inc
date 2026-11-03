Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.cache-subnet-group-invalid-subnet
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.cache-subnet-group-invalid-subnet|run#11|AssertionError|assert refused == (
Issue-Commit: 2fe3407ad3a572d7c56bcafd231aa855d1c09ca1

# Implement elasticache ModifyCacheSubnetGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2fe3407ad3a572d7c56bcafd231aa855d1c09ca1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyCacheSubnetGroup

## Description
`ModifyCacheSubnetGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_cache_subnet_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ModifyCacheSubnetGroup` call, so each one unblocks only when `ModifyCacheSubnetGroup` works: `elasticache.cache-subnet-group-invalid-subnet`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/modify_cache_subnet_group.rs`. Replaying the corpus, the emulator refused the `ModifyCacheSubnetGroup` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_elasticache_cache_subnet_group_invalid_subnet.py", line 68, in run
    assert refused == (
           ^^^^^^^^^^^^
AssertionError: expected CacheSubnetGroupNotFound with its exact message, got ('NotImplemented', 'operation ModifyCacheSubnetGroup is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the elasticache emulator
Call ModifyCacheSubnetGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
