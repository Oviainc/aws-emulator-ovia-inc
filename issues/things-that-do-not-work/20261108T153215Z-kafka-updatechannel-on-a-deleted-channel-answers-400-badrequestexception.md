Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka.channel-lifecycle-create-describe-update-delete
Issue-Shape-Id: com.amazonaws.kafka#Kafka
Issue-Key: kafka.channel-lifecycle-create-describe-update-delete|run#27|AssertionError|assert update_code == "NotFoundException", (
Issue-Commit: ba47091dbe8b2cea4957d024b7a3df683e2ec267

# UpdateChannel on a deleted channel answers 400 BadRequestException (invalid link ARN) instead of 404 NotFoundException, so one gone channel yields two codes across the channel lifecycle

> _Note: this issue was identified against the code base at commit `ba47091dbe8b2cea4957d024b7a3df683e2ec267`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateChannel

## Description
ONE net-new root cause in the kafka (MSK) service, grounded in a behavioral check the suite marked neither ALREADY FILED nor OWNED.

Cause 1 of 1 — UpdateChannel addressed at a channel its cluster no longer holds (because DeleteChannel removed it) is refused with the channel-ARN BadRequest instead of the service's fixed not-found.

AWS: Smithy kafka#UpdateChannel (PUT /v1/clusters/{ClusterArn}/channels/{ChannelArn}; ClusterArn and ChannelArn are @httpLabel @required members) models BadRequestException (HTTP 400) and NotFoundException (HTTP 404) among its errors, exactly as kafka#DescribeChannel (GET on the same URI) does. Recorded behavior: after DeleteChannel completes on an existing (ACTIVE, Express-broker) cluster and the channel has dropped out of ListChannels, BOTH channel-scoped reads/writes aimed at that same, previously existing ChannelArn are refused identically — Error.Code 'NotFoundException' with Error.Message "The requested resource doesn\u2019t exist." That message is entirely FIXED text (the apostrophe is U+2019); it interpolates nothing from the request or from state. A channel that once existed and was deleted is therefore a resource that went missing (404), not a bad identifier in the caller's request (400) — the deleted-channel case is the not-found case for every channel operation, including UpdateChannel.

Emulator: DescribeChannel on the deleted channel does answer NotFoundException with that exact fixed message (that leg of the same check passed), but UpdateChannel on the identical ClusterArn/ChannelArn pair answers HTTP 400 Error.Code 'BadRequestException' with Error.Message "The specified link ARN isn\u2019t valid. Specify a valid link ARN, and try again." and NotFoundException$InvalidParameter 'ChannelArn' — the invalid-link-ARN refusal, whose fixed text belongs to a channel ARN the cluster NEVER held. The handler resolves the channel with the same lookup the reader uses, then maps 'not resolved' to BadRequestException unconditionally, so it cannot tell a never-created ARN from a deleted one and one lifecycle reports two different codes for one gone channel.

Why: a client that drives a channel's lifecycle (create → describe → update → delete) and keys its control flow on the error class is routed to the wrong remedy: 404 means 'the channel is gone, stop retrying and clean up', 400 means 'my request is malformed, fix the code and do not retry'. Idempotent/retrying updaters and reconcilers that treat 404 as 'already deleted' will instead surface a hard client error, and no uniform handler can be written when DescribeChannel says 404 and UpdateChannel says 400 about the very same deleted channel.

