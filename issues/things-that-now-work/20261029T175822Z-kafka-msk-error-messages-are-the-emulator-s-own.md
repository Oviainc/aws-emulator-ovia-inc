Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.cluster-describe-not-found kafka.configuration-duplicate-name-rejected kafka.configuration-create-describe-lifecycle kafka.replicator-duplicate-name-conflict kafka.replicator-same-source-target-rejected kafka.scram-associate-invalid-arn-rejected kafka.tag-resource-reserved-key-rejected kafka.configuration-unsupported-kafka-version-rejected kafka.list-configurations-max-results-above-limit-rejected kafka.bootstrap-brokers-availability-gate
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.cluster-describe-not-found|run#4|AssertionError|assert message == _NOT_FOUND_MESSAGE, message
Issue-Key: kafka.configuration-duplicate-name-rejected|run#10|AssertionError|assert message == "A resource with this name already exists.", message
Issue-Key: kafka.configuration-create-describe-lifecycle|run#16|AssertionError|assert message == "A resource with this name already exists.", message
Issue-Key: kafka.replicator-duplicate-name-conflict|run#14|AssertionError|assert dup_message == expected_message, (
Issue-Key: kafka.replicator-same-source-target-rejected|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: kafka.scram-associate-invalid-arn-rejected|run#5|AssertionError|assert malformed == ("BadRequestException", "The provided secret ARN is invalid."), (
Issue-Key: kafka.tag-resource-reserved-key-rejected|run#8|AssertionError|assert err == expected, f"expected {expected!r}, got {err!r}"
Issue-Key: kafka.configuration-unsupported-kafka-version-rejected|run#11|AssertionError|assert message == f"Unsupported KafkaVersion [{_BOGUS_VERSION}]. Valid values: [{_VALID_VALUES}]", (
Issue-Key: kafka|ListConfigurations|BadRequestException
Issue-Key: kafka.bootstrap-brokers-availability-gate|run#7|AssertionError|assert early == (
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# MSK error Messages are the emulator's own prose instead of AWS's fixed text (9 causes)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterV2

## Description
NINE distinct root causes of one kind: the emulator picks the right HTTP status and the right Error.Code but writes its own explanatory sentence into Error.Message instead of the fixed text Amazon MSK uses for that error class. The message is part of the contract (clients log it, match on it, and surface it to operators), so each of these is a divergence in its own right. All nine must be closed.

NOTE on neighbouring filings: the open issue 20261022T171242Z-kafka-msk-control-plane-operations-answer-http-501-notimplemented.md lists kafka.cluster-describe-not-found among its trace ids, but its replay key for that check is the `assert code == "NotFoundException"` line (the 501 era). The emulator now returns the right code and fails one assertion later, on the Message, so Cause 1 below is NOT covered by it. Likewise the fixed stub issues for DescribeClusterV2 / CreateConfiguration / CreateReplicator / TagResource / ListConfigurations / BatchAssociateScramSecret stood the operations up; none of them pinned the refusal text.

Cause 1 of 9 - DescribeClusterV2/DescribeCluster not-found Message.
AWS: Smithy kafka#DescribeCluster and kafka#DescribeClusterV2 model NotFoundException (httpError 404). For a syntactically well-formed cluster ARN that was never issued, the recorded response is 404 / Error.Code 'NotFoundException' with Error.Message exactly "The requested resource doesn\u2019t exist." - the whole string is FIXED text (note the apostrophe is U+2019, not ASCII '); no part of it is request-derived, and in particular the ARN and the region are NOT echoed.
Emulator: 404 / 'NotFoundException' with a self-composed, request-derived sentence of the form "No cluster exists in <region> with the ARN <clusterArn>".
Why: a client that compares or displays the MSK refusal text gets a string AWS never emits, and a client that greps for the known MSK phrase sees no match; the emulator also leaks the request's ARN/region into a message AWS keeps constant.
Fix: the not-found refusal in services/service-com-amazonaws-kafka-kafka/src/ops/describe_cluster_v2.rs (and describe_cluster.rs) - or, better, the one shared constructor for the service's NotFoundException so all MSK not-found refusals carry the single fixed string.
Traces: kafka.cluster-describe-not-found.

