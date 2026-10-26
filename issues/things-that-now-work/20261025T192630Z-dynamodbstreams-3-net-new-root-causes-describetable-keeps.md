Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodbstreams.disable-closes-shards-and-reenable-mints-new-stream dynamodbstreams.foreign-sequence-number-rejected dynamodbstreams.stream-record-creation-time-tracks-the-live-clock
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams.disable-closes-shards-and-reenable-mints-new-stream|run#17|AssertionError|assert "StreamSpecification" not in described, (
Issue-Key: dynamodbstreams.foreign-sequence-number-rejected|run#9|AssertionError|assert code == "ValidationException", (
Issue-Key: dynamodbstreams.stream-record-creation-time-tracks-the-live-clock|_assert_whole_second_timestamp#26|AssertionError|assert value.microsecond == 0, (
Issue-Commit: 450f7a190afe0db2852dff5128e985260e643504

# dynamodbstreams: 3 net-new root causes — DescribeTable keeps a disabled StreamSpecification, a foreign AT_SEQUENCE_NUMBER is TrimmedDataAccessException instead of ValidationException, and ApproximateCreationDateTime is not floored to the second

> _Note: this issue was identified against the code base at commit `450f7a190afe0db2852dff5128e985260e643504`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetShardIterator

## Description
Three independent, net-new root causes in the DynamoDB Streams surface. All three were reached by checks that DID get real answers from the emulator (7/11 dynamodbstreams checks pass, records do flow), so the open issue 20261022T170000Z-dynamodbstreams-item-writes-publish-no-stream-records.md does not cover any of them. Each cause must be closed before submitting.

=== Cause 1 of 3: DescribeTable keeps StreamSpecification once the stream has been disabled ===
AWS: recorded behavior only. The Smithy model declares TableDescription.StreamSpecification as an optional member and says nothing about its removal, so the requirement comes from the recording. MEASURED on real AWS (us-west-2, 2026-09-10): after UpdateTable{StreamSpecification:{StreamEnabled:false}} walks the stream ENABLED -> DISABLING -> DISABLED, DescribeTable omits the StreamSpecification key from TableDescription ENTIRELY, while KEEPING LatestStreamArn pointing at the now-disabled stream. The general rule: a table whose stream is off has no StreamSpecification member in its description at all.
Emulator: DescribeTable still carries the member, with StreamEnabled false — the replay asserted `"StreamSpecification" not in described` and got `{'StreamEnabled': False}`. The preceding legs (DISABLED status, retained StreamViewType, all shards retained and each closed by a digit EndingSequenceNumber) passed, so this is a projection defect and not a lifecycle defect.
Why: clients and IaC tools decide "does this table stream?" by the PRESENCE of StreamSpecification, not by reading StreamEnabled out of it. A present-but-false spec makes CloudFormation/Terraform drift detection report a permanent diff and re-issue the disable forever, and makes SDK code that branches on `'StreamSpecification' in table` believe a disabled table is still streaming.
Fix: the TableDescription projection/serializer behind DescribeTable in the dynamodb service — omit the StreamSpecification member when the table's stream is off instead of emitting StreamEnabled:false.
Traces: dynamodbstreams.disable-closes-shards-and-reenable-mints-new-stream

