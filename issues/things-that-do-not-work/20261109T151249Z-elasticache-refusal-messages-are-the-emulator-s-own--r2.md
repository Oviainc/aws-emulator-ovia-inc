Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.cache-subnet-group-invalid-subnet elasticache.delete-default-parameter-group-forbidden elasticache.tags-add-list-remove elasticache.create-cache-cluster-invalid-arguments elasticache.create-serverless-cache-conflicting-name-rejected elasticache.serverless-cache-lifecycle elasticache.modify-parameter-group-batch-atomicity-on-unknown-parameter elasticache.parameter-value-outside-allowed-values-rejected elasticache.e2e-snapshot-unsupported-memcached-engine-rejected elasticache.reboot-cache-cluster-invalid-state elasticache.modify-cache-cluster-apply-immediately-false-pends elasticache.modify-shard-configuration-disabled-cluster-mode-rejected elasticache.snapshot-lifecycle-from-cluster
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.cache-subnet-group-invalid-subnet|run#13|AssertionError|assert refused == (
Issue-Key: elasticache.delete-default-parameter-group-forbidden|_assert_error#17|AssertionError|assert got == expected[1], f"{expected[0]}: expected message {expected[1]!r}, got {got!r}"
Issue-Key: elasticache.tags-add-list-remove|_assert_error#21|AssertionError|assert actual == expected[1], f"{expected[0]}: expected message {expected[1]!r}, got {actual!r}"
Issue-Key: elasticache.create-cache-cluster-invalid-arguments|_assert_error#15|AssertionError|assert message == expected[1], f"expected message {expected[1]!r}, got {message!r}"
Issue-Key: elasticache.create-serverless-cache-conflicting-name-rejected|run#9|AssertionError|assert message == "Serverless Cache already exists", message
Issue-Key: elasticache.serverless-cache-lifecycle|_assert_error#12|AssertionError|assert got == expected[1], f"expected message {expected[1]!r}, got {got!r}"
Issue-Key: elasticache.modify-parameter-group-batch-atomicity-on-unknown-parameter|_assert_error#27|AssertionError|assert message == expected[1], (
Issue-Key: elasticache.parameter-value-outside-allowed-values-rejected|_assert_error#24|AssertionError|assert message == expected[1], (
Issue-Key: elasticache.e2e-snapshot-unsupported-memcached-engine-rejected|run#15|AssertionError|assert message == "The engine type of this cluster does not support snapshotting.", (
Issue-Key: elasticache.reboot-cache-cluster-invalid-state|_assert_error#9|AssertionError|assert message == expected[1], f"expected message {expected[1]!r}, got {message!r}"
Issue-Key: elasticache.modify-cache-cluster-apply-immediately-false-pends|_assert_error#24|AssertionError|assert message == expected_message, (
Issue-Key: elasticache.modify-shard-configuration-disabled-cluster-mode-rejected|run#16|AssertionError|assert message == "Operation is only applicable for cluster mode enabled replication groups.", message
Issue-Key: elasticache.snapshot-lifecycle-from-cluster|_assert_error#19|AssertionError|assert actual == expected[1], f"{expected[0]}: expected message {expected[1]!r}, got {actual!r}"
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# ElastiCache refusal messages are the emulator's own prose, not AWS's, across ten validation paths (10 causes)

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheCluster

## Description
Ten net-new root causes in the elasticache emulator. In each the emulator refuses correctly (right HTTP status, right error code) but writes its own sentence instead of the message AWS returns for that error class, so clients that match on message text, and humans reading logs, see something AWS never says. Each cause below names its own fixed text and which parts are values taken from the request or the emulator's state. The suite marked none of these checks ALREADY FILED or OWNED.

Cause 1 of 10: DescribeCacheSubnetGroups not-found message.
  AWS: Smithy CacheSubnetGroupNotFoundFault, awsQueryError code `CacheSubnetGroupNotFound`, HTTP 400. AWS answers `Cache subnet group <name> not found.` — fixed text `Cache subnet group ` + ` not found.`, variable part the requested CacheSubnetGroupName, unquoted.
  Emulator: `Cache subnet group '<name>' does not exist.` (quotes the name, different predicate).
  Why: a client keying teardown/idempotency on the not-found text treats the group as existing.
  Fix: DescribeCacheSubnetGroups' not-found path (services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/describe_cache_subnet_groups.rs).
  Traces: elasticache.cache-subnet-group-invalid-subnet.