Cause 2 of 9 - CreateConfiguration duplicate-name Message.
AWS: Smithy kafka#CreateConfiguration (POST /v1/configurations) models ConflictException (409). Creating a configuration whose Name is already taken is refused with Error.Message exactly "A resource with this name already exists." - entirely fixed text; the offending name and the existing ARN are NOT in the message.
Emulator: a message it composes itself, "A configuration with the name '<name>' already exists: <existing arn>" - i.e. fixed text replaced by prose plus two request/state-derived values AWS does not include.
Why: idempotent create-or-reuse code that recognises the duplicate by MSK's message fails to recognise it; the emulator additionally discloses the existing resource's ARN in an error AWS keeps opaque.
Fix: the name-uniqueness conflict path in services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs.
Traces: kafka.configuration-duplicate-name-rejected, kafka.configuration-create-describe-lifecycle.

Cause 3 of 9 - CreateReplicator duplicate-name Message.
AWS: Smithy kafka#CreateReplicator (POST /replication/v1/replicators) models ConflictException (409). A ReplicatorName that is already taken is refused with the SAME fixed service-wide text as Cause 2: "A resource with this name already exists."
Emulator: "A replicator with the name '<name>' already exists: <existing replicator arn>".
Why: same as Cause 2, and it shows the duplicate-name refusal text is being written per resource type instead of once for the service.
Fix: the name-uniqueness conflict path in services/service-com-amazonaws-kafka-kafka/src/ops/create_replicator.rs; share the constructor with create_configuration.rs.
Traces: kafka.replicator-duplicate-name-conflict.

Cause 4 of 9 - CreateReplicator duplicate-cluster-ARN Message.
AWS: CreateReplicator models BadRequestException (400). When the KafkaClusters list / ReplicationInfoList name the same Kafka cluster as both source and target, the refusal Message is exactly "Kafka cluster list contains duplicate cluster ARNs. Please ensure all the cluster ARNs are unique." - all fixed text; the duplicated ARN is not named. The rule AWS states is about the cluster ARN list being a set, not about a replication flow's endpoints.
Emulator: "The source and target of a replication flow must be different Kafka clusters; both resolve to the alias '<cluster name>'." - different wording AND a different rule (it reports the collision in terms of a resolved cluster alias).
Why: the client is told the wrong thing about which input it must change, and the text does not match MSK's.
Fix: the duplicate-cluster validation in services/service-com-amazonaws-kafka-kafka/src/ops/create_replicator.rs - validate the KafkaClusters ARN list for uniqueness and emit the fixed message.
Traces: kafka.replicator-same-source-target-rejected.

Cause 5 of 9 - BatchAssociateScramSecret invalid-secret-ARN Message.
AWS: Smithy kafka#BatchAssociateScramSecret (POST /v1/clusters/{ClusterArn}/scram-secrets) models BadRequestException (400). A SecretArnList entry that is not a Secrets Manager secret ARN at all is refused 400 / 'BadRequestException' with Error.Message exactly "The provided secret ARN is invalid." - all fixed text; the offending string is NOT echoed.
Emulator: 400 / 'BadRequestException' with "<the offending string> is not a valid AWS Secrets Manager secret ARN." - request-derived prose instead of the fixed sentence.
Why: callers that classify SCRAM association failures by MSK's message cannot; and the emulator echoes caller input into an error AWS keeps constant.
Fix: the SecretArnList shape validation in services/service-com-amazonaws-kafka-kafka/src/ops/batch_associate_scram_secret.rs. (The open issues 20261027T231320Z-kafka-scram-secret-operations-are-over-permissive-no.md and 20261029T072939Z-kafka-scram-secret-refusals-no-cluster-state-gate.md cover the per-secret UnprocessedScramSecrets behaviour and the cluster-state/SASL-SCRAM gates; neither pins this ARN-shape message.)
Traces: kafka.scram-associate-invalid-arn-rejected.

Cause 6 of 9 - TagResource reserved-prefix Message.
AWS: Smithy kafka#TagResource (POST /v1/tags/{ResourceArn}, success 204) models BadRequestException (400). A tag key beginning with the reserved prefix `aws:` is refused with Error.Message exactly "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)" - all fixed text; the offending key is NOT named, and the parenthetical about case-insensitivity is part of the string.
Emulator: "Tag key '<key>' starts with 'aws:', a prefix reserved for use by AWS."
Why: wrong text, and it drops the information AWS does convey (that the check is case-insensitive).
Fix: the tag-key validation in services/service-com-amazonaws-kafka-kafka/src/ops/tag_resource.rs (and wherever CreateCluster*/CreateReplicator Tags share it).
Traces: kafka.tag-resource-reserved-key-rejected.

