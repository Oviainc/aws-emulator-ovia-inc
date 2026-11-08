Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.cache-cluster-duplicate-id elasticache.cache-subnet-group-dependent-resource-lifecycle elasticache.cache-subnet-group-in-use-forbidden elasticache.create-cache-cluster-invalid-arguments elasticache.create-cache-cluster-unsupported-engine-version-rejected elasticache.create-replication-group-authtoken-without-transit-encryption-rejected elasticache.auth-token-invalid-characters-rejected elasticache.create-snapshot-unsupported-node-type-rejected elasticache.global-replication-group-topology-lifecycle elasticache.migration-state-transitions
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.cache-cluster-duplicate-id|run#7|AssertionError|assert refused == (
Issue-Key: elasticache.cache-subnet-group-dependent-resource-lifecycle|run#20|AssertionError|assert message == in_use_message, f"expected message {in_use_message!r}, got {message!r}"
Issue-Key: elasticache.cache-subnet-group-in-use-forbidden|run#17|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.create-cache-cluster-invalid-arguments|_assert_error#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.create-cache-cluster-unsupported-engine-version-rejected|run#11|AssertionError|assert code == "InvalidParameterCombination", f"expected InvalidParameterCombination, got {code}"
Issue-Key: elasticache.create-replication-group-authtoken-without-transit-encryption-rejected|run#11|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: elasticache.auth-token-invalid-characters-rejected|run#9|AssertionError|assert message == "Invalid AuthToken provided.", f"unexpected InvalidParameterValue message: {message!r}"
Issue-Key: elasticache.create-snapshot-unsupported-node-type-rejected|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: elasticache.global-replication-group-topology-lifecycle|run#17|AssertionError|assert message == (
Issue-Key: elasticache.migration-state-transitions|run#11|AssertionError|assert (code, message) == ("InvalidReplicationGroupState", _START_NOT_ALLOWED_MESSAGE), (
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# ElastiCache refusals diverge from AWS on error code and message text across nine validation paths (9 causes)

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCacheCluster

## Description
Nine independent root causes on the IMPLEMENTED ElastiCache refusal surface. Each refusal happens (HTTP 400 as AWS does) but the awsQuery Error/Code and/or Error/Message the emulator serializes is not the one AWS serves, so a client that branches on the code or surfaces the message gets something AWS never emits. Each cause is a separate decision in a different handler and must be closed separately. Related open issues: 20261102T083323Z-elasticache-refusals-carry-the-wrong-error-code-and.md (a WIRE issue covering seven DIFFERENT refusal paths; its Cause 6/7 concern the same AuthToken validations as Causes 5-6 here but carry no behavioral replay keys) and 20261105T143748Z-elasticache-teardown-refusals-diverge-from-aws-a-valid.md. Neither owns any check id below (the replay report lists no owner for them).

Cause 1 of 9: the CacheClusterAlreadyExists refusal interpolates the requested id into a message whose AWS text is fixed.
AWS: Smithy: CreateCacheCluster models CacheClusterAlreadyExistsFault -> awsQueryError code 'CacheClusterAlreadyExists', httpResponseCode 400. Recorded behavior: HTTP 400 with Message 'Cache cluster already exists' — the WHOLE string is fixed text for this class; no value from the request and no trailing period.
Emulator: HTTP 400, correct code, but Message 'Cache cluster <CacheClusterId> already exists.' — the request's id is interpolated and a period appended.
Why: a client matching the message to distinguish an id collision from another 400, or asserting on it, diverges; the echo of the id is not part of AWS's contract for this error.
Fix: the CreateCacheCluster duplicate-id refusal message template in the elasticache crate.
Traces: elasticache.cache-cluster-duplicate-id.

Cause 2 of 9: the CacheSubnetGroupInUse refusal drops the clause naming WHAT holds the group.
AWS: Smithy: DeleteCacheSubnetGroup models CacheSubnetGroupInUse (awsQuery code 'CacheSubnetGroupInUse', 400). Recorded behavior: HTTP 400 with Message 'Cache subnet group <name> is currently in use by a cache cluster.' Fixed text: 'Cache subnet group ' ... ' is currently in use by a cache cluster.'; the one variable part is the CacheSubnetGroupName from the request.
Emulator: 'Cache subnet group <name> is currently in use.' — the 'by a cache cluster' clause is missing.
Why: the message is the only place AWS says which dependency class blocks the delete; a client (or a human reading a teardown failure) loses that.
Fix: the DeleteCacheSubnetGroup in-use refusal message template in the elasticache crate.
Traces: elasticache.cache-subnet-group-dependent-resource-lifecycle, elasticache.cache-subnet-group-in-use-forbidden.

