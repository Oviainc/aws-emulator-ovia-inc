Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-create-unsupported-kafka-version-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-create-unsupported-kafka-version-rejected|run#6|AssertionError|assert message == _UNSUPPORTED_VERSION_MESSAGE, message
Issue-Commit: 6e9861e2e36e09c43d2851607a7d2371f7543aa8

# CreateClusterV2's unsupported-KafkaVersion refusal offers versions MSK no longer supports for standard brokers (3.6.0, 3.7.x, 3.7.x.kraft), and the same set is accepted

> _Note: this issue was identified against the code base at commit `6e9861e2e36e09c43d2851607a7d2371f7543aa8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateClusterV2

## Description
ONE cause.

Cause 1 of 1 — the valid-value list in CreateClusterV2's unsupported-KafkaVersion refusal names three Apache Kafka versions MSK does not offer for a new standard-broker (non-Express) provisioned cluster, and because the same catalogue backs the acceptance gate, a create naming one of them is ACCEPTED instead of refused.
Failing check: kafka.cluster-create-unsupported-kafka-version-rejected.

AWS: Smithy kafka#CreateClusterV2 (POST /api/v2/clusters, success 200; input Provisioned -> ProvisionedRequest, jsonName 'provisioned', which carries KafkaVersion and BrokerNodeGroupInfo$InstanceType; error BadRequestException with errorKind client, httpError 400). The model puts no enum on KafkaVersion and fixes no message text, so for the message: recorded behavior only — MSK answers HTTP 400 / Error.Code 'BadRequestException' with Error.Message "<broker category> instance types are not supported for Kafka version <rejected version>. Valid values: [<the versions MSK offers that category for a NEW cluster, comma-separated>]". FIXED text: " instance types are not supported for Kafka version ", ". Valid values: [" and the closing "]". VARIABLE parts: the broker category, derived from BrokerNodeGroupInfo$InstanceType ("Standard" for the kafka.* families, "Express" for express.*); the rejected version string, taken verbatim from the request's Provisioned$KafkaVersion; and the version list, taken from service state — the set MSK currently offers that broker category, which excludes versions it has retired for new clusters. In the recording, a Standard request for version 9.9.9 is answered with the list [3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft] — that six-entry sequence is an example filling the variable part, not part of the template. The status (400) and the Error.Code ('BadRequestException') are already correct, and so is the fixed text; what diverges is the MEMBERSHIP of that list, and the admission gate behind it.

Emulator: the same request is refused 400/BadRequestException with Error.Message "Standard instance types are not supported for Kafka version 9.9.9. Valid values: [3.6.0, 3.7.x, 3.7.x.kraft, 3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]" — a superset carrying three extra entries (3.6.0, 3.7.x, 3.7.x.kraft) that MSK does not offer a new standard-broker cluster. The list is computed from the shared catalogue, filtered to entries whose status is the catalogue's ACTIVE marker, so the divergence is that the catalogue still publishes those three versions as offered. The same catalogue is the admission gate (the create is admitted when the version is present in the catalogue at all), so CreateClusterV2 with KafkaVersion='3.7.x' and a kafka.m5.large broker is ACCEPTED and mints a CREATING cluster where AWS refuses it with this very message. Note the two neighbouring open issues, neither of which covers this: 20261103T090515Z-kafka-createconfiguration-s-unsupported-kafkaversion-refusal-renders-the-wrong.md is about a DIFFERENT message (CreateConfiguration's "Unsupported KafkaVersion [...]" template, its ORDER and a '3.8.link' entry) emitted by a different helper, and 20261030T072234Z-kafka-msk-version-catalog-is-stale-listkafkaversions-and-getcompatiblekafkaversions.md is a wire finding about the catalogue as served by ListKafkaVersions/GetCompatibleKafkaVersions; neither names CreateClusterV2's standard-vs-Express refusal nor the per-category offered set this message and this gate must use.

Why: a client that reads the retryable versions out of this refusal — the only place the refusal offers them — retries with 3.7.x (or 3.6.0) and succeeds against the emulator, then gets a 400 BadRequestException from real MSK for a version the emulator advertised and accepted. Conversely, CI that pins a cluster to a version the emulator admits silently diverges from production: the cluster exists in the emulator and cannot be created in AWS at all.

Fix: the per-broker-category offered-version set that CreateClusterV2's version admission and its refusal message read — crate::state::create_kafka_version_violation and the offered/ACTIVE marking of the shared Apache Kafka version catalogue it filters (services/service-com-amazonaws-kafka-kafka/src/state.rs). Both halves must move together: the set the message advertises as retryable has to be exactly the set the gate admits for a NEW cluster of that broker category, and that set must exclude versions MSK has retired for new clusters while the catalogue may still publish them for existing ones.

Traces: kafka.cluster-create-unsupported-kafka-version-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1 (kafka 113/215 passed). Failing check [mismatch] kafka.cluster-create-unsupported-kafka-version-rejected, at check_kafka_cluster_create_unsupported_kafka_version_rejected.py line 83 — the assertion on Error.Code 'BadRequestException' (line 81) PASSED, so only Error.Message diverged.

Request replayed (subnets/security group from the run's shared network, two subnets in distinct AZs):
  kafka.create_cluster_v2(
      ClusterName=<unique>,
      Provisioned={'BrokerNodeGroupInfo': {'InstanceType': 'kafka.m5.large', 'ClientSubnets': [<2 subnets>], 'StorageInfo': {'EbsStorageInfo': {'VolumeSize': 10}}, 'SecurityGroups': [<sg>]},
                   'KafkaVersion': '9.9.9', 'NumberOfBrokerNodes': 2})

expected Error.Message:
  'Standard instance types are not supported for Kafka version 9.9.9. Valid values: [3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]'
actual   (AssertionError):
  'Standard instance types are not supported for Kafka version 9.9.9. Valid values: [3.6.0, 3.7.x, 3.7.x.kraft, 3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]'

Fixed text (identical in both, so not at fault): '<category> instance types are not supported for Kafka version ', '. Valid values: [', ']'. Variable parts: the category ('Standard', from InstanceType 'kafka.m5.large'), the rejected version ('9.9.9', verbatim from the request) and the offered-version list (from service state). The divergence is exactly the membership of that last part: three extra entries, 3.6.0, 3.7.x and 3.7.x.kraft.

Code: services/service-com-amazonaws-kafka-kafka/src/ops/create_cluster_v2.rs lines 403-414 call crate::state::create_kafka_version_violation(&request.kafka_version, &request.broker_node_group_info.instance_type) and raise it as BadRequestException with invalidParameter 'kafkaVersion'; services/service-com-amazonaws-kafka-kafka/src/state.rs lines 1022-1037 admit the version on catalogue MEMBERSHIP (kafka_version_index, lines 930-934) and build the list from every catalogue entry for which is_offered_kafka_version (lines 974-976) holds — i.e. status == KAFKA_VERSION_ACTIVE — with no per-broker-category narrowing, which is why 3.6.0 / 3.7.x / 3.7.x.kraft appear and are also accepted.

Smithy cited: kafka#CreateClusterV2 — POST /api/v2/clusters, code 200; Provisioned -> ProvisionedRequest (jsonName 'provisioned'); errors include BadRequestException (errorKind client, httpError 400). No member carries an enum of Apache Kafka versions and nothing in the model fixes this message, hence 'recorded behavior only' for the list's membership.

## Steps to Reproduce
1. Boot the emulator from this checkout (the kafka service answers; bootstrap-kafka is resolved).
2. Discover a VPC with two subnets in distinct AZs plus the VPC's default security group.
3. kafka.create_cluster_v2(ClusterName='badver-cluster', Provisioned={'BrokerNodeGroupInfo': {'InstanceType': 'kafka.m5.large', 'ClientSubnets': [<2 subnets>], 'StorageInfo': {'EbsStorageInfo': {'VolumeSize': 10}}, 'SecurityGroups': [<sg>]}, 'KafkaVersion': '9.9.9', 'NumberOfBrokerNodes': 2}) -> HTTP 400 / Error.Code 'BadRequestException' (both already correct). Inspect Error.Message: the 'Valid values: [...]' list must contain only the versions MSK offers a new Standard-broker cluster; observe the extra entries '3.6.0', '3.7.x' and '3.7.x.kraft' at the head of the list.
4. Over-permissive cross-check of the same cause: repeat step 3 with KafkaVersion='3.7.x' (same Standard instance type). The emulator ACCEPTS it and returns 200 with State 'CREATING'; real MSK refuses it with the step-3 message. kafka.list_clusters_v2(ClusterNameFilter='badver-cluster') then shows the cluster the emulator minted.
5. kafka.list_clusters_v2(ClusterNameFilter='badver-cluster') after step 3 -> must be [] (this already holds; the rejected create leaves nothing behind).
6. Re-replay kafka.cluster-create-unsupported-kafka-version-rejected.