Fix: services/service-com-amazonaws-kafka-kafka/src/ops/update_channel.rs — the two arms that answer crate::state::invalid_channel_arn() when the channel does not resolve (the settled_channel_in resolution and the begin_channel_update fallback) must distinguish a channel the cluster once held and no longer holds from a well-formed ARN the cluster never held: the former is NotFoundException carrying crate::state::RESOURCE_NOT_FOUND_MESSAGE (as describe_channel.rs already answers), the latter keeps the link-ARN BadRequest. The cluster-first gate order and the request-only destination/freshness checks ahead of it are correct and must stay. Related but NOT covering this cause: 20261106T132758Z-kafka-msk-channel-refusals-invert-notfound-badrequest-two-cluster.md (its Cause 2 records that DeleteChannel with a NEVER-created channel ARN on a real cluster must be the link-ARN BadRequest; it names neither UpdateChannel's post-delete refusal nor this check id, and the two requirements together are what force the never-existed/deleted distinction), and 20261104T171616Z-kafka-updatechannel-stub.md (the stub is no longer the blocker here — the handler answers, it answers the wrong error class).

Traces: kafka.channel-lifecycle-create-describe-update-delete.

## Evidence
Suite simparity-behavioral-aws-v1, service heading '### kafka'. Verdict 'mismatch', not marked ALREADY FILED and not marked OWNED by any open issue.

kafka.channel-lifecycle-create-describe-update-delete:
  The check creates an Express-broker PROVISIONED cluster (CreateClusterV2, KafkaVersion 3.8.x, express.m7g.large, 3 subnets) and waits for ACTIVE, creates an S3-destination channel (CreateChannel with DataFreshnessInSeconds 300), waits for channel Status ACTIVE, asserts it is in ListChannels, UpdateChannel S3DestinationUpdate={'DataFreshnessInSeconds': 600} and re-reads 600 — all of this PASSED. It then DeleteChannel and polls ListChannels until the channel is no longer listed (PASSED), and probes the two refusals:
    DescribeChannel(ClusterArn=<that cluster>, ChannelArn=<the deleted channel>)
      expected ('NotFoundException', 'The requested resource doesn\u2019t exist.')
      actual   matched — PASSED
    UpdateChannel(ClusterArn=<that cluster>, ChannelArn=<the deleted channel>, S3DestinationUpdate={'DataFreshnessInSeconds': 600})
      expected Error.Code 'NotFoundException' (HTTP 404), Error.Message 'The requested resource doesn\u2019t exist.'
      actual   AssertionError (check_kafka_channel_lifecycle_create_describe_update_delete.py line 111): "expected UpdateChannel on the deleted channel to raise NotFoundException, got 'BadRequestException'"
  The emulator's 400 body is the shared link-ARN refusal: Error.Message 'The specified link ARN isn\u2019t valid. Specify a valid link ARN, and try again.' (fixed text, U+2019, no interpolated value) with InvalidParameter 'ChannelArn', produced by crate::state::invalid_channel_arn() (services/service-com-amazonaws-kafka-kafka/src/state.rs ~line 2342) from the two unresolved-channel arms of services/service-com-amazonaws-kafka-kafka/src/ops/update_channel.rs (~lines 274-282 and ~327-338). The expected message is the service constant RESOURCE_NOT_FOUND_MESSAGE (state.rs ~line 1151), which describe_channel.rs ~line 230 already uses for the same condition.
  Fixed vs variable text: both messages are 100% fixed text — nothing in either is taken from the request, the cluster or the channel, so no per-run value (ARN, name, timestamp, request id) is part of the requirement. Only the HTTP status (404 vs 400), the Error.Code (NotFoundException vs BadRequestException) and which fixed sentence is emitted diverge.

## Steps to Reproduce
1. Boot the emulator.
2. CreateClusterV2 an Express-broker provisioned cluster (KafkaVersion '3.8.x', InstanceType 'express.m7g.large', 3 subnets in distinct AZs, no StorageInfo) and poll DescribeCluster until State is ACTIVE.
3. CreateTopic on the cluster and wait until DescribeTopic reports Status ACTIVE; create an IAM service-execution role and an S3 bucket for the channel destination.
4. CreateChannel on the cluster with TopicConfigurationList=[{TopicArn, RecordConverter:{ValueConverter:'JSON'}}] and S3DestinationConfiguration{ServiceExecutionRoleArn, DataFreshnessInSeconds:300, Storage{...}, DeadLetterQueueS3{...}}; poll DescribeChannel until Status is ACTIVE. Keep its ChannelArn.
5. DeleteChannel(ClusterArn=<cluster>, ChannelArn=<channel>) and poll ListChannels until that ChannelArn is no longer listed.
6. DescribeChannel(ClusterArn=<cluster>, ChannelArn=<the deleted channel>) — must be HTTP 404, Error.Code 'NotFoundException', Error.Message 'The requested resource doesn\u2019t exist.' (this already matches and must keep matching).
7. UpdateChannel(ClusterArn=<cluster>, ChannelArn=<the deleted channel>, S3DestinationUpdate={'DataFreshnessInSeconds': 600}) — must be HTTP 404, Error.Code 'NotFoundException' and the same fixed message. Observe HTTP 400 'BadRequestException' with 'The specified link ARN isn\u2019t valid. Specify a valid link ARN, and try again.' instead.
8. Confirm the never-created case is still the link-ARN BadRequest: on the same ACTIVE cluster, call UpdateChannel/DeleteChannel with a well-formed ChannelArn that was NEVER created and expect HTTP 400 'BadRequestException' with that link-ARN sentence.
9. Re-replay kafka.channel-lifecycle-create-describe-update-delete.