Cause 2 of 10: CacheParameterGroupNotFound message.
  AWS: Smithy CacheParameterGroupNotFoundFault, awsQueryError code `CacheParameterGroupNotFound`, HTTP 404. AWS answers `CacheParameterGroup <name> not found.` — fixed text `CacheParameterGroup ` + ` not found.`, variable part the requested CacheParameterGroupName.
  Emulator: `CacheParameterGroup not found: <name>` (reversed order, no period).
  Why: both DescribeCacheParameterGroups-after-delete and ListTagsForResource on a missing group hand back text no AWS client recognizes.
  Fix: the shared CacheParameterGroupNotFound error construction used by the parameter-group handlers and ListTagsForResource. Related open issue 20261105T143748Z-elasticache-teardown-refusals-diverge-from-aws-a-valid.md mentions this message family for other paths; it does not carry these two checks.
  Traces: elasticache.delete-default-parameter-group-forbidden, elasticache.tags-add-list-remove.

Cause 3 of 10: CreateCacheCluster unknown Engine message.
  AWS: Smithy CreateCacheCluster error InvalidParameterValueException (awsQueryError `InvalidParameterValue`, HTTP 400). AWS answers `Invalid Cache engine: <engine>.` — fixed text `Invalid Cache engine: ` + `.`, variable part the rejected Engine value from the request.
  Emulator: `Invalid value <engine> for Engine: must be one of memcached | redis | valkey.`
  Why: a client that distinguishes an unknown engine from other InvalidParameterValue cases by message cannot.
  Fix: CreateCacheCluster's engine validation (src/ops/create_cache_cluster.rs).
  Traces: elasticache.create-cache-cluster-invalid-arguments.

Cause 4 of 10: ServerlessCacheAlreadyExists message.
  AWS: Smithy ServerlessCacheAlreadyExistsFault, awsQueryError code `ServerlessCacheAlreadyExistsFault`, HTTP 400. AWS answers the fixed sentence `Serverless Cache already exists` — no variable part, no trailing period.
  Emulator: `Serverless cache <name> already exists.` (interpolates the name, different casing and punctuation).
  Why: two independent checks of the conflict path mis-read the refusal; a client comparing the documented text never matches.
  Fix: CreateServerlessCache's name-conflict path (src/ops/create_serverless_cache.rs).
  Traces: elasticache.create-serverless-cache-conflicting-name-rejected, elasticache.serverless-cache-lifecycle.

Cause 5 of 10: a parameter value outside a parameter's AllowedValues.
  AWS: Smithy ModifyCacheParameterGroup error InvalidParameterValueException (`InvalidParameterValue`, HTTP 400). AWS answers `invalid parameter value, allowed values are:<v1>,<v2>,...` — fixed text `invalid parameter value, allowed values are:`, variable part the parameter's AllowedValues list in model order, comma-separated with no spaces.
  Emulator: `value:<value> is outside of range:<v1>,<v2>,...` — a different sentence that leaks the rejected value and calls the enumeration a range.
  Why: a client (or console) that shows AWS's allowed-values text, or parses it, gets prose AWS never emits; in the mixed-batch check it is also the only signal that the batch was rejected atomically.
  Fix: the parameter-value validator shared by ModifyCacheParameterGroup (src/ops/modify_cache_parameter_group.rs) and the parameter catalog it validates against. Related open issues 20261104T091523Z-elasticache-createserverlesscache-refuses-every-real-subnet-id-the.md and 20261108T152921Z-elasticache-refusals-diverge-from-aws-on-error-code.md carry this text for OTHER checks; they do not carry these two.
  Traces: elasticache.modify-parameter-group-batch-atomicity-on-unknown-parameter, elasticache.parameter-value-outside-allowed-values-rejected.

