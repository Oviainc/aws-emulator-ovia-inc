Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.deploy-cfn-kinesis-stream-put-get-records kinesis.deploy-cfn-stream-encryption-enforced kinesis.deploy-cfn-stream-put-get-records-round-trip kinesis.deploy-kinesis-stream-cfn-behavioral-put-get kinesis.put-record-exceeding-max-record-size-rejected kinesis.put-record-oversized-data-rejected kinesis.put-record-oversized-payload-rejected-before-limit-increase kinesis.at-sequence-number-from-foreign-shard-rejected kinesis.get-records-exceeding-the-per-shard-rate-throttles-then-recovers kinesis.millis-behind-latest-tracks-the-clock-and-reads-do-not-advance-the-shard kinesis.list-shards-pagination-covers-all-shards-exactly-once kinesis.update-max-record-size-out-of-range-fails kinesis.put-records-preserves-submission-order-within-shard kinesis.put-records-preserves-per-shard-sequence-order kinesis.put-records-entries-align-with-partition-key-hash kinesis.put-records-exceeds-batch-limit-fails kinesis.put-records-partial-failure-reports-per-entry-error-codes kinesis.kinpartial-oversized-entry-rejects-whole-batch-and-stores-nothing kinesis.kinpartial-resend-failed-subset-appears-exactly-once kinesis.kinpartial-resend-preserves-per-partition-key-ack-order kinesis.kinpartial-throttled-entries-rejected-and-absent-past-checkpoint kinesis.kinpartial-whole-batch-resend-stores-acknowledged-records-twice kinesis.conc-limits-and-fanout-shard-record-rate-quota-throttles-tiny-records-rejected kinesis.delete-stream-removes-it kinesis.e2e-stream-produce-scale-consume-teardown kinesis.on-demand-stream-records-round-trip-across-every-open-shard kinesis.split-shard-new-starting-hash-key-outside-parent-range-rejected kinesis.add-tags-to-stream-reserved-aws-prefix-rejected kinesis.tag-resource-reserved-aws-prefix-rejected
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|DescribeStreamSummary|ResourceNotFoundException
Issue-Key: kinesis|PutRecord|InvalidArgumentException
Issue-Key: kinesis.put-record-oversized-data-rejected|run#12|AssertionError|assert code == "ValidationException", f"expected ValidationException for a 1 MiB+1 Data blob, got {code!r}"
Issue-Key: kinesis.put-record-oversized-payload-rejected-before-limit-increase|run#5|AssertionError|assert error.get("Code") == "ValidationException", f"expected ValidationException, got {error.get('Code')}"
Issue-Key: kinesis.at-sequence-number-from-foreign-shard-rejected|run#15|AssertionError|raise AssertionError(
Issue-Key: kinesis.get-records-exceeding-the-per-shard-rate-throttles-then-recovers|run#16|AssertionError|assert throttle_error is not None, (
Issue-Key: kinesis.millis-behind-latest-tracks-the-clock-and-reads-do-not-advance-the-shard|run#28|AssertionError|assert last_with_records["MillisBehindLatest"] > 0, (
Issue-Key: kinesis|ListShards|InvalidArgumentException
Issue-Key: kinesis.update-max-record-size-out-of-range-fails|run#4|KeyError|before = kinesis.describe_stream_summary(StreamARN=stream_arn)["StreamDescriptionSummary"][
Issue-Key: kinesis.put-records-preserves-submission-order-within-shard|run#12|KeyError|assert put_resp["FailedRecordCount"] == 0, (
Issue-Key: kinesis.put-records-preserves-per-shard-sequence-order|run#10|KeyError|assert resp["FailedRecordCount"] == 0, f"unexpected failures: {resp['Records']}"
Issue-Key: kinesis.put-records-entries-align-with-partition-key-hash|run#10|KeyError|assert resp["FailedRecordCount"] == 0, (
Issue-Key: kinesis.put-records-exceeds-batch-limit-fails|run#6|KeyError|assert control_resp["FailedRecordCount"] == 0, (
Issue-Key: kinesis.put-records-partial-failure-reports-per-entry-error-codes|run#9|KeyError|assert control["FailedRecordCount"] == 0, (
Issue-Key: kinesis.kinpartial-oversized-entry-rejects-whole-batch-and-stores-nothing|run#7|KeyError|assert control["FailedRecordCount"] == 0, (
Issue-Key: kinesis.kinpartial-resend-failed-subset-appears-exactly-once|run#9|KeyError|assert control["FailedRecordCount"] == 0, (
Issue-Key: kinesis.kinpartial-resend-preserves-per-partition-key-ack-order|run#10|KeyError|assert control["FailedRecordCount"] == 0, (
Issue-Key: kinesis.kinpartial-throttled-entries-rejected-and-absent-past-checkpoint|run#10|KeyError|assert control["FailedRecordCount"] == 0, (
Issue-Key: kinesis.kinpartial-whole-batch-resend-stores-acknowledged-records-twice|run#9|KeyError|assert control["FailedRecordCount"] == 0, (
Issue-Key: kinesis.conc-limits-and-fanout-shard-record-rate-quota-throttles-tiny-records-rejected|run#14|KeyError|assert ctl['FailedRecordCount'] == 0, f'control batch refused entries: {ctl["Records"]!r}'
Issue-Key: kinesis.delete-stream-removes-it|run#5|KeyError|assert put["FailedRecordCount"] == 0, f"PutRecords failed some records: {put!r}"
Issue-Key: kinesis.e2e-stream-produce-scale-consume-teardown|run#17|KeyError|assert put_pre["FailedRecordCount"] == 0, (
Issue-Key: kinesis.on-demand-stream-records-round-trip-across-every-open-shard|run#7|KeyError|assert put["FailedRecordCount"] == 0, f"PutRecords failed records: {put['Records']!r}"
Issue-Key: kinesis.split-shard-new-starting-hash-key-outside-parent-range-rejected|run#12|KeyError|assert resp["FailedRecordCount"] == 0, f"put_records failed: {resp!r}"
Issue-Key: kinesis.add-tags-to-stream-reserved-aws-prefix-rejected|run#4|AssertionError|raise AssertionError(
Issue-Key: kinesis.tag-resource-reserved-aws-prefix-rejected|_client_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: d5c75e29b1e90f1499e967e91f7af40c6cd43d58

