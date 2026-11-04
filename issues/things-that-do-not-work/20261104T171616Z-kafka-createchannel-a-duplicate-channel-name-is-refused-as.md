Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.channel-create-duplicate-name-conflict kafka.channel-create-invalid-config-rejected
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.channel-create-duplicate-name-conflict|run#13|AssertionError|assert losers[0][1] == "BadRequestException", f"expected BadRequestException for the loser, got {losers[0][1]}"
Issue-Key: kafka.channel-create-invalid-config-rejected|_error_of#6|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded: {result!r}")
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# CreateChannel: a duplicate channel name is refused as ConflictException instead of BadRequestException, and an out-of-range DataFreshnessInSeconds is accepted (2 causes)

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
TWO distinct root causes in CreateChannel's admission path — one wrong refusal class/message, one missing validation. Each must be closed separately; both must be closed before this issue is submitted.

Cause 1 of 2: a duplicate ChannelName on the same cluster is refused with the wrong error class and message.
AWS: two concurrent CreateChannel calls with the identical ChannelName on one cluster race to a single winner; the loser is refused with BadRequestException (400) and the message "A channel with name '<the ChannelName from the request>' already exists for this cluster." — fixed connectives with the requested channel name quoted in it. Exactly one channel with that name then exists, and its ChannelArn is the winner's.
Emulator: the loser is refused with ConflictException (409) — the wrong class entirely, so the modeled 400 client contract is not met.
Why: an SDK raises a different exception type than AWS does, so a client that catches MSK's BadRequestException to fall back to 'describe the existing channel' does not catch it at all and the create path fails hard; code branching on 409 versus 400 takes the opposite branch.
Fix: the duplicate-name refusal in CreateChannel's handler must raise BadRequestException with the already-exists sentence rather than the generic conflict error.
Traces: kafka.channel-create-duplicate-name-conflict.

Cause 2 of 2: CreateChannel accepts an S3 destination DataFreshnessInSeconds outside the documented 300-900 second range.
AWS: DataFreshnessInSeconds must be between 300 and 900; a value outside that range (e.g. 1) is refused with BadRequestException and the fixed message 'Data freshness must be between 300 and 900 seconds.' No channel is created, so ListChannels does not show the name. An otherwise-identical request with a valid value succeeds and the channel reaches ACTIVE.
Emulator: HTTP 200 with a ChannelArn and a ClusterOperationArn — the channel is created with an unusable freshness setting.
Why: the caller is told a broken delivery configuration is live; the channel would never be accepted by AWS, so infrastructure validated against the emulator fails at deploy time, and range-validation tests pass vacuously.
Fix: CreateChannel's handler validation of S3DestinationConfiguration.DeliveryConfiguration.DataFreshnessInSeconds (bounds check) before the channel and its cluster operation are minted.
Traces: kafka.channel-create-invalid-config-rejected.

## Evidence
Both checks are kafka behavioral checks with verdict mismatch; neither is marked ALREADY FILED or OWNED (the open issues 20261029T175822Z-kafka-listchannels-stub.md and the UpdateChannel stub concern other operations).

Cause 1 — kafka.channel-create-duplicate-name-conflict: two concurrent CreateChannel calls with one ChannelName on an Express cluster.
  Expected: one winner; the loser ('BadRequestException', "A channel with name '<channel name>' already exists for this cluster."); exactly one channel with that name in ListChannels.
  Actual: AssertionError 'expected BadRequestException for the loser, got ConflictException'.

Cause 2 — kafka.channel-create-invalid-config-rejected: CreateChannel with S3 delivery DataFreshnessInSeconds=1.
  Expected: ('BadRequestException', 'Data freshness must be between 300 and 900 seconds.') and the name absent from ListChannels.
  Actual: HTTP 200 {'ChannelArn': 'arn:aws:kafka:us-west-2:000000000000:channel/sim2real-mskx-<id>/<uuid>-1/sim2real-badchan-<id>/<uuid>', 'ClusterOperationArn': 'arn:aws:kafka:us-west-2:000000000000:cluster-operation/...'} (ids per-run) — AssertionError 'expected a rejection, but the call succeeded'.

## Steps to Reproduce
1. Boot the emulator, create an MSK Express-broker cluster (CreateClusterV2), an S3 bucket plus service-execution role, and a topic; wait for all to be ready.
2. Cause 1: from two threads issue the SAME CreateChannel request (identical ChannelName, TopicConfigurationList, S3DestinationConfiguration). Assert exactly one succeeds and the loser is BadRequestException with Message "A channel with name '<that name>' already exists for this cluster."; assert ListChannels shows exactly one channel with that name and that its ChannelArn is the winner's.
3. Cause 2: CreateChannel with S3DestinationConfiguration.DeliveryConfiguration.DataFreshnessInSeconds=1. Assert 400 BadRequestException with Message 'Data freshness must be between 300 and 900 seconds.' and that ListChannels does not contain the channel name. Then issue the identical request with DataFreshnessInSeconds=300 and assert it succeeds and the channel reaches ACTIVE (positive control).
4. Replay kafka.channel-create-duplicate-name-conflict and kafka.channel-create-invalid-config-rejected.