Cause 6 of 10: snapshotting a memcached cluster.
  AWS: Smithy SnapshotFeatureNotSupportedFault, awsQueryError `SnapshotFeatureNotSupportedFault`, HTTP 400 (doc: creating a snapshot of a cluster running Memcached is not supported). AWS answers the fixed sentence `The engine type of this cluster does not support snapshotting.` — no variable part.
  Emulator: `Cache cluster <cluster-id> runs memcached, and snapshots are supported for valkey and redis only.`
  Why: a client branching on the documented sentence (or showing it) sees emulator prose.
  Fix: CreateSnapshot's engine gate (src/ops/create_snapshot.rs). Related open issue 20261108T152921Z-elasticache-refusals-diverge-from-aws-on-error-code.md carries this text for elasticache.create-snapshot-unsupported-node-type-rejected only.
  Traces: elasticache.e2e-snapshot-unsupported-memcached-engine-rejected.

Cause 7 of 10: CreateSnapshot against a cluster that is not available.
  AWS: Smithy CreateSnapshot error InvalidCacheClusterStateFault (awsQueryError `InvalidCacheClusterState`, HTTP 400). AWS answers the fixed sentence `Cluster must be in available state in order to create a snapshot.` — no variable part.
  Emulator: `Cache cluster <cluster-id> is in creating state, not available.` (interpolates the id and the current status).
  Why: a retry loop keyed on AWS's sentence does not recognize the transient state gate.
  Fix: CreateSnapshot's cluster-state gate (src/ops/create_snapshot.rs).
  Traces: elasticache.reboot-cache-cluster-invalid-state.

Cause 8 of 10: ModifyCacheCluster with NumCacheNodes=0.
  AWS: Smithy ModifyCacheCluster error InvalidParameterValueException (`InvalidParameterValue`, HTTP 400). AWS answers `NumCacheNodes must be greater than or equal to 1.` — fixed text, no variable part.
  Emulator: `Invalid value 0 for NumCacheNodes: a memcached cluster must have between 1 and 40 cache nodes.`
  Why: the refusal must name its own reason in AWS's words; a client matching the documented floor message does not match.
  Fix: ModifyCacheCluster's NumCacheNodes validation (src/ops/modify_cache_cluster.rs).
  Traces: elasticache.modify-cache-cluster-apply-immediately-false-pends.

Cause 9 of 10: ModifyReplicationGroupShardConfiguration on a cluster-mode-disabled group.
  AWS: Smithy ModifyReplicationGroupShardConfiguration error InvalidParameterValueException (`InvalidParameterValue`, HTTP 400). AWS answers the fixed sentence `Operation is only applicable for cluster mode enabled replication groups.` — no variable part.
  Emulator: `Invalid value <replication-group-id> for ReplicationGroupId: shards can only be configured on a cluster mode enabled replication group.`
  Why: a client that detects "wrong cluster mode" from AWS's sentence cannot, and the emulator blames the identifier rather than the group's mode.
  Fix: the shard-configuration handler's cluster-mode gate (src/ops/modify_replication_group_shard_configuration.rs).
  Traces: elasticache.modify-shard-configuration-disabled-cluster-mode-rejected.

Cause 10 of 10: CopySnapshot onto an existing destination name.
  AWS: Smithy SnapshotAlreadyExistsFault, awsQueryError `SnapshotAlreadyExistsFault`, HTTP 400. AWS answers the fixed sentence `A destination snapshot with the specified name already exists` — no variable part, no trailing period.
  Emulator: `Snapshot with specified name already exists.`
  Why: the copy-conflict case is indistinguishable from a create-conflict for a client matching AWS's destination-specific wording.
  Fix: CopySnapshot's destination-name conflict path (src/ops/copy_snapshot.rs).
  Traces: elasticache.snapshot-lifecycle-from-cluster.

