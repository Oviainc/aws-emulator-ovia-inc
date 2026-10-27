Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.e2e-replicator-cross-cluster-topic-sync-teardown
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.e2e-replicator-cross-cluster-topic-sync-teardown|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# A RUNNING MSK Replicator replicates nothing: a source topic never appears on the target cluster's ListTopics

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicator

## Description
ONE root cause in the kafka (Amazon MSK) replicator implementation.

Cause 1 of 1 — the replicator is a pure registry record; it never performs the replication it was created for, so a topic created on the source cluster is never visible on the target cluster.

AWS: Smithy kafka#CreateReplicator (POST /replication/v1/replicators) takes ReplicationInfoList[].TopicReplication.TopicsToReplicate, documented as "List of regular expression patterns indicating the topics to copy", and kafka#DescribeReplicator reports ReplicatorState from the enum CREATING | DELETING | FAILED | RUNNING | UPDATING. KafkaClusterDescription$KafkaClusterAlias is modeled as the alias replicated topic names are prefixed with. Recorded behavior: once a replicator between two SASL/IAM provisioned clusters settles in RUNNING, a topic created on the SOURCE cluster that matches TopicsToReplicate becomes visible on the TARGET cluster via kafka#ListTopics (GET /v1/clusters/{ClusterArn}/topics, HTTP 200) within the check's 900 s convergence budget — that cross-cluster visibility is the entire subject of the check. The model is silent on the propagation latency, so the requirement is the eventual appearance, not any particular delay.

Emulator: the replicator's CREATING -> RUNNING transition is emulated (services/service-com-amazonaws-kafka-kafka/src/state.rs, REPLICATOR_ACTIVATION_DELAY, Replicator::provisioned sets running_due), and DescribeReplicator duly reports RUNNING — but no replication is ever carried out. The topic registry is keyed by (cluster ARN, topic name) and the only insertion path is CreateTopic against the cluster the request addresses (state.rs register/topics_in/settled_topic_in); ListTopics reads only `topics_in(input.cluster_arn)` for the addressed cluster (src/ops/list_topics.rs). Nothing consults the replicator registry, no record is mirrored into the target cluster, and no alias-prefixed topic is ever minted. So ListTopics on the target cluster returns the target's own topics forever and the source topic can never appear. The check consequently never leaves its replication poll and the harness kills it: verdict `mismatch`, detail `Check timed out after 60.0s`. (Hypothesis, stated as such: the stall is attributed to the replication poll rather than to an earlier wait because every other wait in the check is on a state the emulator does settle in ~500 ms — cluster ACTIVE, replicator RUNNING, topic removal — whereas the cross-cluster topic visibility the poll waits on has no code path that could ever make it true.)

Why: a client that creates a replicator and then verifies replication — which is the only way to observe that a replicator works — hangs forever against the emulator instead of converging. Because DescribeReplicator reports RUNNING with no StateInfo error, the emulator actively asserts that replication is healthy while delivering none, so the client cannot distinguish "still converging" from "will never converge" and has no error to key on. Any integration test or pipeline that gates on the replicated topic existing on the target blocks until its own timeout. The permissive direction makes it worse than an outright refusal: a cross-cluster setup real MSK would have made work is reported as working and silently does nothing.

Fix: services/service-com-amazonaws-kafka-kafka/src/state.rs — the replicator record and the topic registry. A replicator that has settled RUNNING must make each of its ReplicationInfoList flows observable: when a topic of the flow's source cluster matches TopicsToReplicate, the target cluster's topic listing must report a corresponding topic (prefixed with the source cluster's KafkaClusterAlias when TopicReplication's detect-and-copy settings call for it), materialised as a clock-owed transition the way the other MSK state changes already are, and withdrawn again when the source topic is deleted or the replicator is deleted. The readers (src/ops/list_topics.rs, src/ops/describe_topic.rs) should answer from that one derivation rather than each growing its own replication rule.

Traces: kafka.e2e-replicator-cross-cluster-topic-sync-teardown.

## Evidence
Suite: simparity-behavioral-ga-prod-v1, 14/85 passed; kafka 14/84, 70 failed. Every other kafka failure in this run was already filed (blocked by an unimplemented scaffold stub) or OWNED by an open issue; this is the only surviving failure.

Failing check: [mismatch] kafka.e2e-replicator-cross-cluster-topic-sync-teardown
  Error: Check timed out after 60.0s