Cause 3 of 9: an unknown Engine on CreateCacheCluster is refused with a generic enum message instead of AWS's engine message.
AWS: Smithy: CreateCacheCluster.Engine is a plain String; InvalidParameterValueException -> code 'InvalidParameterValue', 400. Recorded behavior: HTTP 400 Message 'Invalid Cache engine: <engine>.' Fixed text 'Invalid Cache engine: ' + the Engine value from the request + '.'.
Emulator: 'Invalid value <engine> for Engine: must be one of memcached | redis | valkey.' — a generic 'Invalid value X for <member>' template that also discloses the emulator's own allow-list.
Why: message text is part of the refusal contract; the emulator's sentence shape appears nowhere in AWS.
Fix: the CreateCacheCluster engine validation message in the elasticache crate.
Traces: elasticache.create-cache-cluster-invalid-arguments.

Cause 4 of 9: an EngineVersion that is not offered is refused with the wrong error CODE and message.
AWS: Smithy: CreateCacheCluster models BOTH InvalidParameterCombinationException (code 'InvalidParameterCombination', 400) and InvalidParameterValueException (code 'InvalidParameterValue', 400). Recorded behavior for a version absent from DescribeCacheEngineVersions: HTTP 400, Code 'InvalidParameterCombination', Message 'Cannot find version <version> for <engine>' — fixed text 'Cannot find version ' + the requested EngineVersion + ' for ' + the requested Engine, no trailing period.
Emulator: HTTP 400, Code 'InvalidParameterValue', Message 'Invalid value <version> for EngineVersion: no cache parameter group family supports it.'
Why: botocore builds the exception class from the code, so `except client.exceptions.InvalidParameterCombinationException` never fires and version-negotiation logic takes the wrong branch; the message is also not AWS's.
Fix: the CreateCacheCluster (and CreateReplicationGroup) engine-version resolution refusal in the elasticache crate — classify as InvalidParameterCombination with AWS's template. The same handler path is the one the ec2__elasticache checks hit under 20261108T093128Z-ec2-elasticache-elasticache-in-vpc-control-plane-diverges-from-aws.md.
Traces: elasticache.create-cache-cluster-unsupported-engine-version-rejected.

Cause 5 of 9: AuthToken without transit encryption is refused under InvalidParameterCombination instead of InvalidParameterValue.
AWS: Smithy: CreateReplicationGroup takes AuthToken (String) and TransitEncryptionEnabled (BooleanOptional) and models both error shapes. Recorded behavior: HTTP 400, Code 'InvalidParameterValue', Message 'The AUTH token is only supported when encryption-in-transit is enabled' (entirely fixed text, no trailing period, nothing interpolated).
Emulator: HTTP 400, Code 'InvalidParameterCombination', Message 'AuthToken can be specified only on a replication group where TransitEncryptionEnabled is true.'
Why: the SDK exception class and the user-facing text both differ from AWS for the single most common auth misconfiguration.
Fix: the CreateReplicationGroup AuthToken/TransitEncryption validation in the elasticache crate (classification + message).
Traces: elasticache.create-replication-group-authtoken-without-transit-encryption-rejected, ec2__elasticache.vvd-auth-token-without-transit-encryption-rejected (same handler; listed as evidence only, that composite check is owned elsewhere).

Cause 6 of 9: the invalid-AuthToken refusal leaks the password rule instead of AWS's opaque message.
AWS: Smithy: CreateReplicationGroup.AuthToken is a plain String with no modeled character constraint; the refusal is InvalidParameterValueException -> 'InvalidParameterValue', 400. Recorded behavior: Message 'Invalid AuthToken provided.' — entirely fixed text, deliberately with no detail about WHY the token is invalid.
Emulator: 'Invalid value for AuthToken: the password must be printable ASCII characters, and the only permitted printable special characters are !, &, #, $, ^, <, >, -.'
Why: the wording is not AWS's for this class, and it discloses the credential policy AWS keeps out of the response.
Fix: the CreateReplicationGroup AuthToken validation message in the elasticache crate.
Traces: elasticache.auth-token-invalid-characters-rejected.

