Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-create-invalid-broker-config-rejected kafka.cluster-create-invalid-instance-type-rejected kafka.cluster-create-subnets-same-az-rejected kafka.cluster-create-unsupported-kafka-version-rejected kafka.cluster-create-v2-both-cluster-types-rejected kafka.broker-count-decrease-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-create-invalid-broker-config-rejected|_err#4|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: kafka.cluster-create-invalid-instance-type-rejected|_err#4|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: kafka.cluster-create-subnets-same-az-rejected|_err#4|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: kafka.cluster-create-unsupported-kafka-version-rejected|_err#4|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: kafka.cluster-create-v2-both-cluster-types-rejected|run#7|AssertionError|assert message == _ONE_CLUSTER_TYPE_MESSAGE, message
Issue-Key: kafka.broker-count-decrease-rejected|_err#5|AssertionError|raise AssertionError("the call was accepted; a ClientError refusal was expected")
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# CreateClusterV2 validates no provisioned-cluster input: four requests MSK refuses mint a CREATING cluster, the both-cluster-types message is wrong, and UpdateBrokerCount accepts a broker-count decrease (6 causes)

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateClusterV2

## Description
SIX distinct root causes on the kafka (MSK) cluster create/scale validation path. Each is a separate admissibility rule and must be closed separately; all six must be closed before this issue is submitted. Related open issue 20261029T202809Z-kafka-msk-create-and-update-inputs-aws-refuses-are.md ('MSK create and update inputs AWS refuses are accepted, and refusal order is wrong') is a wire-corpus bundle and carries none of the check ids below, so these six are uncovered.

Cause 1 of 6: NumberOfBrokerNodes that is not a multiple of the ClientSubnets' AZ count is accepted.
AWS: CreateClusterV2 (POST /api/v2/clusters, 200) models BadRequestException/400. MSK requires the broker count to be a multiple of the number of AZs spanned by BrokerNodeGroupInfo.ClientSubnets; 3 brokers across 2 subnets is refused with BadRequestException whose message is the fixed text 'The target number of broker nodes must be a multiple of the number of Availability Zones in the Client subnets parameter. For example, for a cluster with subnets in 2 Availability Zones, the target number of brokers must be a multiple of 2.' (no request value is interpolated), and no cluster is created.
Emulator: HTTP 200 with ClusterArn/ClusterName/State='CREATING'/ClusterType='PROVISIONED'; a cluster is minted and is then visible to ListClustersV2.
Why: a client is told an unbuildable topology was accepted and only discovers it when brokers never come up; negative tests for the AZ-multiple rule pass vacuously, and the rejected name is left occupied in state.
Fix: CreateClusterV2's handler validation, before any cluster record is written.
Traces: kafka.cluster-create-invalid-broker-config-rejected.

