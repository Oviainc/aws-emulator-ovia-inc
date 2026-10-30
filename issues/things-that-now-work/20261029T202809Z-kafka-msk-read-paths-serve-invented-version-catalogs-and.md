Issue-Source: trace-sweep-divergence
Issue-Trace: kafka-catalog-get-compatible-kafka-versions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kafka-catalog-get-compatible-kafka-versions kafka-catalog-list-kafka-versions kafka-create-replicator-nonexistent-clusters kafka-replicator-create-describe-delete-lifecycle
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka|GetCompatibleKafkaVersions|2xx->2xx|$.compatibleKafkaVersions[*]
Issue-Key: kafka|ListKafkaVersions|2xx->2xx|$.kafkaVersions[*]
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.creationTime
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.kafkaClusters[*]
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].consumerGroupReplication.consumerGroupOffsetSyncMode
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].consumerGroupReplication.consumerGroupsToExclude
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].sourceKafkaClusterAlias
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].targetKafkaClusterAlias
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].topicReplication.startingPosition
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].topicReplication.topicNameConfiguration
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].topicReplication.topicsToExclude
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicatorState
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.stateInfo
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].consumerGroupReplication.detectAndCopyNewConsumerGroups
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].consumerGroupReplication.synchroniseConsumerGroupOffsets
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].topicReplication.copyAccessControlListsForTopics
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].topicReplication.copyTopicConfigurations
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.replicationInfoList[*].topicReplication.detectAndCopyNewTopics
Issue-Key: kafka|DescribeReplicator|2xx->2xx|$.tags
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# MSK read paths serve invented version catalogs and replicator projections (4 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCompatibleKafkaVersions

## Description
FOUR distinct root causes sharing one fault: a 200 read path returns content the emulator invented rather than what MSK reports - two version catalogs that do not match the service's own, and two replicator projections whose state, state detail and cluster aliases are wrong. The status is 200 in all four, so every one of these is a silent wrong answer. All four must be closed.

Cause 1 of 4 - GetCompatibleKafkaVersions serves a fabricated compatibility matrix.
AWS: Smithy kafka#GetCompatibleKafkaVersions (GET /v1/compatible-kafka-versions, ClusterArn bound to httpQuery 'clusterArn') returns `compatibleKafkaVersions` as a list of {sourceVersion, targetVersions}. Recorded behavior: the matrix MSK serves covers the versions the service currently offers - including the KRaft and tiered-storage variants (e.g. a 3.9.x source whose only target is 3.9.x.kraft; a 3.9.x.kraft source whose targets are the 4.x KRaft lines; a deprecated 3.8.link source with an EMPTY targetVersions list) - and each source's target list is the set of upgrades actually permitted FROM that version, which is a small subset, not one blanket list.
Emulator: a different source-version set (legacy 1.1.1, 2.1.0, 2.3.1, 2.4.1, 2.5.1, 2.6.2 ...) and, for every source, the SAME blanket target list ['2.8.1','2.8.2.tiered','3.1.1','3.2.0','3.3.1','3.3.2','3.4.0','3.5.1','3.6.0','3.7.x','3.8.x','3.9.x'] - i.e. a hard-coded table, not a per-source upgrade graph. No source with an empty target list exists, and no KRaft source appears at all.
Why: UpdateClusterKafkaVersion callers read this matrix to choose a legal target. Against the emulator they are offered upgrades MSK forbids and denied the ones it permits, and KRaft upgrade paths are invisible. A client that asserts a terminal version (empty targetVersions) never sees one.
Fix: services/service-com-amazonaws-kafka-kafka/src/ops/get_compatible_kafka_versions.rs must derive the matrix from the same version catalog ListKafkaVersions serves, as a per-source upgrade graph rather than one shared target list.
Traces: kafka-catalog-get-compatible-kafka-versions#task#1.