The check's sequence (abbreviated, from the replayed source):
  1. iam.create_role(...) + attach AWSMSKReplicatorExecutionRole, ec2 subnet/SG discovery (3 subnets in distinct AZs)
  2. kafka.create_cluster(... NumberOfBrokerNodes=3, ClientAuthentication={'Sasl': {'Iam': {'Enabled': True}}}) x2 (source, target), each polled to State 'ACTIVE' via describe_cluster_v2
  3. kafka.create_replicator(ReplicatorName=..., ServiceExecutionRoleArn=..., KafkaClusters=[{source},{target}], ReplicationInfoList=[{'SourceKafkaClusterArn': source, 'TargetKafkaClusterArn': target, 'TargetCompressionType': 'NONE', 'TopicReplication': {'TopicsToReplicate': [<topic>]}, 'ConsumerGroupReplication': {...}}]) -> polled via describe_replicator to ReplicatorState 'RUNNING'
  4. kafka.create_topic(ClusterArn=<source>, TopicName=<topic>, PartitionCount=1, ReplicationFactor=2)
  5. poll, every 15 s for up to 900 s:
       [t['TopicName'] for t in kafka.list_topics(ClusterArn=<target>)['Topics'] if <topic> in t['TopicName']]
     expected: a non-empty match once the replicator is RUNNING ('source topic never appeared on the target cluster's ListTopics' is the check's own failure text)
     actual:   the target cluster's ListTopics never reports any topic derived from the source cluster, so the predicate is never satisfied and the check is killed at the harness's 60 s budget
  6. (never reached) delete_topic / delete_replicator, each confirmed by an independent read resolving to NotFoundException

Source corroboration that step 5 can never succeed:
  * services/service-com-amazonaws-kafka-kafka/src/state.rs — `topics` is keyed (cluster ARN, topic name); `topics_in(cluster_arn)` ranges over exactly one cluster's keys; the only insertion is from CreateTopic for the addressed cluster. `ReplicatorCluster.alias` is stored and rendered into DescribeReplicator/ListReplicators output but is never used to mint a topic.
  * services/service-com-amazonaws-kafka-kafka/src/ops/list_topics.rs — the handler validates the ARN, pages `topics_in(input.cluster_arn)`, and never reads the replicator registry.
  * services/service-com-amazonaws-kafka-kafka/src/ops/create_replicator.rs — validates identity, aliases, list ceilings and tags, registers the record, and returns; its own doc comment notes the named clusters are deliberately not resolved, and no post-registration replication work is scheduled.

Note for the fixer, not filed as a separate defect: the check's wait loops sleep 30 s (cluster), 20 s (replicator), 5 s (topic deletion) and 15 s (replicator deletion) between probes, and the emulator's clock-owed transitions (CLUSTER_ACTIVATION_DELAY / REPLICATOR_ACTIVATION_DELAY / TOPIC_DELETION_DELAY, all 500 ms) are long enough that each loop needs a second probe. Replicating topics is therefore necessary but may not by itself bring the check inside the 60 s budget; re-replay the check id after the fix to confirm.

## Steps to Reproduce
1. Boot the emulator.
2. Discover 3 subnets in distinct AZs of the request's region plus that VPC's default security group (ec2.describe_subnets / ec2.describe_security_groups).
3. Create an IAM role trusted by kafka.amazonaws.com with AWSMSKReplicatorExecutionRole attached.
4. Create two provisioned MSK clusters with kafka.create_cluster(KafkaVersion=<an ACTIVE version from kafka.list_kafka_versions()>, NumberOfBrokerNodes=3, BrokerNodeGroupInfo={'InstanceType':'kafka.m5.large','ClientSubnets':<the 3 subnets>,'SecurityGroups':[<sg>]}, ClientAuthentication={'Sasl':{'Iam':{'Enabled':True}}}) — call them SOURCE and TARGET — and poll kafka.describe_cluster_v2 until each reports State 'ACTIVE'.
5. kafka.create_replicator(ReplicatorName='repltest', ServiceExecutionRoleArn=<role arn>, KafkaClusters=[{'AmazonMskCluster':{'MskClusterArn':SOURCE},'VpcConfig':{...}},{'AmazonMskCluster':{'MskClusterArn':TARGET},'VpcConfig':{...}}], ReplicationInfoList=[{'SourceKafkaClusterArn':SOURCE,'TargetKafkaClusterArn':TARGET,'TargetCompressionType':'NONE','TopicReplication':{'TopicsToReplicate':['repl-topic']},'ConsumerGroupReplication':{'ConsumerGroupsToReplicate':['.*']}}]).
6. Poll kafka.describe_replicator(ReplicatorArn=...) until ReplicatorState is 'RUNNING' — observe it does reach RUNNING.
7. kafka.create_topic(ClusterArn=SOURCE, TopicName='repl-topic', PartitionCount=1, ReplicationFactor=2).
8. Poll kafka.list_topics(ClusterArn=TARGET) -> expect a topic derived from 'repl-topic' to appear on the TARGET cluster while the replicator is RUNNING; observe the TARGET listing never reports it, for any wait.
9. kafka.delete_topic(ClusterArn=SOURCE, TopicName='repl-topic') -> the TARGET listing must stop reporting the replicated topic; kafka.delete_replicator(...) -> replication must stop and kafka.describe_replicator must resolve to HTTP 404 NotFoundException.
10. Re-replay kafka.e2e-replicator-cross-cluster-topic-sync-teardown.
