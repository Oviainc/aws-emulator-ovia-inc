Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis.ddbkds-cross-region-stream-arn-settles-to-enable-failed dynamodb__kinesis.ddbkds-enable-failed-destination-is-excluded-by-the-active-filter
Issue-Key: dynamodb__kinesis.ddbkds-cross-region-stream-arn-settles-to-enable-failed|run#12|AssertionError|assert failed[0].get("DestinationStatusDescription"), (
Issue-Key: dynamodb__kinesis.ddbkds-enable-failed-destination-is-excluded-by-the-active-filter|run#14|AssertionError|assert failed[0].get("DestinationStatusDescription"), (
Issue-Commit: d8469fdbee4aefb676d6a1b6581464fb2b4efb05

# dynamodb__kinesis: an ENABLE_FAILED Kinesis streaming destination is described without DestinationStatusDescription, so the reason the enable failed is never served

> _Note: this issue was identified against the code base at commit `d8469fdbee4aefb676d6a1b6581464fb2b4efb05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeKinesisStreamingDestination

## Description
Cause 1 of 1: DescribeKinesisStreamingDestination omits `DestinationStatusDescription` for a destination that has settled to ENABLE_FAILED.

AWS: Smithy — `DescribeKinesisStreamingDestination` returns `KinesisDataStreamDestinations` (list of `KinesisDataStreamDestination`), whose members are `StreamArn`, `DestinationStatus` (enum), `DestinationStatusDescription` (String, documented "The human-readable string that corresponds to the replica status") and `ApproximateCreationDateTimePrecision`. Recorded behavior: EnableKinesisStreamingDestination against a stream the table cannot actually stream to — a well-formed ARN naming no existing stream, or a stream in ANOTHER region — is ACCEPTED (response DestinationStatus `ENABLING`), the destination then settles to `DestinationStatus` = `ENABLE_FAILED`, and that entry is served WITH a non-empty `DestinationStatusDescription` carrying the human-readable reason. The description's text is service-authored and names the stream/condition; the requirement is that the member is present and non-empty on every ENABLE_FAILED entry, not any one run's wording.

Emulator: the destination does settle to ENABLE_FAILED and is listed, but the entry is served as exactly `{'StreamArn': '<arn>', 'DestinationStatus': 'ENABLE_FAILED'}` — `DestinationStatusDescription` is absent. Same omission for both failure conditions (absent stream and cross-region stream), so it is one serializer/state gap, not two.

Why: ENABLE_FAILED is a terminal state with no other channel for its cause — a client or operator has no way to learn WHY change-data-capture never started (wrong region vs deleted stream vs permissions), and code that reads the description to decide whether to retry or re-point the destination reads `None`.

Fix: the DescribeKinesisStreamingDestination handler / the streaming-destination state that records the failure — the reason must be stored when the destination transitions to ENABLE_FAILED and serialized into `DestinationStatusDescription`. Distinct from the open issue 20261103T065250Z-dynamodb-kinesis-streaming-destinations-never-settle-5-checks.md (destinations not settling at all) and 20261027T001720Z-dynamodb-kinesis-enablekinesisstreamingdestination-omits-enablekinesisstreamingconfiguration-from-its-response.md (a missing member on the ENABLE response): here the settle to ENABLE_FAILED does happen and the describe is served — only the description member is missing.

Traces: dynamodb__kinesis.ddbkds-cross-region-stream-arn-settles-to-enable-failed, dynamodb__kinesis.ddbkds-enable-failed-destination-is-excluded-by-the-active-filter

## Evidence
Both from the behavioral suite replay against this checkout (service heading `### dynamodb__kinesis`), both verdict mismatch, both failing on the same assertion.

dynamodb__kinesis.ddbkds-cross-region-stream-arn-settles-to-enable-failed:
  expected: the ENABLE_FAILED entry carries a non-empty DestinationStatusDescription
  actual:   AssertionError: the failed destination must carry a DestinationStatusDescription, got [{'StreamArn': 'arn:aws:kinesis:us-east-1:<account>:stream/<token>-peer-cdc', 'DestinationStatus': 'ENABLE_FAILED'}]

dynamodb__kinesis.ddbkds-enable-failed-destination-is-excluded-by-the-active-filter:
  expected: the ENABLE_FAILED entry carries a non-empty DestinationStatusDescription
  actual:   AssertionError: an ENABLE_FAILED destination must carry a DestinationStatusDescription, got [{'StreamArn': 'arn:aws:kinesis:us-west-2:<account>:stream/<token>-absent', 'DestinationStatus': 'ENABLE_FAILED'}]

(The ARN, account and random token differ per run and are not part of the requirement; the absence of the DestinationStatusDescription member is. In both cases the preceding steps passed: the enable was accepted with DestinationStatus ENABLING and the destination did settle to ENABLE_FAILED.)

## Steps to Reproduce
1. Create an on-demand DynamoDB table and wait ACTIVE; create a Kinesis stream in the same region.
2. Condition A (absent stream): call EnableKinesisStreamingDestination(TableName=<table>, StreamArn=<a well-formed ARN in this region naming a stream that does not exist>). Condition B (cross-region): create a stream in a different region and call EnableKinesisStreamingDestination with that peer-region ARN.
3. Both calls must return DestinationStatus='ENABLING'.
4. Poll DescribeKinesisStreamingDestination(TableName=<table>) until the entry for that ARN reaches DestinationStatus='ENABLE_FAILED'.
5. Assert the ENABLE_FAILED entry carries a non-empty DestinationStatusDescription. The emulator serves only StreamArn + DestinationStatus.