# kinesis: 9 net-new root causes — CloudFormation AWS::Kinesis::Stream is a no-op, PutRecord's 1 MiB limit is mis-enforced, shard-scoped sequence numbers leak, GetRecords never throttles and always reports MillisBehindLatest 0, ListShards refuses a consistent StreamName+NextToken, DescribeStreamSummary omits MaxRecordSizeInKiB, PutRecords omits FailedRecordCount, reserved 'aws:' tag keys are accepted

> _Note: this issue was identified against the code base at commit `d5c75e29b1e90f1499e967e91f7af40c6cd43d58`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Nine net-new root causes in the kinesis emulator (Cause 1 of 9 … Cause 9 of 9). All nine are grounded in failing behavioral checks of the `kinesis` service; each cause below names its own failing check ids. Causes that are already covered by a filed issue (the scaffold-stub issues the suite filed for CreateChannel, SplitShard, MergeShards, UpdateShardCount, UpdateStreamMode, IncreaseStreamRetentionPeriod, StartStreamEncryption, PutResourcePolicy, GetResourcePolicy, ListTagsForResource, RemoveTagsFromStream, DescribeLimits, DescribeAccountSettings, UpdateAccountSettings, EnableEnhancedMonitoring, UpdateMaxRecordSize, ListStreamConsumers) are deliberately NOT repeated here.

