Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.concurrent-channel-update-serializes-cleanly kinesis.update-channel-concurrent-modification-conflict kinesis.concurrent-update-channel-lost-update-detected
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.concurrent-channel-update-serializes-cleanly|run#8|AssertionError|assert code == "ResourceInUseException", (
Issue-Key: kinesis.update-channel-concurrent-modification-conflict|run#35|AssertionError|assert len(successes) == 1, f"expected exactly one winner, got outcomes={outcomes}"
Issue-Key: kinesis.concurrent-update-channel-lost-update-detected|_wait_logging_config_usable#3|Timeout|time.sleep(3)
Issue-Commit: dab4acbec7168116790618fb6879ca8d40945264

# kinesis: concurrent UpdateChannel calls are neither serialized nor refused with ResourceInUseException — every UpdateChannel answers NotImplemented, so no racer wins and the channel config never settles

> _Note: this issue was identified against the code base at commit `dab4acbec7168116790618fb6879ca8d40945264`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateChannel

## Description
Cause 1 of 1: UpdateChannel has no per-channel update state machine, so the concurrency contract of UpdateChannel (exactly one winner, the loser refused with ResourceInUseException, the channel settling ACTIVE on exactly one racer's configuration) is unreachable; the emulator answers every UpdateChannel call with error code NotImplemented instead.

NOTE ON OVERLAP: the open issue `20261027T172504Z-kinesis-updatechannel-stub.md` ("Implement kinesis UpdateChannel: unimplemented scaffold stub") carries the replay keys `kinesis.update-channel-persists-configuration-change`, `kinesis.update-channel-properties-persist`, `kinesis.update-channel-reflects-in-describe` only, and asks only that the operation be implemented. The three checks named below are NOT carried by it, and they grade behavior beyond mere implementation: concurrent-update serialization and the ResourceInUseException conflict. Implementing UpdateChannel as a plain read-modify-write would still leave these three failing, so this cause needs the state machine as well.

AWS: Smithy (com.amazonaws.kinesis#Kinesis_20131202) models UpdateChannel with required input member `ChannelARN` (length 1-2048, pattern `^arn:aws.*:kinesis:.*:\d{12}:channel/\S+$`) and the optional update members `S3DestinationConfiguration` (S3DestinationUpdateInput), `S3TablesDestinationConfiguration` and `LoggingConfiguration` (ChannelLoggingUpdateInput); output carries required `ChannelDescription`; modeled errors are AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException and ValidationException, all client errors at HTTP 400. On AWS an accepted UpdateChannel moves the channel out of ACTIVE into UPDATING and only one update is admitted at a time: a second UpdateChannel issued against a channel that is CREATING, UPDATING or DELETING is refused with error code `ResourceInUseException` at HTTP 400, while the admitted one returns 200 with the full ChannelDescription and the channel returns to ACTIVE carrying exactly the submitted values (`S3DestinationConfiguration.DataFreshnessInSeconds`, or `LoggingConfiguration.CloudWatchLogs.{Enabled,LogGroupName,LogStreamName}`) — never a merge of two racers' values. Recorded behavior only for the refusal text: these checks assert the error CODE and the settled configuration, not the message string. Hypothesis (wording not stated by the model or these recordings): AWS's ResourceInUseException message for this class is fixed text naming the channel and its blocking state, with the channel name/ARN and state taken from the request and service state; a fixer should keep the code and the 400 status as the hard requirement.

Emulator: every UpdateChannel call is refused with error code `NotImplemented` ("operation UpdateChannel is scaffolded but not implemented in this emulator", HTTP 501) from the scaffold handler. kinesis.update-channel-concurrent-modification-conflict therefore observes `outcomes=[('NotImplemented', 400), ('NotImplemented', 700)]` where exactly one 200 and one ResourceInUseException are required; kinesis.concurrent-channel-update-serializes-cleanly observes `results=[('error', 'NotImplemented'), ('error', 'NotImplemented')]` where the only acceptable rejection code is ResourceInUseException and at least one racer must succeed; kinesis.concurrent-update-channel-lost-update-detected never gets past its SERIAL warm-up UpdateChannel (it retries the NotImplemented refusal in a loop) and so hits the harness 60 s cap. No channel status ever leaves ACTIVE for UPDATING, and DescribeChannel never reflects any submitted DataFreshnessInSeconds or LoggingConfiguration.

Why: a client that changes a channel's destination buffering or its CloudWatch Logs configuration gets a refusal that no AWS SDK retry/branch logic models (NotImplemented is not in UpdateChannel's modeled error set), so configuration changes are silently impossible. A client that relies on Kinesis serializing updates — catching ResourceInUseException to back off and re-submit, and assuming the channel settles on exactly one submitted configuration — cannot distinguish a conflict from a hard failure, and once UpdateChannel is implemented without the state machine two concurrent updates can interleave into a LoggingConfiguration/DataFreshnessInSeconds combination neither caller submitted (a lost update).

Fix: the UpdateChannel handler and the channel state it writes — `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/update_channel.rs` plus the service's channel state record (channel status field and its ACTIVE -> UPDATING -> ACTIVE transition, admitting one in-flight update per channel and refusing a concurrent one with ResourceInUseException at HTTP 400), with DescribeChannel reading back the settled ChannelDescription.

Traces: kinesis.concurrent-channel-update-serializes-cleanly, kinesis.update-channel-concurrent-modification-conflict, kinesis.concurrent-update-channel-lost-update-detected.

## Evidence
Expected (AWS): of two concurrent UpdateChannel calls on one ACTIVE channel, exactly one returns HTTP 200 with the required ChannelDescription and the other is refused with error code ResourceInUseException at HTTP 400; DescribeChannel then reports ChannelStatus=ACTIVE with S3DestinationConfiguration.DataFreshnessInSeconds equal to one of the submitted values (e.g. one of the two valid 300-900 values a caller submitted), or LoggingConfiguration.CloudWatchLogs.LogStreamName equal to exactly one racer's value with Enabled=true.

Actual (emulator): both racers are refused with error code `NotImplemented` (HTTP 501, message "operation UpdateChannel is scaffolded but not implemented in this emulator"). Replay output:
  - kinesis.update-channel-concurrent-modification-conflict: AssertionError: expected exactly one winner, got outcomes=[('NotImplemented', 400), ('NotImplemented', 700)]
  - kinesis.concurrent-channel-update-serializes-cleanly: AssertionError: a racer failed with an unexpected code (only ResourceInUseException, for a channel already UPDATING/CREATING/DELETING, is an acceptable rejection), got code='NotImplemented', results=[('error', 'NotImplemented'), ('error', 'NotImplemented')]
  - kinesis.concurrent-update-channel-lost-update-detected: Check timed out after 60.0s — its serial `_wait_logging_config_usable` warm-up keeps retrying UpdateChannel because the NotImplemented refusal never clears.

Model evidence: Smithy UpdateChannel declares ResourceInUseException (client error, HTTP 400) among its errors and a required ChannelDescription output; NotImplemented/501 is not a modeled outcome of this operation.

## Steps to Reproduce
1. Boot the kinesis emulator.
2. Create an ON_DEMAND stream, an S3 bucket and a service execution role, then CreateChannel with StreamConfigurationList=[{StreamARN, RecordConfiguration={RecordFormatType: JSON}}] and S3DestinationConfiguration.StorageConfiguration={BucketARN, ExpectedBucketOwner, CompressionType: NONE}; wait for DescribeChannel to report ChannelStatus=ACTIVE.
3. From two threads released by a barrier, call UpdateChannel(ChannelARN=<channel arn>, S3DestinationConfiguration={DataFreshnessInSeconds: 400}) and UpdateChannel(ChannelARN=<channel arn>, S3DestinationConfiguration={DataFreshnessInSeconds: 700}).
4. Observe both calls refused with error code NotImplemented (HTTP 501) instead of one HTTP 200 + ChannelDescription and one ResourceInUseException (HTTP 400).
5. Call DescribeChannel and observe ChannelStatus never entered UPDATING and no submitted DataFreshnessInSeconds was persisted.
6. Repeat with LoggingConfiguration={CloudWatchLogs:{Enabled:true, LogGroupName:<group>, LogStreamName:<per-racer value>}} and observe the same refusal, so the settled LogStreamName is never one racer's value.
7. Re-replay: kinesis.concurrent-channel-update-serializes-cleanly, kinesis.update-channel-concurrent-modification-conflict, kinesis.concurrent-update-channel-lost-update-detected.