Cause 7 of 9: CreateSnapshot on a memcached cluster is refused with the emulator's own sentence.
AWS: Smithy: CreateSnapshot models SnapshotFeatureNotSupportedFault and InvalidParameterValueException (400). Recorded behavior: HTTP 400 with Message 'The engine type of this cluster does not support snapshotting.' — the whole string is fixed text; nothing from the request is echoed.
Emulator: 'Cache cluster <id> runs memcached, and snapshots are supported for valkey and redis only.'
Why: a client cannot match the refusal class on the message, and the emulator's sentence does not exist in AWS.
Fix: the CreateSnapshot engine-support refusal message in the elasticache crate.
Traces: elasticache.create-snapshot-unsupported-node-type-rejected.

Cause 8 of 9: DisassociateGlobalReplicationGroup on the PRIMARY member answers a generic 'Invalid value X for <member>' message.
AWS: Smithy: DisassociateGlobalReplicationGroup requires GlobalReplicationGroupId/ReplicationGroupId/ReplicationGroupRegion and models InvalidParameterValueException (code 'InvalidParameterValue', 400). Recorded behavior: HTTP 400, Code 'InvalidParameterValue', Message 'Cannot disassociate primary replication group. Primary Replication Group will be disassociated when you delete the Global Replication Group.' — two fixed sentences, nothing interpolated.
Emulator: correct code, but Message 'Invalid value <rg id> for ReplicationGroupId: the primary of Global Replication Group <grg id> cannot be disassociated.' — the shared 'Invalid value <v> for <member>' template with both ids interpolated.
Why: the message is the only signal that the right remedy is DeleteGlobalReplicationGroup; AWS's instruction sentence is absent.
Fix: the DisassociateGlobalReplicationGroup primary-member refusal message in the elasticache crate.
Traces: elasticache.global-replication-group-topology-lifecycle.

Cause 9 of 9: a duplicate StartMigration while the first is applying is refused with the generic 'must be in available state to modify' message.
AWS: Smithy: StartMigration models InvalidReplicationGroupStateFault (code 'InvalidReplicationGroupState', 400) and ReplicationGroupAlreadyUnderMigrationFault. Recorded behavior for a second StartMigration while the group is still applying the first: HTTP 400, Code 'InvalidReplicationGroupState', Message 'Replication group must be in available or migrating status to start migration.' — fixed text naming BOTH acceptable states and the migration operation.
Emulator: same code, Message 'Replication group must be in available state to modify.' — the generic modify-state sentence.
Why: the emulator's text omits the 'or migrating' state and says 'modify' rather than 'start migration', so a client cannot tell this refusal from any other state refusal.
Fix: the StartMigration state refusal message in the elasticache crate (a migration-specific template rather than the shared modify-state one).
Traces: elasticache.migration-state-transitions.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout (68/161 passed). Expected = recorded AWS behavior, actual = emulator:
1. cache-cluster-duplicate-id: expected ('CacheClusterAlreadyExists', 'Cache cluster already exists'), got ('CacheClusterAlreadyExists', 'Cache cluster sim2real-behav-ecc-dup-573b572ff79b already exists.') — the generated id is a per-run value; the requirement is that AWS interpolates nothing here.
2. cache-subnet-group-in-use-forbidden / cache-subnet-group-dependent-resource-lifecycle: expected '<name> ... is currently in use by a cache cluster.', got '... is currently in use.'
3. create-cache-cluster-invalid-arguments: expected message 'Invalid Cache engine: not-an-engine.', got 'Invalid value not-an-engine for Engine: must be one of memcached | redis | valkey.'
4. create-cache-cluster-unsupported-engine-version-rejected: CreateCacheCluster(Engine='redis', EngineVersion='99.99.99') -> expected 400 InvalidParameterCombination 'Cannot find version 99.99.99 for redis'; got InvalidParameterValue (AssertionError: expected InvalidParameterCombination, got InvalidParameterValue). The sibling ec2__elasticache run shows the emulator message for the same path: 'Invalid value 6.2 for EngineVersion: no cache parameter group family supports it.'
5. create-replication-group-authtoken-without-transit-encryption-rejected: AssertionError: CreateReplicationGroup with AuthToken but no TransitEncryptionEnabled was refused with an unexpected error code 'InvalidParameterCombination'; expected InvalidParameterValue with 'The AUTH token is only supported when encryption-in-transit is enabled'.
6. auth-token-invalid-characters-rejected: expected 'Invalid AuthToken provided.', got 'Invalid value for AuthToken: the password must be printable ASCII characters, and the only permitted printable special characters are !, &, #, $, ^, <, >, -.'
7. create-snapshot-unsupported-node-type-rejected: expected 'The engine type of this cluster does not support snapshotting.', got 'Cache cluster sim2real-ecmc-0c62b40651e5 runs memcached, and snapshots are supported for valkey and redis only.'
8. global-replication-group-topology-lifecycle: expected 'Cannot disassociate primary replication group. Primary Replication Group will be disassociated when you delete the Global Replication Group.', got 'Invalid value sim2real-grgprim-299f9f083c91 for ReplicationGroupId: the primary of Global Replication Group ibpgl-sim2realgds39788 cannot be disassociated.'
9. migration-state-transitions: expected ('InvalidReplicationGroupState', 'Replication group must be in available or migrating status to start migration.'), got ('InvalidReplicationGroupState', 'Replication group must be in available state to modify.')
Model evidence (services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json): awsQuery protocol; CacheClusterAlreadyExistsFault -> {code CacheClusterAlreadyExists, 400}; InvalidParameterValueException -> {InvalidParameterValue, 400}; InvalidParameterCombinationException -> {InvalidParameterCombination, 400}; InvalidReplicationGroupStateFault -> {InvalidReplicationGroupState, 400}; CreateCacheCluster, CreateReplicationGroup, StartMigration and DisassociateGlobalReplicationGroup all bind these.