Cause 2 of 4 - ListKafkaVersions reports every version ACTIVE and omits the KRaft/variant lines.
AWS: Smithy kafka#ListKafkaVersions (GET /v1/kafka-versions) returns `kafkaVersions` as {version, status}. Recorded behavior: the catalog carries BOTH statuses - the older lines (2.8.1, 2.8.2.tiered, 3.1.1, 3.2.0, 3.3.1, 3.3.2, 3.4.0, 3.5.1, 3.6.0.1, 3.8.link, 3.9.x, 4.2.x.kraft ...) are DEPRECATED, while the current ones (3.7.x.kraft, 3.8.x.kraft, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft ...) are ACTIVE - and it includes the KRaft and tiered variants.
Emulator: a shorter list in which EVERY entry carries status 'ACTIVE', with the KRaft entries (3.8.x.kraft, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft, 4.2.x.kraft) and 3.8.link absent entirely (the list ends early - those indices are reported as absent).
Why: `status` is the only signal a client has that a version is on the way out; marking every version ACTIVE means a deprecation check can never fire, and omitting the KRaft lines means a client cannot select a KRaft cluster version at all. It is also the catalog Cause 1 and CreateCluster's version validation should both read, so one wrong table propagates.
Fix: ops/list_kafka_versions.rs - serve the full catalog with per-version status, from a single source the create/validation paths share.
Traces: kafka-catalog-list-kafka-versions#task#1.