=== Cause 1 of 9: a CloudFormation-deployed AWS::Kinesis::Stream is never actually created ===
Failing checks: kinesis.deploy-cfn-kinesis-stream-put-get-records, kinesis.deploy-cfn-stream-encryption-enforced, kinesis.deploy-cfn-stream-put-get-records-round-trip, kinesis.deploy-kinesis-stream-cfn-behavioral-put-get.
AWS: a stack whose template declares `Type: AWS::Kinesis::Stream` with `Name`/`ShardCount`/`RetentionPeriodHours` only reaches CREATE_COMPLETE after the stream exists; `Ref` returns the stream name and `!GetAtt Stream.Arn` the stream ARN of a stream that DescribeStreamSummary then answers for (StreamStatus eventually ACTIVE), and DeleteStack removes it so DescribeStreamSummary answers ResourceNotFoundException. Recorded behavior only (the resource contract lives in the CloudFormation resource-type registry, not in the kinesis Smithy model).
Emulator: the stack reaches CREATE_COMPLETE and even returns both Outputs (a syntactically correct ARN of the form arn:aws:<partition>:kinesis:<region>:<account>:stream/<name>), but no stream is registered in kinesis state: the very next call fails with HTTP 400 `ResourceNotFoundException` and message `stream <stream-arn-or-name> not found under account <account-id>` (fixed text `stream `/` not found under account `, variable parts = the ARN/name from the template parameter and the caller's account id). Every one of the four deploy checks dies there, so nothing downstream (put/get round-trip, encryption enforcement, tag application, teardown) is graded.
Why: Infrastructure-as-code is the primary way streams are created. A client gets CREATE_COMPLETE and a usable-looking ARN, then every data-plane and control-plane call against that ARN fails as if the stream never existed — the deploy appears to succeed and the application breaks at first use, with no failure signal on the stack.
Fix: the CloudFormation resource provisioner for `AWS::Kinesis::Stream` — the resource-type handler in the cloudformation service crate that executes a stack's resources — must create the stream in kinesis state (name, shard count, retention, stream-mode, encryption) before the stack is reported CREATE_COMPLETE, derive `Ref`/`GetAtt Arn` from that created stream, and delete it on stack deletion. The kinesis CreateStream/DeleteStream handlers themselves are fine when called directly.

=== Cause 2 of 9: PutRecord's per-record size limit uses the wrong error class and charges PartitionKey against the budget ===
Failing checks: kinesis.put-record-exceeding-max-record-size-rejected, kinesis.put-record-oversized-data-rejected, kinesis.put-record-oversized-payload-rejected-before-limit-increase.
AWS: Smithy models PutRecordInput$Data as `Data` (blob, @length max 10485760) and PutRecordInput$PartitionKey as `PartitionKey` (@length 1..256), and PutRecord's modeled errors are AccessDenied/DryRunOperation/InvalidArgument/KMS*/ProvisionedThroughputExceeded/ResourceNotFound — the 1 MiB record limit is NOT in the model, it is enforced by the front-end request validator above the operation's error surface. Recorded behavior (verified us-west-2, carried in the check header): `Data` of exactly 1048576 bytes together with a PartitionKey is ACCEPTED, and 1048577 bytes is refused HTTP 400 `ValidationException` with message `1 validation error detected: Value at 'data' failed to satisfy constraint: Member must have length less than or equal to 1048576` — fixed text apart from the limit, which is the stream's effective ceiling (MaxRecordSizeInKiB × 1024, default 1048576). So the budget is charged against `Data` ALONE.
Emulator: it adds the PartitionKey's bytes to the Data length and refuses with HTTP 400 `InvalidArgumentException` and an invented message `the record's data and partition key total <n> bytes, past the <limit> bytes one record of this stream may carry`. Two consequences: a perfectly valid exactly-1-MiB record plus a 1-byte partition key is REFUSED (the at-limit control of kinesis.put-record-exceeding-max-record-size-rejected dies with that error on the happy path), and the over-limit case reports the wrong error code and message (both oversized-payload checks assert ValidationException and got InvalidArgumentException).
Why: producers that fill records to the documented 1 MiB boundary are rejected outright against the emulator while working against AWS, and retry logic keyed on ValidationException versus the retryable/argument error classes takes the wrong branch.
Fix: the PutRecord handler's size validation (the operation's validation step, before the append to shard state) must measure `Data` only, compare it against the stream's MaxRecordSizeInKiB-derived ceiling, and raise the front-end ValidationException with AWS's wording rather than the operation's modeled InvalidArgumentException.

=== Cause 3 of 9: sequence numbers are a global counter, so GetShardIterator accepts another shard's sequence number ===
Failing check: kinesis.at-sequence-number-from-foreign-shard-rejected.
AWS: Kinesis sequence numbers are shard-scoped (their high bits encode the shard). GetShardIterator with ShardIteratorType=AT_SEQUENCE_NUMBER and a StartingSequenceNumber minted on a DIFFERENT shard of the same stream is refused HTTP 400 `InvalidArgumentException` (Smithy: GetShardIterator declares InvalidArgumentException; recorded behavior confirms it is InvalidArgumentException, not ResourceNotFoundException or ValidationException).
Emulator: the call is accepted and returns a usable iterator for the foreign shard (the replay shows shard shardId-000000000000's sequence number accepted for shardId-000000000001). The control arm — the same sequence number on its own shard — works, so the value is well-formed; the emulator simply allocates sequence numbers from one global counter and never binds them to a shard.
Why: a consumer that mixes up checkpoints between shards gets no error and silently reads from the wrong shard (or, once the global counter also indexes records, reads another shard's records) — a data-correctness failure with no signal anywhere.
Fix: sequence-number allocation and resolution in the stream's shard state must be per-shard (or carry the shard identity), and the GetShardIterator handler must refuse an AT_SEQUENCE_NUMBER/AFTER_SEQUENCE_NUMBER value that does not belong to the requested shard with InvalidArgumentException.

=== Cause 4 of 9: GetRecords never throttles, so the per-shard read quota is unenforced ===
Failing check: kinesis.get-records-exceeding-the-per-shard-rate-throttles-then-recovers.
AWS: GetRecords is limited to 5 transactions per second per shard; calls past that are refused HTTP 400 `ProvisionedThroughputExceededException` (Smithy: GetRecords declares ProvisionedThroughputExceededException) and succeed again once the rate drops.
Emulator: 120 concurrent GetRecords calls against ONE shard completed in 0.19 s and were ALL accepted; no ProvisionedThroughputExceededException is ever raised.
Why: consumer libraries (KCL and hand-rolled pollers) exist to back off on this exact error. Against the emulator the back-off path is never exercised, and code that would be throttled in production looks correct in test.
Fix: per-shard read-rate accounting in the GetRecords handler/shard state, refusing over-quota calls with the modeled ProvisionedThroughputExceededException.

=== Cause 5 of 9: GetRecords always reports MillisBehindLatest 0 ===
Failing check: kinesis.millis-behind-latest-tracks-the-clock-and-reads-do-not-advance-the-shard.
AWS: Smithy models GetRecordsOutput$MillisBehindLatest as `MillisBehindLatest` (long, range min 0), documented as the number of milliseconds the returned batch is behind the tip of the stream. A response carrying a record written seconds earlier reports a strictly positive lag derived from that record's Kinesis arrival time.
Emulator: the response carrying the LAST record reported MillisBehindLatest 0 although the record was written seconds earlier — the member is a constant 0 rather than a clock-derived value.
Why: consumer lag is the single metric operators alarm on; a constant 0 makes every backlog invisible, and consumers that decide whether to catch up based on MillisBehindLatest never do.
Fix: the GetRecords handler must compute MillisBehindLatest from the arrival timestamp stored with the last record it returned and the current clock, in the operation's output construction.

=== Cause 6 of 9: ListShards refuses StreamName alongside a NextToken instead of cross-checking it ===
Failing check: kinesis.list-shards-pagination-covers-all-shards-exactly-once.
AWS: the NextToken carries the stream's identity and the service CROSS-CHECKS any identifier sent with it. Recorded behavior (verified us-west-2, carried in the check header): StreamName alongside a NextToken is ACCEPTED when it names the stream the token was minted for, and only a MISMATCH is refused, HTTP 400 `InvalidArgumentException` with message `NextToken does not match the provided StreamName.` (fixed text, no variable parts). The Smithy doc's "you cannot specify this parameter if you specify NextToken" is prose on the member, not the service's refusal rule.
Emulator: any NextToken+StreamName pair is refused outright — HTTP 400 `InvalidArgumentException`, message `StreamName cannot be specified together with NextToken, which already identifies what is being listed and where the listing resumes` — so the ordinary paging loop (first page with StreamName, subsequent pages with StreamName plus the token) cannot complete and shard discovery breaks at the second page.
Why: shard discovery is the first thing every consumer does; an SDK paginator that keeps the original request parameters and adds the token — the default behavior of several SDKs — cannot list the shards of a stream with more shards than one page at all.
Fix: the ListShards handler's input validation must accept a StreamName/StreamARN that agrees with the token's stream identity, refuse only a disagreement with AWS's message, and keep paging from the token.

=== Cause 7 of 9: DescribeStreamSummary omits MaxRecordSizeInKiB ===
Failing check: kinesis.update-max-record-size-out-of-range-fails.
AWS: Smithy models StreamDescriptionSummary$MaxRecordSizeInKiB (integer, range 1024..10240) — "The maximum record size of a single record in kibibyte (KiB) that you can write to, and read from a stream". Real DescribeStreamSummary returns it for every stream (1024 on a stream that never called UpdateMaxRecordSize).
Emulator: the member is absent from StreamDescriptionSummary, so a client reading it raises `KeyError: 'MaxRecordSizeInKiB'`; the check dies on its very first call, before it can reach UpdateMaxRecordSize at all.
Why: a producer cannot discover the record ceiling it must size its batches against, and any code that reads the field crashes on a 200 response. (The separate UpdateMaxRecordSize scaffold stub is covered by its own already-filed issue; this cause is the read path, which is implemented and incomplete.)
Fix: the DescribeStreamSummary handler's output construction must populate `max_record_size_in_ki_b` from the stream's stored ceiling, defaulting to 1024 KiB.

=== Cause 8 of 9: PutRecords omits FailedRecordCount on a clean batch, blocking 15 further kinesis checks ===
Failing checks: kinesis.put-records-preserves-submission-order-within-shard, kinesis.put-records-preserves-per-shard-sequence-order, kinesis.put-records-entries-align-with-partition-key-hash, kinesis.put-records-exceeds-batch-limit-fails, kinesis.put-records-partial-failure-reports-per-entry-error-codes, kinesis.kinpartial-oversized-entry-rejects-whole-batch-and-stores-nothing, kinesis.kinpartial-resend-failed-subset-appears-exactly-once, kinesis.kinpartial-resend-preserves-per-partition-key-ack-order, kinesis.kinpartial-throttled-entries-rejected-and-absent-past-checkpoint, kinesis.kinpartial-whole-batch-resend-stores-acknowledged-records-twice, kinesis.conc-limits-and-fanout-shard-record-rate-quota-throttles-tiny-records-rejected, kinesis.delete-stream-removes-it, kinesis.e2e-stream-produce-scale-consume-teardown, kinesis.on-demand-stream-records-round-trip-across-every-open-shard, kinesis.split-shard-new-starting-hash-key-outside-parent-range-rejected.
AWS: real PutRecords returns `FailedRecordCount` on EVERY 200 response, valued 0 when every entry was stored (Smithy models PutRecordsOutput$FailedRecordCount as PositiveIntegerObject, optional, range min 1 — the modeled range says nothing about the all-succeeded case; recorded behavior only).
Emulator: the member is elided when nothing failed, so each of the 15 checks above dies in or just after setup with `KeyError: 'FailedRecordCount'` and none of their real assertions (ordering, per-entry error codes, partial-failure resend semantics, shard round-trip, teardown) are graded at all.
Why: the canonical SDK PutRecords idiom reads FailedRecordCount unconditionally to decide whether to retry the failed subset, so a fully successful write crashes the producer.
Fix: the PutRecords output construction — services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/put_records.rs — must always populate the count, including Some(0), and the output serializer must emit a zero rather than eliding it.
Relationship to an already-filed issue: open issue 20261026T094039Z-kinesis-putrecords-omits-failedrecordcount-from-a-fully-successful.md describes this same defect, but its Issue-Trace-Ids/Issue-Key carry only firehose__glue__kinesis__s3.kds-put-records-accepts-the-stream-arn-in-place-of-the-stream-name, so a fixer closing it would never re-replay the 15 kinesis checks listed above. This cause exists to carry those replay keys; it must be closed by the same single fix, and whoever fixes either issue should close both.

=== Cause 9 of 9: reserved 'aws:' tag keys are accepted on both tagging surfaces ===
Failing checks: kinesis.add-tags-to-stream-reserved-aws-prefix-rejected, kinesis.tag-resource-reserved-aws-prefix-rejected.
AWS: a tag key beginning with the reserved `aws:` prefix is refused on both tagging paths with HTTP 400 `InvalidArgumentException` and message `Some tag keys in the request started with 'aws:'. System tags cannot be modified.` (fixed text, no variable parts; Smithy: both AddTagsToStream and TagResource declare InvalidArgumentException), and the resource's tag set is left unchanged.
Emulator: both calls SUCCEED — TagResource answers HTTP 200 with an empty body for Tags={"aws:<key>": "..."}, and AddTagsToStream likewise stores the reserved key as an ordinary user tag (the replay reports `AddTagsToStream('aws:sim2real-behav') on stream <name> unexpectedly succeeded`). Both surfaces, keyed by StreamName and by ResourceARN respectively, miss the rule.
Why: this is the over-permissive direction — tooling that would be refused in production silently writes into AWS's reserved namespace against the emulator, and tag-based cost allocation/automation that keys on system tags reads values no real account can produce.
Fix: a shared reserved-prefix check in the tag-validation path used by both the AddTagsToStream and TagResource handlers, refusing with the modeled InvalidArgumentException before any state mutation.

## Evidence
All evidence is from the replayed behavioral suite (simparity-behavioral-ga-prod-v1), 25/231 passed.

Cause 1 — expected: DescribeStreamSummary answers for the stream the stack created. Actual:
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeStreamSummary operation: stream arn:aws:kinesis:us-west-2:000000000000:stream/<name> not found under account 000000000000
  (identical on all four deploy checks, after the stack reported CREATE_COMPLETE and returned both Outputs).

Cause 2 — expected: Data of exactly 1048576 bytes + a PartitionKey is accepted; 1048577 bytes is refused ValidationException "1 validation error detected: Value at 'data' failed to satisfy constraint: Member must have length less than or equal to 1048576". Actual, on the AT-LIMIT control call:
  botocore.errorfactory.InvalidArgumentException: An error occurred (InvalidArgumentException) when calling the PutRecord operation: the record's data and partition key total 1048577 bytes, past the 1048576 bytes one record of this stream may carry
  and on the over-limit arms: AssertionError: expected ValidationException for a 1 MiB+1 Data blob, got 'InvalidArgumentException' / AssertionError: expected ValidationException, got InvalidArgumentException.

Cause 3 — AssertionError: GetShardIterator accepted shard shardId-000000000000's sequence number <seq> for shard shardId-000000000001 and returned an iterator; expected InvalidArgumentException.

Cause 4 — AssertionError: 120 concurrent GetRecords calls against ONE shard in 0.19s were all accepted; expected ProvisionedThroughputExceededException — GetRecords is documented at 5 transactions per second per shard.

Cause 5 — AssertionError: the response carrying the LAST record reported MillisBehindLatest 0; that record was written seconds earlier, so the reported lag must be strictly positive.

Cause 6 — botocore.errorfactory.InvalidArgumentException: An error occurred (InvalidArgumentException) when calling the ListShards operation: StreamName cannot be specified together with NextToken, which already identifies what is being listed and where the listing resumes. Expected: the consistent pair is accepted and paging continues; only a mismatched pair is refused with "NextToken does not match the provided StreamName."

Cause 7 — KeyError: 'MaxRecordSizeInKiB' raised at describe_stream_summary(StreamARN=...)["StreamDescriptionSummary"]["MaxRecordSizeInKiB"] on a freshly created PROVISIONED stream.

Cause 8 — KeyError: 'FailedRecordCount' on a 200 put_records response, in all 15 checks listed for that cause.

Cause 9 — AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}} (TagResource with Tags={'aws:<key>': 'blocked'}), and AssertionError: AddTagsToStream('aws:sim2real-behav') on stream <name> unexpectedly succeeded.

## Steps to Reproduce
Cause 1: cloudformation.create_stack with a template declaring `Resources: Stream: {Type: AWS::Kinesis::Stream, Properties: {Name: !Ref StreamName, ShardCount: 1, RetentionPeriodHours: 24}}` and Outputs Ref/GetAtt Arn; poll describe_stacks until CREATE_COMPLETE; then kinesis.describe_stream_summary(StreamARN=<the StreamArn output>) → ResourceNotFoundException instead of a stream description.
Cause 2: create a 1-shard PROVISIONED stream; kinesis.put_record(StreamName=n, Data=b'u'*1048576, PartitionKey='k') → refused InvalidArgumentException (AWS accepts); then Data=b'o'*1048577 → InvalidArgumentException (AWS: ValidationException with the 'data' length message).
Cause 3: create a 2-shard stream; put a record, note its ShardId A and SequenceNumber S; get_shard_iterator(ShardId=A, ShardIteratorType='AT_SEQUENCE_NUMBER', StartingSequenceNumber=S) succeeds (control); repeat with ShardId=B → an iterator is returned instead of InvalidArgumentException.
Cause 4: create a 1-shard stream, obtain one shard iterator, issue ~120 get_records calls concurrently inside one second → all 200 (AWS: ProvisionedThroughputExceededException past 5 TPS/shard).
Cause 5: put a record, wait a few seconds, get_shard_iterator TRIM_HORIZON and get_records → MillisBehindLatest is 0 on the response that carries that record.
Cause 6: create a 3-shard stream; list_shards(StreamName=n, MaxResults=1); then list_shards(StreamName=n, NextToken=<token>, MaxResults=1) → InvalidArgumentException instead of the next page.
Cause 7: create a PROVISIONED stream; describe_stream_summary(StreamARN=arn)['StreamDescriptionSummary'] → no MaxRecordSizeInKiB key.
Cause 8: put_records(StreamName=n, Records=[{'Data': b'x', 'PartitionKey': 'pk'}]) → 200 response with Records and EncryptionType but no FailedRecordCount.
Cause 9: tag_resource(ResourceARN=<stream arn>, Tags={'aws:anything': 'blocked'}) → 200; add_tags_to_stream(StreamName=n, Tags={'aws:anything': 'blocked'}) → 200; both must be InvalidArgumentException "Some tag keys in the request started with 'aws:'. System tags cannot be modified." and must leave the tag set unchanged.