Cause 7 of 9 - CreateConfiguration unsupported-KafkaVersions Message.
AWS: CreateConfiguration models BadRequestException (400). A KafkaVersions entry MSK does not offer is refused 400 / 'BadRequestException' with Message of the form "Unsupported KafkaVersion [<the rejected version>]. Valid values: [<comma-separated list of the versions the service offers, in the service's own order>]". Fixed text: "Unsupported KafkaVersion [", "]. Valid values: [", "]". Variable parts: the rejected version string, taken verbatim from the request, and the service's current version list, taken from service state (the recorded run's list is one example of that list, not part of the template).
Emulator: "Amazon MSK supports none of these Apache Kafka versions: '<version>'." - different sentence, and it omits the valid-value list entirely.
Why: the valid-value list is the actionable part of this refusal - a caller that discovers supported versions from the error text gets nothing, and the plural phrasing misreports a single-entry rejection.
Fix: the KafkaVersions validation in services/service-com-amazonaws-kafka-kafka/src/ops/create_configuration.rs - name the first offending version and render the supported-version list from the same source ListKafkaVersions serves.
Traces: kafka.configuration-unsupported-kafka-version-rejected.

Cause 8 of 9 - ListConfigurations out-of-range MaxResults Message (a generic constraint-violation message leaks where MSK uses its own).
AWS: Smithy kafka#ListConfigurations binds MaxResults to httpQuery 'maxResults' with @range min 1 max 100. A value above the range is refused 400 / Error.Code 'BadRequestException' with Message exactly "One or more of the parameters are not valid." - all fixed text; MSK does NOT name the member or the limit.
Emulator: 400 / 'BadRequestException' with the AWS-framework constraint report "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Why: the emulator is emitting a different service family's validation envelope for an MSK operation; a client keying on MSK's opaque parameter message, or on the absence of a member name, diverges. The same fixed message is MSK's generic parameter refusal elsewhere (e.g. an ARN whose region segment is not a region), so getting it wrong here is systemic.
Fix: the query-parameter range validation for services/service-com-amazonaws-kafka-kafka/src/ops/list_configurations.rs - and the shared range/constraint rejection helper it calls, so it renders MSK's message rather than the generic one.
Traces: kafka.list-configurations-max-results-above-limit-rejected.

Cause 9 of 9 - GetBootstrapBrokers CREATING-state refusal Message.
AWS: Smithy kafka#GetBootstrapBrokers (GET /v1/clusters/{ClusterArn}/bootstrap-brokers) models BadRequestException (400). Called inside the provisioning window, while the cluster is still CREATING, it is refused 400 / 'BadRequestException' with Message exactly "You can't get bootstrap broker nodes for a cluster in CREATING state." - fixed text apart from the state name, which comes from the cluster's current state.
Emulator: 400 / 'BadRequestException' with "You can't get bootstrap brokers for a cluster that is in the CREATING state." - a reworded variant ("bootstrap brokers" vs "bootstrap broker nodes", "for a cluster that is in the CREATING state" vs "for a cluster in CREATING state").
Why: the code and status are right, so this is exactly the case a client cannot defend against: it matches MSK's phrase and silently stops matching. The same reworded variant also shows up in the cluster-state refusal set, so the string is wrong in one place and reused.
Fix: the cluster-state gate in services/service-com-amazonaws-kafka-kafka/src/ops/get_bootstrap_brokers.rs.
Traces: kafka.bootstrap-brokers-availability-gate.

## Evidence
Suite: simparity-behavioral-aws-v1. kafka 44/215 passed.

Cause 1 - kafka.cluster-describe-not-found (mismatch):
  describe_cluster_v2(ClusterArn='arn:aws:kafka:us-west-2:<acct>:cluster/<never-issued-name>/00000000-0000-0000-0000-000000000000-1')
  expected Error.Message == "The requested resource doesn\u2019t exist."
  actual   AssertionError: No cluster exists in us-west-2 with the ARN arn:aws:kafka:us-west-2:000000000000:cluster/sim2real-behav-nonexistent-<rand>/00000000-0000-0000-0000-000000000000-1

Cause 2 - kafka.configuration-duplicate-name-rejected and kafka.configuration-create-describe-lifecycle (mismatch):
  create_configuration(Name=<already used>, ...)
  expected 'A resource with this name already exists.'
  actual   "A configuration with the name 'sim2real-msk-cfg-dup-<rand>' already exists: arn:aws:kafka:us-west-2:000000000000:configuration/<name>/<uuid>-1"

