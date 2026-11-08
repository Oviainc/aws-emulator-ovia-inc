Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.vpc-connection-create-available-lifecycle kafka.vpc-connection-reject-transitions-state kafka.vpc-connection-malformed-subnet-config-rejected kafka.update-broker-count-operation-lifecycle kafka.update-replication-info-concurrent-conflicting-updates kafka.update-replication-info-invalid-target-topics-rejected kafka.update-replication-info-server-enforced-propagation kafka.update-channel-wrong-state-rejected kafka.channel-list-pagination-and-filter-consistency kafka.channel-update-unicode-fields-round-trip kafka.configuration-delete-in-use-conflict kafka.describe-topic-partitions-invalid-topic-rejected kafka.replicator-create-invalid-role-rejected kafka.server-side-cluster-policy-cross-account-enforcement
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.vpc-connection-create-available-lifecycle|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: kafka.vpc-connection-reject-transitions-state|_enable_multi_vpc#2|Timeout|time.sleep(interval_s)
Issue-Key: kafka.vpc-connection-malformed-subnet-config-rejected|_enable_multi_vpc#5|Timeout|time.sleep(interval_s)
Issue-Key: kafka.update-broker-count-operation-lifecycle|_wait_operation_state#7|Timeout|time.sleep(interval_s)
Issue-Key: kafka.update-replication-info-concurrent-conflicting-updates|_wait_for_replicator_state#12|Timeout|time.sleep(interval_s)
Issue-Key: kafka.update-replication-info-invalid-target-topics-rejected|_wait_cluster_v2_active#4|Timeout|time.sleep(interval_s)
Issue-Key: kafka.update-replication-info-server-enforced-propagation|_wait_replicator_state#7|Timeout|time.sleep(interval_s)
Issue-Key: kafka.update-channel-wrong-state-rejected|_wait_channel_status#11|Timeout|resp = kafka.describe_channel(ChannelArn=channel_arn, ClusterArn=cluster_arn)
Issue-Key: kafka.channel-list-pagination-and-filter-consistency|_wait_channel_status#10|Timeout|resp = kafka.describe_channel(ChannelArn=channel_arn, ClusterArn=cluster_arn)
Issue-Key: kafka.channel-update-unicode-fields-round-trip|_wait_channel_status#17|Timeout|time.sleep(interval_s)
Issue-Key: kafka.configuration-delete-in-use-conflict|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: kafka.describe-topic-partitions-invalid-topic-rejected|_wait_for_topic_partitions#6|Timeout|time.sleep(5)
Issue-Key: kafka.replicator-create-invalid-role-rejected|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: kafka.server-side-cluster-policy-cross-account-enforcement|_poll_for_error_code#17|Timeout|time.sleep(interval_s)
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# MSK asynchronous transitions are clock-owed, so an AWS-cadence poller pays a full interval per transition and 14 control-plane lifecycle checks never reach a verdict

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterOperation

## Description
Cause 1 of 1: every multi-step MSK control-plane sequence that polls a transition at an AWS-appropriate cadence runs out of budget before it finishes, so the behaviour behind the transition is never observed. The resolved issue 20261028T200901Z-kafka-createvpcconnection-under-contention-is-never-reached-the-multi.md recorded this exact symptom for one check and attributed it to clock-owed settling; the symptom recurs here across 14 different checks, none of which carries that issue's check id.

