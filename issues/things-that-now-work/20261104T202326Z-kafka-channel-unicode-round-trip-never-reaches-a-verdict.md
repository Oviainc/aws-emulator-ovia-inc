Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.channel-update-unicode-fields-round-trip
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.channel-update-unicode-fields-round-trip|_wait_channel_status#17|Timeout|status = kafka.describe_channel(ClusterArn=cluster_arn, ChannelArn=channel_arn)["Status"]
Issue-Commit: f5d8661b1b5f7b261f923feb0488ab9136ea31db

# Channel unicode round-trip never reaches a verdict: CreateClusterV2/CreateTopic/CreateChannel transitions are clock-owed, so an AWS-cadence poller is charged a full interval per transition

> _Note: this issue was identified against the code base at commit `f5d8661b1b5f7b261f923feb0488ab9136ea31db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChannel

## Description
Cause 1 of 1 — the MSK Express channel sequence a client runs (CreateClusterV2 -> wait cluster ACTIVE -> CreateTopic -> wait topic ACTIVE -> CreateChannel -> wait channel ACTIVE -> UpdateChannel -> wait the cluster operation and the channel again) cannot be completed against the emulator inside a realistic budget, so the unicode round-trip contract it exercises (ChannelName, S3DestinationConfiguration.Storage.OutputPrefix and a non-ASCII Tags value unchanged across an unrelated UpdateChannel, with DataFreshnessInSeconds reported as the new value) is never observed at all.

AWS: these transitions are genuinely asynchronous and non-terminal until the work completes. Smithy: DescribeChannel (GET /v1/clusters/{ClusterArn}/channels/{ChannelArn}) returns a required `Status` of enum ChannelStatus, and UpdateChannel (PUT, idempotent) returns `ClusterOperationArn`; a cluster is CREATING until ACTIVE, a topic CREATING until ACTIVE, a channel CREATING until ACTIVE, and a channel update's cluster operation runs until it reports COMPLETED. A client therefore polls DescribeClusterV2 / DescribeTopic / DescribeChannel / DescribeClusterOperationV2 at intervals sized for AWS (10-30 s) and the sequence costs as many intervals as there are real transitions — on AWS each wait is satisfied by the poll that follows the actual completion.

Emulator: every one of those transitions is owed to a clock instead of being complete on return, and the owed span is far shorter than any AWS-sized poll interval, so the FIRST probe after each write always observes the non-terminal value and the caller is then charged its ENTIRE interval. In the kafka state layer the activation instants are fixed at 500 ms and materialised only on a LATER read (CLUSTER_ACTIVATION_DELAY, TOPIC_ACTIVATION_DELAY, CHANNEL_ACTIVATION_DELAY are each Duration::from_millis(500); Cluster::settle, Topic::settle and Channel::settle apply CREATING -> ACTIVE only when now >= *_due, reached through settled_cluster_in / settled_topic_in / settled_channel_in). The replayed check therefore spends ~20 s waiting for the cluster, ~10 s for the topic and ~15 s for the channel purely in sleeps for work the emulator had already finished, and ends with 'Error: Check timed out after 60.0s' — no HTTP status, no error code and no traceback frame, i.e. nothing was refused and not one assertion was evaluated.

Why: a client that polls at an AWS-appropriate cadence pays a full poll interval per transition the emulator has in fact already completed, so emulator-backed suites and provisioning scripts time out on sequences AWS completes well inside the same budget, and the behaviour behind each transition (here the unicode channel fields surviving an unrelated UpdateChannel, and the updated DataFreshnessInSeconds being reported) is unverifiable.

Fix: the kafka service's state layer — make the asynchronous instants for a created cluster, topic and channel (and a channel update's cluster operation) complete on the write path, or settle them so a first probe after the write observes the terminal value, rather than deferring them to a clock a later read materialises. Then re-replay the check to see whether any remaining step (this sequence also calls UpdateChannel, whose unimplemented scaffold stub is filed separately) still blocks a verdict.

Related open issue, named rather than relied on: 20261104T171616Z-kafka-msk-asynchronous-transitions-are-clock-owed-so-an.md describes this same clock-owed settling for other control-plane lifecycles; the replay did not mark this check as owned by it, so it is filed here with its own grounding.

Traces: kafka.channel-update-unicode-fields-round-trip.

## Evidence
Failing check (verdict mismatch): kafka.channel-update-unicode-fields-round-trip. Detail from the replay report, in full: 'Error: Check timed out after 60.0s' — no frame, no status, no error code, so the emulator refused nothing and no assertion ran.

What the check does, from its source as replayed: creates an Express cluster via CreateClusterV2 and polls DescribeClusterV2 at 20 s intervals for State ACTIVE; CreateTopic then polls DescribeTopic at 10 s intervals for Status ACTIVE; CreateChannel (one TopicConfiguration, S3DestinationConfiguration with DataFreshnessInSeconds 300 and Storage.OutputPrefix 'logger/donnees-zhongwen/', Tags {'sim2real-unicode': non-ASCII value}) then polls DescribeChannel at 15 s intervals for Status ACTIVE; then UpdateChannel with S3DestinationUpdate {DataFreshnessInSeconds: 600}, polls the returned ClusterOperationArn to COMPLETED and DescribeChannel to ACTIVE again, and finally asserts ChannelName, Storage.OutputPrefix and Tags are byte-identical to the create and DataFreshnessInSeconds == 600.

Emulator side, read from the checkout: services/service-com-amazonaws-kafka-kafka/src/state.rs declares CLUSTER_ACTIVATION_DELAY, TOPIC_ACTIVATION_DELAY and CHANNEL_ACTIVATION_DELAY as Duration::from_millis(500) with the comment 'The project fixes an emulated clock-owed transition at 250-750 ms, so a caller that describes the channel immediately still observes CREATING while a caller that waits sees ACTIVE', and Channel::settle / Topic::settle / Cluster::settle only promote CREATING -> ACTIVE when now >= the stored *_due instant, i.e. on a later read. CreateChannel (services/service-com-amazonaws-kafka-kafka/src/ops/create_channel.rs) mints the record through Channel::streaming and returns while it is CREATING.

Expected (AWS): the sequence completes and the assertions are evaluated — DescribeChannel echoes the non-ASCII ChannelName, the non-ASCII Storage.OutputPrefix and the non-ASCII tag value unchanged before and after the update, and reports S3DestinationConfiguration.DataFreshnessInSeconds as the updated value.
Actual (emulator): no verdict at all; the wall clock is consumed by one full poll interval per already-finished transition (~20 s + ~10 s + ~15 s before UpdateChannel is even reached) and the check is cut off at 60 s.

## Steps to Reproduce
1. Boot the emulator.
2. CreateClusterV2 an Express PROVISIONED cluster and, IMMEDIATELY after the response returns, call DescribeClusterV2 exactly once. Record whether State is already ACTIVE or still CREATING.
3. On that cluster call CreateTopic and, immediately after the response, call DescribeTopic once; record whether Status is ACTIVE or CREATING.
4. CreateChannel with one TopicConfiguration and an S3DestinationConfiguration, and immediately after the response call DescribeChannel once; record whether Status is ACTIVE or CREATING.
5. Now run the sequence a client runs, with AWS-sized cadences: wait for cluster ACTIVE polling every 20 s, wait for topic ACTIVE polling every 10 s, wait for channel ACTIVE polling every 15 s. Measure the wall-clock time consumed; assert the whole sequence completes well inside 60 s.
6. Re-replay kafka.channel-update-unicode-fields-round-trip and confirm it produces a pass/fail verdict instead of 'Check timed out after 60.0s'.