Cause 3 - kafka.replicator-duplicate-name-conflict (mismatch):
  create_replicator(ReplicatorName=<already used>, ...)
  expected 'A resource with this name already exists.'
  actual   "A replicator with the name 'sim2real-behav-rep-dup-<rand>' already exists: arn:aws:kafka:us-west-2:000000000000:replicator/<name>/<uuid>-1"

Cause 4 - kafka.replicator-same-source-target-rejected (mismatch):
  expected 'Kafka cluster list contains duplicate cluster ARNs. Please ensure all the cluster ARNs are unique.'
  actual   "The source and target of a replication flow must be different Kafka clusters; both resolve to the alias 'sim2real-behav-ghost-cluster-<rand>'."

Cause 5 - kafka.scram-associate-invalid-arn-rejected (mismatch):
  batch_associate_scram_secret(ClusterArn=<cluster>, SecretArnList=['not-a-secrets-manager-arn'])
  expected ('BadRequestException', 'The provided secret ARN is invalid.')
  actual   ('BadRequestException', 'not-a-secrets-manager-arn is not a valid AWS Secrets Manager secret ARN.')

Cause 6 - kafka.tag-resource-reserved-key-rejected (mismatch):
  tag_resource(ResourceArn=<cluster arn>, Tags={'aws:reserved': ...})
  expected ('BadRequestException', 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)')
  actual   ('BadRequestException', "Tag key 'aws:reserved' starts with 'aws:', a prefix reserved for use by AWS.")

Cause 7 - kafka.configuration-unsupported-kafka-version-rejected (mismatch):
  create_configuration(Name=<new>, KafkaVersions=['9.9.9'], ServerProperties=b'num.partitions=1\n')
  expected 'Unsupported KafkaVersion [9.9.9]. Valid values: [<the service's version list>]'
  actual   "Amazon MSK supports none of these Apache Kafka versions: '9.9.9'."

Cause 8 - kafka.list-configurations-max-results-above-limit-rejected (mismatch):
  list_configurations(MaxResults=101)
  expected ('BadRequestException', 'One or more of the parameters are not valid.')
  actual   ('BadRequestException', "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100")

Cause 9 - kafka.bootstrap-brokers-availability-gate (mismatch):
  create_cluster_v2(...) -> State CREATING, then get_bootstrap_brokers(ClusterArn=<that cluster>)
  expected ('BadRequestException', "You can't get bootstrap broker nodes for a cluster in CREATING state.")
  actual   AssertionError: expected the provisioning-window refusal, got ('BadRequestException', "You can't get bootstrap brokers for a cluster that is in the CREATING state.")

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: kafka.describe_cluster_v2(ClusterArn='arn:aws:kafka:<region>:<account>:cluster/never-issued/00000000-0000-0000-0000-000000000000-1') -> expect 404 NotFoundException with Message exactly "The requested resource doesn\u2019t exist." (U+2019 apostrophe).
3. Cause 2: kafka.create_configuration(Name='dup', ServerProperties=b'auto.create.topics.enable=true\n'); repeat the identical call -> expect 409 ConflictException, Message 'A resource with this name already exists.'
4. Cause 3: kafka.create_replicator(ReplicatorName='dup-rep', ...) twice -> expect 409 ConflictException, Message 'A resource with this name already exists.'
5. Cause 4: kafka.create_replicator with the same MSK cluster ARN as both the source and the target entry -> expect 400 BadRequestException, Message 'Kafka cluster list contains duplicate cluster ARNs. Please ensure all the cluster ARNs are unique.'
6. Cause 5: kafka.batch_associate_scram_secret(ClusterArn=<any>, SecretArnList=['not-a-secrets-manager-arn']) -> expect 400 BadRequestException, Message 'The provided secret ARN is invalid.'
7. Cause 6: kafka.tag_resource(ResourceArn=<cluster arn>, Tags={'aws:reserved':'x'}) -> expect 400 BadRequestException, Message 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'
8. Cause 7: kafka.create_configuration(Name='badver', KafkaVersions=['9.9.9'], ServerProperties=b'num.partitions=1\n') -> expect 400 BadRequestException, Message 'Unsupported KafkaVersion [9.9.9]. Valid values: [<versions ListKafkaVersions reports>]'
9. Cause 8: kafka.list_configurations(MaxResults=101) -> expect 400 BadRequestException, Message 'One or more of the parameters are not valid.' (no member name, no limit).
10. Cause 9: create a provisioned cluster, then immediately kafka.get_bootstrap_brokers on it while it is CREATING -> expect 400 BadRequestException, Message "You can't get bootstrap broker nodes for a cluster in CREATING state."
11. Re-replay every check id in trace_ids.
