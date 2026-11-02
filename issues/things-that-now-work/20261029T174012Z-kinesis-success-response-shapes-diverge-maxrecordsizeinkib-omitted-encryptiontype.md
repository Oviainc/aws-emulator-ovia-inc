Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-cbor-error-encoding-and-serialization-exception
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-cbor-error-encoding-and-serialization-exception kinesis-subscribe-to-shard-unregistered-consumer kinesis-put-record-get-records-round-trip kinesis-put-records-batch-success-shape
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|DescribeStreamSummary|2xx->2xx|$.StreamDescriptionSummary.MaxRecordSizeInKiB
Issue-Key: kinesis|PutRecord|2xx->2xx|$.EncryptionType
Issue-Key: kinesis|PutRecords|2xx->2xx|$.EncryptionType
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# kinesis: success-response shapes diverge — MaxRecordSizeInKiB omitted, EncryptionType emitted on unencrypted streams

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStreamSummary

## Description
Three net-new root causes in which a 200 response carries the wrong member set: one modeled member AWS always populates is absent, and one optional member AWS omits is emitted. None is marked ALREADY FILED or OWNED in the replay report. The open issue 20261029T145828Z-kinesis-3-net-new-root-causes-updatestreamrecorddistributionstrategy-is reports that DescribeStreamSummary never reports RecordDistributionStrategy; Cause 1 here is a different member of the same structure (MaxRecordSizeInKiB) and is not covered by it, though both live in the same serializer/state.

=== Cause 1 of 3: DescribeStreamSummary omits StreamDescriptionSummary.MaxRecordSizeInKiB ===
AWS: Smithy: DescribeStreamSummary -> StreamDescriptionSummary (required), whose member MaxRecordSizeInKiB targets MaxRecordSizeInKiB (integer, @range 1024..10240), documented "The maximum record size of a single record in kibibyte (KiB) that you can write to, and read from a stream." CreateStream takes the same member with the same 1024..10240 range, so every stream has a value for it; a stream created without specifying it reports the range's minimum, the 1 MiB default record size. AWS therefore returns MaxRecordSizeInKiB in the summary of an ordinary stream.
Emulator: the member is absent from StreamDescriptionSummary altogether — the stream's maximum record size is not stored or not serialized, so a client cannot read it for any stream.
Why: a producer that sizes its records from the stream's reported maximum (the whole point of the member, now that the limit is configurable per stream) reads None and must hardcode a limit; code that asserts the configured value after CreateStream(MaxRecordSizeInKiB=...) cannot confirm it took effect.
Fix: the kinesis stream state must carry the per-stream maximum record size (defaulting to 1024 KiB when CreateStream omits it) and the DescribeStreamSummary serializer must emit it.
Traces: kinesis-cbor-error-encoding-and-serialization-exception#task#2 (cluster also covers kinesis-subscribe-to-shard-unregistered-consumer#task#2).

=== Cause 2 of 3: PutRecord emits EncryptionType 'NONE' on a stream with no encryption, where AWS omits the member ===
AWS: Smithy: PutRecord output { ShardId (required), SequenceNumber (required), EncryptionType (enum NONE|KMS, optional, NOT required) }. For a write to a stream with no server-side encryption configured AWS returns only ShardId and SequenceNumber — the optional EncryptionType member is absent from the body rather than present with the value 'NONE'.
Emulator: always serializes EncryptionType, with the value 'NONE' for an unencrypted stream, so the response carries a member the recording does not have.
Why: the presence of the member is itself the signal — clients and tests use "EncryptionType in the response" to detect that a stream is encrypted, and a serializer that always emits it makes an unencrypted stream indistinguishable from an encrypted one by shape; strict response-shape assertions fail on the extra key.
Fix: the PutRecord response serializer / the handler that fills it — emit EncryptionType only when the stream actually has an encryption type configured (i.e. omit it when the stream state has none), rather than defaulting the enum to NONE.
Traces: kinesis-put-record-get-records-round-trip#task#4.

=== Cause 3 of 3: PutRecords emits the same unwanted EncryptionType 'NONE' ===
AWS: Smithy: PutRecords output { FailedRecordCount, Records (required), EncryptionType (optional enum NONE|KMS) }. As with PutRecord, a batch written to an unencrypted stream comes back with FailedRecordCount/Records only and no EncryptionType member.
Emulator: emits EncryptionType: 'NONE' in the batch response too, confirming the default-the-optional-enum behaviour is shared by both data-plane write paths rather than local to PutRecord.
Why: same as Cause 2, on the batch path that high-throughput producers actually use.
Fix: same serializer/state fix as Cause 2, applied to the PutRecords response as well (and audited for the other outputs carrying the optional EncryptionType member).
Traces: kinesis-put-records-batch-success-shape#task#4.

## Evidence
Cause 1 — fixture kinesis-cbor-error-encoding-and-serialization-exception, step task#2, DescribeStreamSummary, expected 200 / actual 200:
  $.StreamDescriptionSummary.MaxRecordSizeInKiB: missing (expected 1024, got None)
  cluster also covers kinesis-subscribe-to-shard-unregistered-consumer#task#2
  (1024 is the default/minimum of the modeled @range 1024..10240 for a stream created without the member; the requirement is that the member be present and report the stream's configured maximum, not that it be literally 1024 in every environment.)

Cause 2 — fixture kinesis-put-record-get-records-round-trip, step task#4, PutRecord, expected 200 / actual 200:
  $.EncryptionType: unexpected_present (expected absent, got 'NONE')

Cause 3 — fixture kinesis-put-records-batch-success-shape, step task#4, PutRecords, expected 200 / actual 200:
  $.EncryptionType: unexpected_present (expected absent, got 'NONE')

## Steps to Reproduce
Cause 1: CreateStream(StreamName=<name>, ShardCount=1), wait for ACTIVE, call DescribeStreamSummary(StreamName=<name>) and look for StreamDescriptionSummary.MaxRecordSizeInKiB. Repeat with CreateStream(..., MaxRecordSizeInKiB=2048) and confirm the configured value is reported back.

Cause 2: on that same stream (no StartStreamEncryption called), PutRecord(StreamName, Data=b'hello', PartitionKey='pk') and list the response's top-level members: expect exactly ShardId and SequenceNumber, with no EncryptionType key.

Cause 3: PutRecords(StreamName, Records=[{'Data': b'hello', 'PartitionKey': 'pk'}]) on the unencrypted stream and list the response's top-level members: expect FailedRecordCount and Records, with no EncryptionType key. Then call StartStreamEncryption and confirm EncryptionType appears only after that.