## Steps to Reproduce
1. Boot the emulator from this checkout and run suite simparity-behavioral-aws-v1 (or drive the calls below with botocore against it).
2. Cause 1: CreateCacheCluster twice with the same CacheClusterId -> expect 400 'Cache cluster already exists'; observe the id interpolated. Re-replay elasticache.cache-cluster-duplicate-id.
3. Cause 2: create a cache cluster in a subnet group, then DeleteCacheSubnetGroup on that group -> expect '... is currently in use by a cache cluster.'; observe the truncated sentence. Re-replay elasticache.cache-subnet-group-in-use-forbidden and elasticache.cache-subnet-group-dependent-resource-lifecycle.
4. Cause 3: CreateCacheCluster(Engine='not-an-engine', CacheNodeType='cache.t3.micro', NumCacheNodes=1) -> expect 'Invalid Cache engine: not-an-engine.'. Re-replay elasticache.create-cache-cluster-invalid-arguments.
5. Cause 4: CreateCacheCluster(Engine='redis', EngineVersion='99.99.99', ...) -> expect 400 InvalidParameterCombination 'Cannot find version 99.99.99 for redis'. Re-replay elasticache.create-cache-cluster-unsupported-engine-version-rejected.
6. Cause 5: CreateReplicationGroup with AuthToken set and TransitEncryptionEnabled absent -> expect 400 InvalidParameterValue 'The AUTH token is only supported when encryption-in-transit is enabled'. Re-replay elasticache.create-replication-group-authtoken-without-transit-encryption-rejected.
7. Cause 6: CreateReplicationGroup with TransitEncryptionEnabled=True and AuthToken='Aa1@34567890ABCD' -> expect 400 InvalidParameterValue 'Invalid AuthToken provided.'. Re-replay elasticache.auth-token-invalid-characters-rejected.
8. Cause 7: CreateSnapshot(CacheClusterId=<a memcached cluster>, SnapshotName=...) -> expect 400 'The engine type of this cluster does not support snapshotting.'. Re-replay elasticache.create-snapshot-unsupported-node-type-rejected.
9. Cause 8: CreateGlobalReplicationGroup from a primary, then DisassociateGlobalReplicationGroup naming that primary -> expect the two-sentence AWS message. Re-replay elasticache.global-replication-group-topology-lifecycle.
10. Cause 9: on an available replication group with AutomaticFailoverEnabled=True, StartMigration once, then immediately StartMigration again -> expect 400 InvalidReplicationGroupState 'Replication group must be in available or migrating status to start migration.'. Re-replay elasticache.migration-state-transitions.