=== Cause 2 of 3: GetShardIterator AT_SEQUENCE_NUMBER on a sequence number foreign to the shard answers the wrong error class ===
AWS: MEASURED on real AWS (us-west-2, 2026-09-10). A well-formed 21-digit SequenceNumber that does not belong to the named shard is refused 400 ValidationException with the message "Invalid SequenceNumber for the shard" — that message is entirely FIXED text, it interpolates nothing from the request. AT_SEQUENCE_NUMBER with no SequenceNumber at all is refused 400 ValidationException "One or more parameter values were invalid: SequenceNumber cannot be null", whose only variable part is the member name (SequenceNumber) taken from the request's missing member; the rest is fixed text. Note the deliberate divergence from the Smithy model, which declares GetShardIterator's errors as InternalServerError, ResourceNotFoundException and TrimmedDataAccessException and does NOT model ValidationException — the recording is authoritative here. Smithy documents TrimmedDataAccessException as "The operation attempted to read past the oldest stream record in a shard" (the 24-hour retention trim point), which is a different condition entirely and must stay reserved for it.
Emulator: the foreign 21-zero sequence number is refused as 'TrimmedDataAccessException' instead of ValidationException (message therefore also wrong). The second leg — AT_SEQUENCE_NUMBER with the SequenceNumber omitted — sits after the failed assertion and was never graded, so it is unverified but is part of the same validation rule and must hold once this is fixed. The positive control (the shard's OWN sequence number) is likewise downstream and ungraded.
Why: the two codes mean opposite things to a consumer. TrimmedDataAccessException means "your checkpoint fell behind the 24 h retention window", and consumer frameworks (KCL-style pollers, Lambda event-source mappings) respond by restarting the shard at TRIM_HORIZON. So a client that resumes from a corrupt or cross-shard checkpoint silently REPROCESSES the shard's entire change history instead of getting a non-retryable bad-argument refusal it would surface.
Fix: the SequenceNumber validation in the dynamodbstreams GetShardIterator handler — classify a well-formed sequence number that is not in the named shard, and a missing SequenceNumber for AT_SEQUENCE_NUMBER/AFTER_SEQUENCE_NUMBER, as ValidationException with the messages above; keep TrimmedDataAccessException only for a position older than the shard's trim point.
Traces: dynamodbstreams.foreign-sequence-number-rejected

=== Cause 3 of 3: StreamRecord.ApproximateCreationDateTime is not rounded down to the closest second ===
AWS: Smithy: StreamRecord.ApproximateCreationDateTime is documented as "The approximate date and time when the stream record was created, in UNIX epoch time format and rounded down to the closest second." The requirement is therefore that the value carries NO sub-second component — its microsecond part is always 0, whatever the write's actual sub-second phase.
Emulator: the INSERT record came back with a sub-second component (microsecond=134000 on the replayed record), i.e. the emulator stamps the host wall clock straight through without flooring it to the second. The particular instant in that run is a per-run value and is not the requirement; the requirement is microsecond == 0.
Why: the field is defined as integral epoch seconds, so clients bucket, dedupe and compare records on it, and SDKs/tools re-serialize it as an integer epoch. Against the emulator, a record's timestamp never equals the floored value AWS would have produced, equality and ordering comparisons against second-granular checkpoints mis-sort, and anything re-serializing to an int silently truncates differently from the service.
Fix: the stream-record construction/projection in the dynamodbstreams record state — floor ApproximateCreationDateTime to a whole second when the record is appended (and keep it stamped at WRITE time, not read time).
Traces: dynamodbstreams.stream-record-creation-time-tracks-the-live-clock

## Evidence
Cause 1 — check dynamodbstreams.disable-closes-shards-and-reenable-mints-new-stream:
  Expected (AWS, measured us-west-2 2026-09-10): DescribeTable(TableName=<disabled-stream table>)["Table"] has NO "StreamSpecification" key; "LatestStreamArn" still present and equal to the disabled stream's ARN.
  Actual (emulator): AssertionError: DescribeTable drops StreamSpecification once the stream is off, got {'StreamEnabled': False}

Cause 2 — check dynamodbstreams.foreign-sequence-number-rejected:
  Expected (AWS, measured us-west-2 2026-09-10): GetShardIterator(StreamArn=<arn>, ShardId=<shard holding the record>, ShardIteratorType="AT_SEQUENCE_NUMBER", SequenceNumber="000000000000000000000") -> HTTP 400, code ValidationException, message "Invalid SequenceNumber for the shard" (fixed text, nothing interpolated).
  Actual (emulator): AssertionError: AT_SEQUENCE_NUMBER on a sequence number foreign to the shard must be ValidationException, got 'TrimmedDataAccessException'
  Smithy for contrast: GetShardIterator errors = InternalServerError(500), ResourceNotFoundException(400), TrimmedDataAccessException(400); TrimmedDataAccessException doc = "The operation attempted to read past the oldest stream record in a shard" (24 h retention).
  Ungraded but part of the same rule: GetShardIterator with ShardIteratorType="AT_SEQUENCE_NUMBER" and SequenceNumber omitted must be 400 ValidationException "One or more parameter values were invalid: SequenceNumber cannot be null".

Cause 3 — check dynamodbstreams.stream-record-creation-time-tracks-the-live-clock:
  Expected (Smithy doc: "rounded down to the closest second"): record["dynamodb"]["ApproximateCreationDateTime"].microsecond == 0.
  Actual (emulator): AssertionError: ... the INSERT record reported 2026-10-25T19:20:12.134000+00:00 with microsecond=134000. A target stamping datetime.now() straight through lands here.
  (The instant itself is a per-run value; only microsecond == 0 is asserted as the requirement.)

## Steps to Reproduce
Cause 1:
1. CreateTable with StreamSpecification{StreamEnabled:true, StreamViewType:NEW_AND_OLD_IMAGES}; wait for the stream to be usable; PutItem one record and drain it.
2. UpdateTable(TableName=t, StreamSpecification={"StreamEnabled": false}); poll DescribeStream until StreamStatus == DISABLED.
3. DescribeTable(TableName=t) -> emulator's Table still contains StreamSpecification {'StreamEnabled': False}; AWS omits the key.

Cause 2:
1. CreateTable with StreamSpecification{StreamEnabled:true, StreamViewType:NEW_IMAGE}; wait for the stream to be usable.
2. PutItem {"pk":{"S":"sequence-item"},"v":{"N":"7"}}; drain every shard from TRIM_HORIZON to learn which shard holds it and that record's SequenceNumber.
3. GetShardIterator(StreamArn, ShardId=<that shard>, ShardIteratorType="AT_SEQUENCE_NUMBER", SequenceNumber="0"*21) -> emulator answers TrimmedDataAccessException; expected 400 ValidationException "Invalid SequenceNumber for the shard".
4. Repeat the call with SequenceNumber omitted -> expected 400 ValidationException "One or more parameter values were invalid: SequenceNumber cannot be null".
5. Control: the same call with the record's OWN SequenceNumber must succeed and return exactly that record.

Cause 3:
1. CreateTable with StreamSpecification{StreamEnabled:true, StreamViewType:NEW_IMAGE}; wait for the stream to be usable.
2. PutItem {"pk":{"S":"clock-item"},"v":{"N":"1"}}; drain one record from TRIM_HORIZON.
3. Inspect record["dynamodb"]["ApproximateCreationDateTime"] -> emulator returns a value with a non-zero microsecond component; AWS returns a whole second.
