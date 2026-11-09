Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.delete-replication-group-snapshotting-state-blocked elasticache.replication-group-migration-lifecycle
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.delete-replication-group-snapshotting-state-blocked|_assert_error#14|AssertionError|assert code == expected[0], f"expected {expected[0]}, got {code}"
Issue-Key: elasticache.replication-group-migration-lifecycle|run#18|AssertionError|assert code == "InvalidReplicationGroupState", f"expected InvalidReplicationGroupState, got {code}"
Issue-Commit: 41c5ae389f495755d1c1e47282b4b58fd25764aa

# ElastiCache DeleteReplicationGroup answers NotImplemented instead of its two InvalidReplicationGroupState state gates (snapshot in progress, migration in flight) (2 causes)

> _Note: this issue was identified against the code base at commit `41c5ae389f495755d1c1e47282b4b58fd25764aa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteReplicationGroup

## Description
Two net-new root causes in the elasticache emulator, both grounded in failing behavioral checks the suite marked neither ALREADY FILED nor OWNED. Both are reached through DeleteReplicationGroup but they are distinct refusals with distinct AWS wording and distinct preconditions, so each must be closed on its own.

Related open issues, named so a fixer can see what they do and do not say: 20261108T093128Z-elasticache-deletereplicationgroup-stub.md records only that the handler is an unmodified scaffold (it asks for an implementation and explicitly does not state any refusal, code or message); 20261109T010905Z-elasticache-deletereplicationgroup-never-applies-the-snapshot-in-progress.md describes Cause 1 but the suite does not report it as owning this check's replay key (the check's assertion site moved, so its recorded Issue-Key no longer matches) and it says nothing about the migration gate of Cause 2. Neither covers these two checks as filed.

Cause 1 of 2: DeleteReplicationGroup does not refuse a delete while a manual snapshot of one of the group's member cache clusters is in progress.
  AWS: Smithy (services/service-com-amazonaws-elasticache-amazonelasticachev9, DeleteReplicationGroup) models error InvalidReplicationGroupStateFault with awsQueryError code `InvalidReplicationGroupState` and httpError 400. AWS refuses the delete while a member cluster is being snapshotted with HTTP 400, code `InvalidReplicationGroupState`, message `Cache cluster <cache-cluster-id> is not in a valid state to be deleted.` — the fixed text is `Cache cluster ` and ` is not in a valid state to be deleted.`; the only variable part is the member cache cluster id, taken from the group's own state (the entry in MemberClusters the snapshot is running on), never a literal id from one run. The group must survive the refusal (a following DescribeReplicationGroups still returns exactly it), and once the snapshot reaches `available` the identical DeleteReplicationGroup must succeed (Status `deleting`, then the group is gone).
  Emulator: the call is answered with error code `NotImplemented` — replay verdict `AssertionError: expected InvalidReplicationGroupState, got NotImplemented`. There is no state gate, no modeled error code, no AWS message, and because the operation never succeeds the positive control (delete after the snapshot completes) can never be reached either.
  Why: a client that guards teardown on InvalidReplicationGroupState — retry-until-deletable loops, CloudFormation-style waiters, Terraform-style destroy retries — receives an error code that is not in the operation's modeled error set, cannot classify it as retryable, and either aborts teardown or treats the replication group as permanently undeletable. A client that relies on the refusal to tell it a snapshot is still running learns nothing about the group's real state.
  Fix: the DeleteReplicationGroup handler and its state validation (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_replication_group.rs), which must consult the member cluster / snapshot state before deleting and raise the modeled InvalidReplicationGroupStateFault.
  Traces: elasticache.delete-replication-group-snapshotting-state-blocked.

Cause 2 of 2: DeleteReplicationGroup does not refuse a delete while an online migration started by StartMigration is in flight, and the refusal it owes is a DIFFERENT message from Cause 1's.
  AWS: same modeled error (InvalidReplicationGroupStateFault, awsQueryError `InvalidReplicationGroupState`, httpError 400). After StartMigration the group leaves `available`, and AWS refuses DeleteReplicationGroup with HTTP 400, code `InvalidReplicationGroupState`, message `Replication group <replication-group-id> has status <status> which is not valid for deletion.` — the fixed text is `Replication group `, ` has status `, ` which is not valid for deletion.`; the variable parts are the ReplicationGroupId from the request and the group's current status from the emulator's state (`modifying` while a migration is in flight). The group must survive, and once the migration tracking clears the identical delete must succeed.
  Emulator: the call is answered with error code `NotImplemented` — replay verdict `AssertionError: expected InvalidReplicationGroupState, got NotImplemented` at the delete-during-migration step (the check's earlier CompleteMigration and StartMigration steps were answered, so the group does exist and the migration did start).
  Why: the whole migration lifecycle becomes untestable and unsafe for a client: it cannot distinguish "delete blocked because a migration is running" from "this emulator cannot delete at all", so a migration-then-cutover-then-teardown workflow neither completes nor reports a classifiable error.
  Fix: the same DeleteReplicationGroup handler and its state validation (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_replication_group.rs) plus the replication-group status/migration state it must read, which must emit this operation's status-based message rather than Cause 1's cluster-based message.
  Traces: elasticache.replication-group-migration-lifecycle.

## Evidence
Cause 1 — check elasticache.delete-replication-group-snapshotting-state-blocked [mismatch]:
  Expected: ClientError with Error.Code == `InvalidReplicationGroupState` (HTTP 400) and Error.Message == `Cache cluster <member-cluster-id> is not in a valid state to be deleted.`, where <member-cluster-id> is the group's sole MemberClusters entry; then DescribeReplicationGroups(ReplicationGroupId=<rg>) still returns exactly one group; then, after the snapshot reaches `available`, DeleteReplicationGroup succeeds and the group disappears.
  Actual: `AssertionError: expected InvalidReplicationGroupState, got NotImplemented` (check_elasticache_delete_replication_group_snapshotting_state_blocked.py:109, _assert_error) — DeleteReplicationGroup answered error code `NotImplemented`, which is not in the operation's modeled error set.

Cause 2 — check elasticache.replication-group-migration-lifecycle [mismatch]:
  Expected: after CreateReplicationGroup(NumCacheClusters=2, AutomaticFailoverEnabled=true) reaches `available` and StartMigration is accepted, DeleteReplicationGroup(ReplicationGroupId=<rg>) raises ClientError with Error.Code == `InvalidReplicationGroupState` (HTTP 400) and Error.Message == `Replication group <rg> has status modifying which is not valid for deletion.`, and DescribeReplicationGroups still returns exactly that group.
  Actual: `AssertionError: expected InvalidReplicationGroupState, got NotImplemented` (check_elasticache_replication_group_migration_lifecycle.py:77) — DeleteReplicationGroup answered error code `NotImplemented`; the status-based refusal message is never produced.

Smithy used for both: DeleteReplicationGroup errors include InvalidReplicationGroupStateFault {errorKind: client, httpError: 400, awsQueryError {code: InvalidReplicationGroupState, httpResponseCode: 400}} (model services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json).

## Steps to Reproduce
Cause 1 (snapshot-in-progress gate):
1. CreateCacheSubnetGroup over a subnet of a fresh VPC.
2. CreateReplicationGroup(Engine=redis, CacheNodeType=cache.t3.micro, NumCacheClusters=1, CacheSubnetGroupName=<sg>); wait for Status `available`.
3. Read the group's sole MemberClusters id and call CreateSnapshot(CacheClusterId=<member>, SnapshotName=<name>); poll DescribeSnapshots until the snapshot appears (status `creating`).
4. Call DeleteReplicationGroup(ReplicationGroupId=<rg>). Expect 400 `InvalidReplicationGroupState` / `Cache cluster <member> is not in a valid state to be deleted.`; the emulator answers `NotImplemented`.
5. Confirm DescribeReplicationGroups still returns the group; then wait for the snapshot to reach `available` and call DeleteReplicationGroup again — it must succeed and the group must go away.

Cause 2 (migration-in-flight gate):
1. CreateCacheSubnetGroup, then CreateReplicationGroup(Engine=redis, CacheNodeType=cache.t3.micro, NumCacheClusters=2, AutomaticFailoverEnabled=true); wait for `available`.
2. Call StartMigration(ReplicationGroupId=<rg>, CustomerNodeEndpointList=[{Address: <ip>, Port: 6379}]).
3. Call DeleteReplicationGroup(ReplicationGroupId=<rg>). Expect 400 `InvalidReplicationGroupState` / `Replication group <rg> has status modifying which is not valid for deletion.`; the emulator answers `NotImplemented`.
4. Confirm DescribeReplicationGroups(ReplicationGroupId=<rg>) still returns exactly that group; then, once the migration tracking clears, the identical DeleteReplicationGroup must succeed.