AWS: the transitions these sequences wait on are genuinely asynchronous and are reported as non-terminal until the work completes: a new cluster is CREATING until it is ACTIVE, a cluster operation (UpdateBrokerCount, UpdateConnectivity, UpdateStorage) is UPDATE_IN_PROGRESS until UPDATE_COMPLETE, a VPC connection is CREATING until AVAILABLE and absent after delete, a replicator is CREATING until RUNNING, and a channel is CREATING until ACTIVE. A client therefore polls DescribeClusterV2 / DescribeClusterOperation / DescribeVpcConnection / DescribeReplicator / ListChannels at intervals sized for AWS (15-30 s), and the sequence completes in as many intervals as there are real transitions.
Emulator: each transition is owed to a clock rather than complete on return, so the FIRST probe after every write always observes the non-terminal value and the caller is charged its entire poll interval; a sequence with two or three transitions (create cluster, run an update operation, then create and settle a VPC connection or a replicator) therefore costs 60 s or more of pure waiting and the checks end with 'Check timed out after 60.0s' and no frame, having asserted nothing. Hypothesis (the only part not stated by the replay output): the same clock-owed settling the resolved issue above describes — a created cluster and a cluster operation are promoted only on a LATER read — is still in force; an alternative this evidence does not rule out is that one of these specific transitions never completes at all.
Why: none of these contracts can be verified against the emulator, and a client that polls at an AWS-appropriate cadence pays a full interval per transition the emulator had in fact already finished, so emulator-backed suites time out on sequences AWS completes. Covered behaviours left unobserved include: the VPC-connection CREATING->AVAILABLE->deleted lifecycle and its reject transition, UpdateBrokerCount's operation reaching UPDATE_COMPLETE before DescribeClusterV2 reports the new broker count, UpdateReplicationInfo's propagation and its conflicting-update and invalid-target refusals, the channel list/filter and unicode round trips and the wrong-state channel refusal, the in-use-configuration delete conflict, the invalid-topic DescribeTopicPartitions refusal, CreateReplicator's invalid-role refusal and server-side cluster-policy cross-account enforcement.
Fix: make the transition instants in the kafka service's state layer complete-on-return (or settle on the write path) for a created cluster, a cluster operation, a VPC connection, a replicator and a channel, so a first probe after a write observes the terminal value; then re-replay the checks to see whether any individual transition still never completes. Note three of these checks additionally touch operations whose stubs are filed separately (ListChannels, UpdateChannel, DescribeTopicPartitions); the requirement here is only that the sequence reaches a verdict.
Traces: the 14 checks in trace_ids, all reporting 'Check timed out after 60.0s'.

## Evidence
All 14 checks are kafka behavioral checks with verdict mismatch and the identical detail 'Error: Check timed out after 60.0s' — no HTTP status, no error code and no traceback frame, so nothing was refused and no assertion was evaluated. None is marked ALREADY FILED or OWNED.

The poll cadences in the timed-out checks, read from their sources, are AWS-sized and are what the 60 s ceiling cannot absorb: kafka.vpc-connection-create-available-lifecycle waits for cluster ACTIVE at 30 s intervals, then polls the UpdateConnectivity cluster operation at 30 s until UPDATE_COMPLETE, then DescribeVpcConnection at 15 s until AVAILABLE and again until NotFoundException after delete; kafka.update-broker-count-operation-lifecycle waits for ACTIVE then DescribeClusterOperation at 20 s until UPDATE_COMPLETE then for ACTIVE again; kafka.vpc-connection-reject-transitions-state and kafka.vpc-connection-malformed-subnet-config-rejected both call the same _enable_multi_vpc sequence (UpdateConnectivity + 30 s operation poll) first.

Expected (AWS): each sequence completes and its assertions are evaluated — e.g. the VPC connection reaches AVAILABLE echoing its VpcId, DeleteVpcConnection makes DescribeVpcConnection answer NotFoundException 'The requested resource doesn\u2019t exist.' and a second delete answer the same; UpdateBrokerCount's operation reaches UPDATE_COMPLETE and only then does DescribeClusterV2 report the new NumberOfBrokerNodes.
Actual: no verdict at all for any of the 14.

Prior art: 20261028T200901Z-kafka-createvpcconnection-under-contention-is-never-reached-the-multi.md (resolved) documented the same 'Check timed out after 60.0s' with no frame for kafka.concurrent-vpc-connection-duplicate-create-race and cited the clock-owed cluster/operation settling in the kafka state layer as the leading cause.

## Steps to Reproduce
1. Boot the emulator.
2. CreateClusterV2 a PROVISIONED cluster and, IMMEDIATELY after the response, call DescribeClusterV2 once. Record whether State is already ACTIVE or still CREATING, and how long it stays CREATING.
3. On that cluster call UpdateConnectivity (multi-VPC, Sasl.Iam) and, immediately after the response, call DescribeClusterOperationV2/DescribeClusterOperation once; record whether OperationState is already UPDATE_COMPLETE or still UPDATE_IN_PROGRESS.
4. Repeat the same first-probe measurement for CreateVpcConnection (State CREATING vs AVAILABLE), CreateReplicator (CREATING vs RUNNING) and CreateChannel (CREATING vs ACTIVE).
5. Then run the full sequence a client would: create cluster -> wait ACTIVE at a 30 s cadence -> UpdateConnectivity -> poll the operation at 30 s until UPDATE_COMPLETE -> CreateVpcConnection -> poll at 15 s until AVAILABLE -> delete and poll until NotFoundException. Assert the whole sequence completes inside 60 s of wall clock.
6. Replay the 14 checks in trace_ids and confirm each now produces a pass/fail verdict rather than a timeout.