Cause 2 of 6: an unsupported BrokerNodeGroupInfo.InstanceType is accepted.
AWS: an InstanceType outside MSK's catalogue (e.g. 'kafka.t1.nano') is refused with BadRequestException and the message 'Unsupported InstanceType specified. Valid values: [' + the comma-separated list of supported instance types in MSK's order + ']' — the fixed prefix plus the service's own valid-value list (express.m7g.*, kafka.m5.*, kafka.m7g.*, kafka.t3.small); the offending value is named in the prefix sentence only as part of 'Unsupported InstanceType specified.'
Emulator: HTTP 200, cluster minted in CREATING with the bogus instance type.
Why: capacity-validation and guardrail tests pass against an emulator that would mint any instance type; a client gets no signal until the real deployment fails.
Fix: CreateClusterV2's handler validation against the service's instance-type catalogue (same catalogue the error message must render).
Traces: kafka.cluster-create-invalid-instance-type-rejected.

Cause 3 of 6: ClientSubnets containing the same subnet twice (so not spanning distinct AZs) is accepted.
AWS: MSK requires each client subnet to be in a distinct Availability Zone; a duplicated subnet id is refused with BadRequestException and the fixed message 'The list provided contains duplicate items.' and no cluster is created.
Emulator: HTTP 200, cluster minted in CREATING.
Why: a client builds a single-AZ 'multi-AZ' cluster in the emulator and is refused in AWS; durability assumptions silently differ between the two.
Fix: CreateClusterV2's handler validation of the ClientSubnets list (duplicate detection) before minting.
Traces: kafka.cluster-create-subnets-same-az-rejected.

Cause 4 of 6: an unsupported Provisioned.KafkaVersion is accepted.
AWS: a KafkaVersion MSK does not support (e.g. '9.9.9') is refused with BadRequestException whose message names the rejected version and the valid list for the requested broker category: 'Standard instance types are not supported for Kafka version <the KafkaVersion sent>. Valid values: [' + the supported versions for standard instance types + ']' — the connective text is fixed, the version is taken from the request and the list from the service catalogue.
Emulator: HTTP 200, cluster minted in CREATING with KafkaVersion '9.9.9'.
Why: a version typo is accepted and the cluster reports a version AWS would never run; upgrade/compat tooling reads a version the catalogue does not contain.
Fix: CreateClusterV2's handler validation against the version catalogue (the same catalogue ListKafkaVersions serves), before minting.
Traces: kafka.cluster-create-unsupported-kafka-version-rejected.

Cause 5 of 6: CreateClusterV2 with BOTH Provisioned and Serverless is correctly refused 400 BadRequestException but with the emulator's own sentence.
AWS: the two request members are mutually exclusive; sending both is refused with BadRequestException and the fixed message 'You can specify only one cluster type, either provisioned or serverless.' (entirely fixed text, no request value interpolated), and no cluster is created.
Emulator: BadRequestException with 'A create names one cluster type: Provisioned and Serverless cannot both be given.' — status and code right, message is emulator prose.
Why: the message is part of the contract; log scrapers, error tables and assertions keyed on MSK's sentence stop matching even though the class is right.
Fix: the mutual-exclusion refusal in CreateClusterV2's handler must raise the service's fixed message constant rather than formatting its own sentence.
Traces: kafka.cluster-create-v2-both-cluster-types-rejected.

Cause 6 of 6: UpdateBrokerCount accepts a target broker count LOWER than the current count.
AWS: MSK cannot scale a cluster's broker count down; UpdateBrokerCount with TargetNumberOfBrokerNodes below the current value is refused with BadRequestException and the fixed message "Amazon MSK cannot proceed with decreasing the cluster's broker count. Make sure that at least one broker in each Availability Zone has no user partitions and try again. Allow five minutes after partition reassignments." The cluster stays ACTIVE, NumberOfBrokerNodes is unchanged and CurrentVersion does not advance (no ClusterOperation is queued).
Emulator: the call is ACCEPTED (no ClientError raised), so a decrease is enqueued.
Why: a client that mistakenly scales down gets a successful operation in the emulator and a 400 in AWS; capacity-management code that relies on the refusal to protect a cluster loses that protection, and the cluster's version/broker count drift from what AWS would report.
Fix: the UpdateBrokerCount handler's target-count validation (reject any target below the cluster's current broker count) before the operation is minted.
Traces: kafka.broker-count-decrease-rejected.

## Evidence
All six checks are kafka behavioral checks with verdict mismatch; none is marked ALREADY FILED or OWNED.

Cause 1 — kafka.cluster-create-invalid-broker-config-rejected: NumberOfBrokerNodes=3 over 2 distinct-AZ subnets.
  Expected: ('BadRequestException', 'The target number of broker nodes must be a multiple of the number of Availability Zones in the Client subnets parameter. For example, for a cluster with subnets in 2 Availability Zones, the target number of brokers must be a multiple of 2.') and ListClustersV2(ClusterNameFilter=name) == [].
  Actual: HTTP 200 {'ClusterArn': 'arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-behav-badcfg-cluster-<id>/<uuid>-1', 'State': 'CREATING', 'ClusterType': 'PROVISIONED'} (names/ids are per-run).

Cause 2 — kafka.cluster-create-invalid-instance-type-rejected: InstanceType='kafka.t1.nano'.
  Expected: ('BadRequestException', 'Unsupported InstanceType specified. Valid values: [express.m7g.12xlarge, express.m7g.16xlarge, express.m7g.2xlarge, ... kafka.m7g.xlarge, kafka.t3.small]').
  Actual: HTTP 200, cluster minted State='CREATING'.