Cause 3 of 4 - DescribeReplicator reports a replicator over nonexistent clusters as RUNNING with no stateInfo.
AWS: Smithy kafka#ReplicatorState is an enum with values RUNNING, CREATING, UPDATING, DELETING, FAILED, and kafka#ReplicationStateInfo carries {code, message}. Recorded behavior: a replicator created over MSK cluster ARNs that do not exist converges to `replicatorState` FAILED with `stateInfo` = {code: 'InvalidInput.InvalidKafkaCluster', message: "The specified Amazon MSK Cluster does not exist. Cluster ARN: <the offending cluster ARN>"} - the code is a fixed token, the message's fixed text is "The specified Amazon MSK Cluster does not exist. Cluster ARN: " and its one variable part is the cluster ARN from the request. Separately, MSK DERIVES each entry's `kafkaClusterAlias` from the cluster name truncated and suffixed with the ARN's trailing uuid segment, so the alias is not simply the cluster name, and that derived alias is what `replicationInfoList[].sourceKafkaClusterAlias` / `targetKafkaClusterAlias` carry.
Emulator: `replicatorState` RUNNING, `stateInfo` absent entirely, and `kafkaClusterAlias` set to the raw cluster name (so the source/target aliases disagree with AWS's). Several replicationInfoList sub-members (consumerGroupReplication.consumerGroupOffsetSyncMode / consumerGroupsToExclude, topicReplication.startingPosition / topicNameConfiguration / topicsToExclude) also diverge in the same projection.
Why: a client waiting for a replicator to converge waits forever on RUNNING and never sees the failure reason MSK supplies, so a misconfigured replication flow looks healthy; and the alias is the key other replication APIs are addressed by, so a wrong alias makes those calls unaddressable.
Fix: ops/describe_replicator.rs plus the replicator state machine in the service's state module - converge to FAILED with the modeled stateInfo when a referenced MSK cluster does not resolve, and derive kafkaClusterAlias the way MSK does, in the one place both the create and describe projections use.
Traces: kafka-create-replicator-nonexistent-clusters#task#3.

Cause 4 of 4 - the same projection is wrong on the create/describe/delete lifecycle, and additionally drops boolean and tag members.
AWS: as Cause 3 - FAILED with stateInfo {code: 'InvalidInput.InvalidKafkaCluster', message: "The specified Amazon MSK Cluster does not exist. Cluster ARN: <arn>"}, derived kafkaClusterAlias, and `vpcConfig.securityGroupIds` present (an empty list when none were given) rather than omitted.
Emulator: RUNNING, no stateInfo, raw-name aliases, `vpcConfig.securityGroupIds` omitted instead of empty, and divergences in replicationInfoList[].consumerGroupReplication.detectAndCopyNewConsumerGroups / synchroniseConsumerGroupOffsets, topicReplication.copyAccessControlListsForTopics / copyTopicConfigurations / detectAndCopyNewTopics / startingPosition / topicNameConfiguration / topicsToExclude, and `tags`.
Why: listed as its own cause because it is a different fixture path exercising the full lifecycle, and it shows additional members of the same projection (the replication booleans, securityGroupIds and tags) are unpopulated - fixing only the state/alias leaves those wrong.
Fix: ops/describe_replicator.rs - populate every modeled replicationInfoList member and vpcConfig.securityGroupIds from the stored record, emitting empty lists rather than omitting them.
Traces: kafka-replicator-create-describe-delete-lifecycle#task#4.

Neighbouring filing: the resolved issue 20261024T221708Z-kafka-a-running-msk-replicator-replicates-nothing-a-source.md concerns a RUNNING replicator not moving data; it does not cover the FAILED convergence, the stateInfo payload or the alias derivation asserted here, and it carries no wire fixture#step key.

## Evidence
Corpus: simparity-wire-aws-v1. All four clusters verdict=mismatch with expected_status == actual_status == 200.

Cause 1 - kafka-catalog-get-compatible-kafka-versions#task#1, GetCompatibleKafkaVersions: 38 diverging indices of $.compatibleKafkaVersions, e.g.
  [0]: expected {'sourceVersion': '3.9.x', 'targetVersions': ['3.9.x.kraft']}, got {'sourceVersion': '1.1.1', 'targetVersions': ['2.8.1','2.8.2.tiered','3.1.1','3.2.0','3.3.1','3.3.2','3.4.0','3.5.1','3.6.0','3.7.x','3.8.x','3.9.x']}
  [1]: expected {'sourceVersion': '3.9.x.kraft', 'targetVersions': ['4.1.x.kraft','4.0.x.kraft']}, got sourceVersion '2.1.0' with the same blanket target list
  [3]: expected {'sourceVersion': '3.8.link', 'targetVersions': []}, got sourceVersion '2.3.1' with the blanket list
  [4]: expected {'sourceVersion': '3.7.x.kraft', 'targetVersions': ['4.1.x.kraft','3.8.x.kraft','3.9.x.kraft','4.0.x.kraft']}
  [5]: expected {'sourceVersion': '2.4.1.1', 'targetVersions': ['3.6.0','3.7.x','3.8.x','3.9.x']}, got the SAME sourceVersion with the blanket 12-entry list - i.e. the source matches but the permitted targets do not

Cause 2 - kafka-catalog-list-kafka-versions#task#1, ListKafkaVersions: 18 diverging indices of $.kafkaVersions, e.g.
  [15]-[22]: expected {'version': '2.8.1'...'3.5.1', 'status': 'DEPRECATED'}, got the same versions with 'ACTIVE'
  [24]: expected {'version': '3.6.0.1', 'status': 'DEPRECATED'}, got {'version': '3.7.x', 'status': 'ACTIVE'}
  [26]: expected {'version': '3.7.x.kraft', 'status': 'ACTIVE'}, got {'version': '3.9.x', 'status': 'ACTIVE'}
  [28],[29],[31],[32],[33],[34],[35]: expected 3.8.x.kraft/ACTIVE, 3.8.link/DEPRECATED, 3.9.x.kraft/ACTIVE, 3.9.x/DEPRECATED, 4.0.x.kraft/ACTIVE, 4.1.x.kraft/ACTIVE, 4.2.x.kraft/DEPRECATED - got None (absent)

Cause 3 - kafka-create-replicator-nonexistent-clusters#task#3, DescribeReplicator:
  $.replicatorState: expected 'FAILED', got 'RUNNING'
  $.stateInfo: expected {'code': 'InvalidInput.InvalidKafkaCluster', 'message': 'The specified Amazon MSK Cluster does not exist. Cluster ARN: <src cluster arn>'}, got None
  $.kafkaClusters[0]: expected kafkaClusterAlias 'sim2real-probe-kafka-src-clu-749b26b604d5' (name truncated + ARN uuid tail), got 'sim2real-probe-kafka-src-cluster-dd40c9de' (raw name)
  $.kafkaClusters[1]: same, expected 'sim2real-probe-kafka-tgt-clu-2dbff207b95b', got the raw name
  also diverging: $.replicationInfoList[0].sourceKafkaClusterAlias, .targetKafkaClusterAlias, .consumerGroupReplication.consumerGroupOffsetSyncMode, .consumerGroupReplication.consumerGroupsToExclude, .topicReplication.startingPosition, .topicReplication.topicNameConfiguration, .topicReplication.topicsToExclude, $.creationTime

Cause 4 - kafka-replicator-create-describe-delete-lifecycle#task#4, DescribeReplicator:
  $.replicatorState: expected 'FAILED', got 'RUNNING'
  $.stateInfo: expected {'code': 'InvalidInput.InvalidKafkaCluster', 'message': 'The specified Amazon MSK Cluster does not exist. Cluster ARN: <lc-src cluster arn>'}, got None
  $.kafkaClusters[0]/[1]: expected kafkaClusterAlias 'sim2real-probe-kafka-lc-src--000000000000' / 'sim2real-probe-kafka-lc-tgt--000000000000' and vpcConfig.securityGroupIds: [], got the raw names and securityGroupIds omitted
  $.replicationInfoList[0].sourceKafkaClusterAlias / .targetKafkaClusterAlias diverge accordingly
  also diverging: consumerGroupReplication.detectAndCopyNewConsumerGroups, .synchroniseConsumerGroupOffsets, topicReplication.copyAccessControlListsForTopics, .copyTopicConfigurations, .detectAndCopyNewTopics, .startingPosition, .topicNameConfiguration, .topicsToExclude, $.tags, $.creationTime

Smithy cited: kafka#GetCompatibleKafkaVersions GET /v1/compatible-kafka-versions, input ClusterArn httpQuery 'clusterArn', output CompatibleKafkaVersions jsonName 'compatibleKafkaVersions'; kafka#ListKafkaVersions GET /v1/kafka-versions, output KafkaVersions jsonName 'kafkaVersions'; kafka#ReplicatorState enum values [RUNNING, CREATING, UPDATING, DELETING, FAILED]; kafka#ReplicationStateInfo members Code (jsonName 'code') and Message (jsonName 'message'). (The concrete cluster names, uuids and epoch creationTime values above are one run's fillings, not part of the requirement; Causes 3/4 also show the epoch-timestamp defect filed separately for the configuration surface.)

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: replay kafka-catalog-get-compatible-kafka-versions#task#1 - GetCompatibleKafkaVersions must return a per-source upgrade graph covering the KRaft and tiered variants, including at least one source whose targetVersions is EMPTY, and must not return one blanket target list shared by every source.
3. Cause 2: kafka-catalog-list-kafka-versions#task#1 - ListKafkaVersions must include the KRaft and .link entries and must report DEPRECATED for the superseded lines, not ACTIVE for everything. Assert the catalog it serves is the same one step 2 derives from.
4. Cause 3: kafka-create-replicator-nonexistent-clusters#task#3 - create a replicator over MSK cluster ARNs that do not exist, then DescribeReplicator: replicatorState must converge to FAILED with stateInfo {code 'InvalidInput.InvalidKafkaCluster', message 'The specified Amazon MSK Cluster does not exist. Cluster ARN: <that ARN>'}, and each kafkaClusters[].kafkaClusterAlias must be the derived alias (truncated name + the ARN's trailing uuid segment), matching replicationInfoList[].sourceKafkaClusterAlias / targetKafkaClusterAlias.
5. Cause 4: kafka-replicator-create-describe-delete-lifecycle#task#4 - the same assertions, plus vpcConfig.securityGroupIds present as an empty list and every replicationInfoList consumerGroupReplication / topicReplication member and `tags` populated from the stored record.
6. Re-replay all four fixture#step bindings.
