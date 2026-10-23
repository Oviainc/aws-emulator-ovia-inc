Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.automatic-failover-promotes-replica elasticache.concurrent-modify-replication-group-state-conflict elasticache.concurrent-replica-count-increase-decrease-contention elasticache.concurrent-shard-config-modification-conflict elasticache.create-global-replication-group-unknown-primary elasticache.create-replication-group-invalid-shard-config-rejected elasticache.decrease-replica-count-noop-rejected elasticache.global-replication-group-lifecycle-full-transition elasticache.global-replication-group-secondary-lifecycle elasticache.global-replication-group-topology-lifecycle elasticache.increase-node-groups-below-current-rejected elasticache.increase-replica-count-exceeds-limit-rejected elasticache.migration-complete-without-active-migration-rejected elasticache.migration-state-transitions elasticache.modify-replication-group-shard-configuration-invalid-rejected elasticache.modify-shard-configuration-disabled-cluster-mode-rejected elasticache.online-migration-lifecycle elasticache.online-migration-not-found-semantics elasticache.replica-count-state-gating elasticache.replication-group-auth-token-rotation-grace-window elasticache.replication-group-duplicate elasticache.replication-group-lifecycle elasticache.replication-group-migration-lifecycle elasticache.replication-group-not-found-semantics elasticache.shard-configuration-resize-transitions elasticache.shard-reconfiguration-state-machine elasticache.test-failover-state-gating
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache|CreateReplicationGroup|NotImplemented
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Implement elasticache CreateReplicationGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationGroup

## Description
`CreateReplicationGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_replication_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

27 recorded unit(s) fail on their `CreateReplicationGroup` call, so each one unblocks only when `CreateReplicationGroup` works: `elasticache.automatic-failover-promotes-replica`, `elasticache.concurrent-modify-replication-group-state-conflict`, `elasticache.concurrent-replica-count-increase-decrease-contention`, `elasticache.concurrent-shard-config-modification-conflict`, `elasticache.create-global-replication-group-unknown-primary`, `elasticache.create-replication-group-invalid-shard-config-rejected`, `elasticache.decrease-replica-count-noop-rejected`, `elasticache.global-replication-group-lifecycle-full-transition`, `elasticache.global-replication-group-secondary-lifecycle`, `elasticache.global-replication-group-topology-lifecycle`, `elasticache.increase-node-groups-below-current-rejected`, `elasticache.increase-replica-count-exceeds-limit-rejected`, `elasticache.migration-complete-without-active-migration-rejected`, `elasticache.migration-state-transitions`, `elasticache.modify-replication-group-shard-configuration-invalid-rejected`, `elasticache.modify-shard-configuration-disabled-cluster-mode-rejected`, `elasticache.online-migration-lifecycle`, `elasticache.online-migration-not-found-semantics`, `elasticache.replica-count-state-gating`, `elasticache.replication-group-auth-token-rotation-grace-window` (+7 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/create_replication_group.rs`. Replaying the corpus, the emulator refused the `CreateReplicationGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateReplicationGroup operation: operation CreateReplicationGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the elasticache emulator
Call CreateReplicationGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