## Evidence
Cause 1 — elasticache.cache-subnet-group-invalid-subnet: expected ('CacheSubnetGroupNotFound', 'Cache subnet group <name> not found.'), got ('CacheSubnetGroupNotFound', "Cache subnet group '<name>' does not exist.").
Cause 2 — elasticache.delete-default-parameter-group-forbidden and elasticache.tags-add-list-remove: `CacheParameterGroupNotFound: expected message 'CacheParameterGroup <name> not found.', got 'CacheParameterGroup not found: <name>'`.
Cause 3 — elasticache.create-cache-cluster-invalid-arguments: `expected message 'Invalid Cache engine: not-an-engine.', got 'Invalid value not-an-engine for Engine: must be one of memcached | redis | valkey.'`.
Cause 4 — elasticache.create-serverless-cache-conflicting-name-rejected and elasticache.serverless-cache-lifecycle: `expected message 'Serverless Cache already exists', got 'Serverless cache <name> already exists.'`.
Cause 5 — elasticache.parameter-value-outside-allowed-values-rejected and elasticache.modify-parameter-group-batch-atomicity-on-unknown-parameter: `expected message 'invalid parameter value, allowed values are:volatile-lru,allkeys-lru,volatile-lfu,allkeys-lfu,volatile-random,allkeys-random,volatile-ttl,noeviction', got 'value:<rejected value> is outside of range:<same list>'` (and the same shape for acl-pubsub-default with allowed values allchannels,resetchannels).
Cause 6 — elasticache.e2e-snapshot-unsupported-memcached-engine-rejected: `unexpected SnapshotFeatureNotSupportedFault message: 'Cache cluster <id> runs memcached, and snapshots are supported for valkey and redis only.'` where the expected message is 'The engine type of this cluster does not support snapshotting.'.
Cause 7 — elasticache.reboot-cache-cluster-invalid-state: `expected message 'Cluster must be in available state in order to create a snapshot.', got 'Cache cluster <id> is in creating state, not available.'`.
Cause 8 — elasticache.modify-cache-cluster-apply-immediately-false-pends: `ModifyCacheCluster with NumCacheNodes=0: the refusal must name its own reason; expected 'NumCacheNodes must be greater than or equal to 1.', got 'Invalid value 0 for NumCacheNodes: a memcached cluster must have between 1 and 40 cache nodes.'`.
Cause 9 — elasticache.modify-shard-configuration-disabled-cluster-mode-rejected: expected 'Operation is only applicable for cluster mode enabled replication groups.', got 'Invalid value <rg-id> for ReplicationGroupId: shards can only be configured on a cluster mode enabled replication group.'.
Cause 10 — elasticache.snapshot-lifecycle-from-cluster: `SnapshotAlreadyExistsFault: expected message 'A destination snapshot with the specified name already exists', got 'Snapshot with specified name already exists.'`.

## Steps to Reproduce
Cause 1: DescribeCacheSubnetGroups(CacheSubnetGroupName='<name that does not exist>') — compare Error.Message to 'Cache subnet group <name> not found.'.
Cause 2: DescribeCacheParameterGroups(CacheParameterGroupName='<missing>') and ListTagsForResource(ResourceName='<arn of a missing parameter group>').
Cause 3: CreateCacheCluster(CacheClusterId='<id>', Engine='not-an-engine', CacheNodeType='cache.t3.micro', NumCacheNodes=1).
Cause 4: CreateServerlessCache(ServerlessCacheName='<existing name>', Engine='valkey') twice.
Cause 5: ModifyCacheParameterGroup(CacheParameterGroupName='<group>', ParameterNameValues=[{'ParameterName':'maxmemory-policy','ParameterValue':'<not an allowed value>'}]), and the same inside a batch that also contains a valid pair.
Cause 6: CreateCacheCluster(Engine='memcached'), wait available, then CreateSnapshot(CacheClusterId=<that cluster>, SnapshotName='<name>').
Cause 7: CreateCacheCluster(Engine='redis') and, while it is still 'creating', CreateSnapshot(CacheClusterId=<it>, SnapshotName='<name>').
Cause 8: ModifyCacheCluster(CacheClusterId='<memcached cluster>', NumCacheNodes=0, ApplyImmediately=False).
Cause 9: CreateReplicationGroup with cluster mode disabled, then ModifyReplicationGroupShardConfiguration(ReplicationGroupId=<it>, NodeGroupCount=2, ApplyImmediately=True).
Cause 10: CreateSnapshot(SnapshotName='<a>'), CreateSnapshot(SnapshotName='<b>'), then CopySnapshot(SourceSnapshotName='<a>', TargetSnapshotName='<b>').
