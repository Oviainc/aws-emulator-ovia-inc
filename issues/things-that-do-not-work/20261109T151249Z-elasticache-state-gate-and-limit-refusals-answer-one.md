Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.test-failover-state-gating elasticache.concurrent-shard-config-modification-conflict elasticache.migration-complete-without-active-migration-rejected elasticache.serverless-cache-snapshot-retention-limit-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.test-failover-state-gating|run#12|AssertionError|assert message == f"Node group 9999 was not found for replication group {rg_id}", (
Issue-Key: elasticache.concurrent-shard-config-modification-conflict|run#16|AssertionError|assert failures[0][3] == "Could not reshard replication group. Please try again later.", (
Issue-Key: elasticache.migration-complete-without-active-migration-rejected|run#20|AssertionError|assert second_message == expected, f"expected {expected!r}, got {second_message!r}"
Issue-Key: elasticache.serverless-cache-snapshot-retention-limit-out-of-range-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# ElastiCache state-gate and limit refusals answer one generic message instead of AWS's per-operation text (4 causes)

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyReplicationGroupShardConfiguration

## Description
Four net-new root causes in the elasticache emulator. All four refuse with the right code but the wrong message: three replication-group gates collapse onto one generic sentence the emulator invented, and a serverless-cache limit check rewords AWS's retention-limit refusal. None of these checks was marked ALREADY FILED or OWNED by the suite.

Cause 1 of 4: TestFailover with an unknown NodeGroupId.
  AWS: Smithy NodeGroupNotFoundFault, awsQueryError code `NodeGroupNotFoundFault`, HTTP 404 (doc: the node group specified by the NodeGroupId parameter could not be found). AWS answers `Node group <node-group-id> was not found for replication group <replication-group-id>` — fixed text `Node group ` + ` was not found for replication group `, with NO trailing period; the two variable parts are the requested NodeGroupId and ReplicationGroupId.
  Emulator: `Node group <node-group-id> not found in replication group <replication-group-id>.` (different predicate, trailing period).
  Why: a client matching the documented sentence cannot tell an unknown shard from any other 404.
  Fix: TestFailover's node-group lookup/refusal (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/test_failover.rs).
  Traces: elasticache.test-failover-state-gating.

Cause 2 of 4: the losing concurrent ModifyReplicationGroupShardConfiguration.
  AWS: Smithy ModifyReplicationGroupShardConfiguration error InvalidReplicationGroupStateFault (awsQueryError `InvalidReplicationGroupState`, HTTP 400). When two reshards race, AWS accepts exactly one and refuses the other with the reshard-specific sentence `Could not reshard replication group. Please try again later.` — fixed text, no variable part.
  Emulator: the loser is refused `InvalidReplicationGroupState` (correct code) with the generic `Replication group must be in available state to modify.`
  Why: the AWS message tells a client this particular reshard lost a race and is retryable; the generic one is indistinguishable from "you called while the group was busy for any reason", so retry logic keyed on AWS's text never fires.
  Fix: the concurrency/state gate of the shard-configuration handler (src/ops/modify_replication_group_shard_configuration.rs) must emit the reshard-specific message rather than the shared modify-state sentence.
  Traces: elasticache.concurrent-shard-config-modification-conflict.

Cause 3 of 4: a second StartMigration while a migration is already in flight.
  AWS: Smithy StartMigration error InvalidReplicationGroupStateFault (`InvalidReplicationGroupState`, HTTP 400); AWS reports this as the migration-specific `Replication group must be in available or migrating status to start migration.` — fixed text, no variable part.
  Emulator: the same generic `Replication group must be in available state to modify.` is returned for StartMigration too.
  Why: a client driving the online-migration state machine cannot distinguish "a migration is already running" from any other busy state, and the emulator's sentence contradicts AWS's rule that a *migrating* group is an acceptable StartMigration state.
  Fix: StartMigration's state gate (src/ops/start_migration.rs) and the shared replication-group state-gate message helper.
  Traces: elasticache.migration-complete-without-active-migration-rejected.

Cause 4 of 4: CreateServerlessCache with a SnapshotRetentionLimit above the maximum.
  AWS: Smithy CreateServerlessCache error InvalidParameterValueException (awsQueryError `InvalidParameterValue`, HTTP 400). AWS answers `Invalid snapshot retention limit: <n>. Snapshot retention limit must be between 0 and 35.` — fixed text `Invalid snapshot retention limit: ` + `. Snapshot retention limit must be between 0 and 35.`, the single variable part being the rejected value from the request.
  Emulator: `Invalid value <n> for SnapshotRetentionLimit: must be at least 0 and no more than 35 days.`
  Why: right bound, wrong sentence — a client surfacing or matching AWS's limit message sees emulator prose.
  Fix: CreateServerlessCache's retention-limit validation (src/ops/create_serverless_cache.rs).
  Traces: elasticache.serverless-cache-snapshot-retention-limit-out-of-range-rejected.

## Evidence
Cause 1 — elasticache.test-failover-state-gating: `expected the exact NodeGroupNotFound message, got 'Node group 9999 not found in replication group <rg-id>.'` (expected `Node group 9999 was not found for replication group <rg-id>`).
Cause 2 — elasticache.concurrent-shard-config-modification-conflict: two ModifyReplicationGroupShardConfiguration calls released from one barrier; one wins, the loser fails with code InvalidReplicationGroupState but `loser reshard call failed with message 'Replication group must be in available state to modify.'` (expected 'Could not reshard replication group. Please try again later.').
Cause 3 — elasticache.migration-complete-without-active-migration-rejected: `expected 'Replication group must be in available or migrating status to start migration.', got 'Replication group must be in available state to modify.'`.
Cause 4 — elasticache.serverless-cache-snapshot-retention-limit-out-of-range-rejected: `expected message 'Invalid snapshot retention limit: 36. Snapshot retention limit must be between 0 and 35.', got 'Invalid value 36 for SnapshotRetentionLimit: must be at least 0 and no more than 35 days.'`.

## Steps to Reproduce
Cause 1: CreateReplicationGroup and wait 'available'; TestFailover(ReplicationGroupId=<rg>, NodeGroupId='9999') — compare Error.Message to 'Node group 9999 was not found for replication group <rg>' (no trailing period).
Cause 2: CreateReplicationGroup(NumNodeGroups=2, ReplicasPerNodeGroup=1, AutomaticFailoverEnabled=True, CacheNodeType='cache.m5.large'); wait 'available'; from two threads released by one barrier call ModifyReplicationGroupShardConfiguration(NodeGroupCount=3) and (NodeGroupCount=4). Exactly one must succeed; the loser must be 400 InvalidReplicationGroupState with 'Could not reshard replication group. Please try again later.'.
Cause 3: CreateReplicationGroup, wait 'available', StartMigration(CustomerNodeEndpointList=[{'Address':'203.0.113.10','Port':6379}]), then StartMigration again with another endpoint. Expect 400 InvalidReplicationGroupState with 'Replication group must be in available or migrating status to start migration.'.
Cause 4: CreateServerlessCache(ServerlessCacheName='<name>', Engine='valkey', SnapshotRetentionLimit=36). Expect 400 InvalidParameterValue with 'Invalid snapshot retention limit: 36. Snapshot retention limit must be between 0 and 35.'.