Cause 3 — kafka.cluster-create-subnets-same-az-rejected: the same subnet id twice in ClientSubnets.
  Expected: ('BadRequestException', 'The list provided contains duplicate items.').
  Actual: HTTP 200, cluster minted State='CREATING'.

Cause 4 — kafka.cluster-create-unsupported-kafka-version-rejected: KafkaVersion='9.9.9'.
  Expected: ('BadRequestException', 'Standard instance types are not supported for Kafka version 9.9.9. Valid values: [3.8.x, 3.8.x.kraft, 3.9.x, 3.9.x.kraft, 4.0.x.kraft, 4.1.x.kraft]') — '9.9.9' is the request's value filling the template.
  Actual: HTTP 200, cluster minted State='CREATING'.

Cause 5 — kafka.cluster-create-v2-both-cluster-types-rejected: Provisioned and Serverless both supplied.
  Expected message: 'You can specify only one cluster type, either provisioned or serverless.'
  Actual message: 'A create names one cluster type: Provisioned and Serverless cannot both be given.' (code BadRequestException matched).

Cause 6 — kafka.broker-count-decrease-rejected: 4-broker ACTIVE cluster, UpdateBrokerCount(TargetNumberOfBrokerNodes=2, CurrentVersion=<current>).
  Expected: ('BadRequestException', "Amazon MSK cannot proceed with decreasing the cluster's broker count. Make sure that at least one broker in each Availability Zone has no user partitions and try again. Allow five minutes after partition reassignments.") with State/NumberOfBrokerNodes/CurrentVersion unchanged.
  Actual: AssertionError 'the call was accepted; a ClientError refusal was expected'.

Smithy cited: CreateClusterV2 POST /api/v2/clusters code 200, errors include BadRequestException (client, httpError 400) and ConflictException/409; input members ClusterName (required, length 1-64), Provisioned (ProvisionedRequest), Serverless (ServerlessRequest) — both optional in the model, so the mutual-exclusion rule is the service's.

## Steps to Reproduce
1. Boot the emulator; discover two distinct-AZ subnets and a security group in the seeded default VPC.
2. Cause 1: CreateClusterV2 with Provisioned.NumberOfBrokerNodes=3 and two subnets. Assert 400 BadRequestException with the AZ-multiple sentence and that ListClustersV2(ClusterNameFilter=<name>) is empty afterwards.
3. Cause 2: same request with BrokerNodeGroupInfo.InstanceType='kafka.t1.nano' and a valid broker count. Assert 400 with 'Unsupported InstanceType specified. Valid values: [...]' and no cluster minted.
4. Cause 3: same request with ClientSubnets=[subnetA, subnetA]. Assert 400 with 'The list provided contains duplicate items.' and no cluster minted.
5. Cause 4: same request with KafkaVersion='9.9.9'. Assert 400 with 'Standard instance types are not supported for Kafka version 9.9.9. Valid values: [...]' and no cluster minted.
6. Cause 5: CreateClusterV2 with BOTH Provisioned and Serverless bodies. Assert 400 BadRequestException and Message exactly 'You can specify only one cluster type, either provisioned or serverless.'
7. Cause 6: create a 4-broker cluster across 2 subnets, wait for ACTIVE, then UpdateBrokerCount with TargetNumberOfBrokerNodes=2 and the cluster's CurrentVersion. Assert 400 BadRequestException with the decrease sentence, then DescribeClusterV2 and assert State=='ACTIVE', NumberOfBrokerNodes==4 and CurrentVersion unchanged. Then UpdateBrokerCount to 6 and assert it is accepted (positive control).
8. Replay kafka.cluster-create-invalid-broker-config-rejected, kafka.cluster-create-invalid-instance-type-rejected, kafka.cluster-create-subnets-same-az-rejected, kafka.cluster-create-unsupported-kafka-version-rejected, kafka.cluster-create-v2-both-cluster-types-rejected and kafka.broker-count-decrease-rejected.
