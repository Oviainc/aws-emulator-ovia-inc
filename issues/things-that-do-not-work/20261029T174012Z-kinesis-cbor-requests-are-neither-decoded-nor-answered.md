Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-cbor-error-encoding-and-serialization-exception
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-cbor-error-encoding-and-serialization-exception
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|DescribeStreamSummary|4xx->4xx|$.__type
Issue-Key: kinesis|DescribeStreamSummary|4xx->4xx|$.message
Issue-Key: kinesis|DescribeStreamSummary|4xx->4xx|$headers.content-type
Issue-Key: kinesis|PutRecord|4xx->4xx|$.Message
Issue-Key: kinesis|PutRecord|4xx->4xx|$.__type
Issue-Key: kinesis|PutRecord|4xx->4xx|$headers.content-type
Issue-Key: kinesis|DescribeStreamSummary|4xx->4xx|$.Message
Issue-Key: kinesis|GetShardIterator|2xx->4xx|$.ShardIterator
Issue-Key: kinesis|GetShardIterator|2xx->4xx|$headers.content-type
Issue-Key: kinesis|GetShardIterator|2xx->4xx|status
Issue-Commit: a8a8dc0cc1ff1737eab503998484303fbbf09d5b

# kinesis: CBOR requests are neither decoded nor answered in CBOR, and body type errors are not SerializationException

> _Note: this issue was identified against the code base at commit `a8a8dc0cc1ff1737eab503998484303fbbf09d5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStreamSummary

## Description
Four net-new root causes in the kinesis emulator's handling of the awsJson1_1 CBOR wire encoding (Content-Type application/x-amz-cbor-1.1), which Kinesis has always accepted alongside application/x-amz-json-1.1 and which SDKs such as the AWS SDK for Java v1 use by default for this service. None of the four is marked ALREADY FILED or OWNED in the replay report, and no open kinesis issue names the CBOR encoding or SerializationException (the open ones cover CloudFormation/DryRun/not-found messages, per-shard quotas, StartStreamEncryption, ListStreamConsumers pagination, RecordDistributionStrategy and the resource-policy/UntagResource scaffold stubs).

=== Cause 1 of 4: a CBOR-encoded request body is not decoded at all; every operation fails with an invented parser ValidationException instead of executing ===
AWS: Smithy: service Kinesis_20131202, protocol awsJson1_1 (protocol_config http: http/1.1, h2). Kinesis accepts the request body in the CBOR representation of that protocol when the request carries Content-Type: application/x-amz-cbor-1.1, decodes it identically to the JSON representation, and then applies normal operation semantics. For DescribeStreamSummary on a stream that does not exist (Smithy: DescribeStreamSummary errors AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceNotFoundException, all HTTP 400) the result is 400 ResourceNotFoundException whose message is the fixed template "Stream <streamName> under account <accountId> not found." — only the stream name (from the request) and the 12-digit account id (from the caller's credentials/state) vary.
Emulator: the CBOR body is fed to the JSON parser, which fails, and the request is refused with 400 ValidationException and the message "json parse error in lookahead" — a message that is not AWS wording for any error class and names an internal parser stage. The modeled operation never runs, so the stream lookup and its ResourceNotFoundException are unreachable over CBOR.
Why: every client that speaks the CBOR representation (the default for the Java v1 SDK and anything that negotiates it) gets a ValidationException on *every* Kinesis call, including calls that should succeed, so the whole service is unusable on that encoding; worse, a not-found condition is reported as a client validation fault, so retry/branching logic keyed on ResourceNotFoundException never fires.
Fix: request-body decoding for the kinesis service's awsJson1_1 protocol layer — select the CBOR decoder on Content-Type application/x-amz-cbor-1.1 instead of always parsing JSON, before dispatch to the operation handler.
Traces: kinesis-cbor-error-encoding-and-serialization-exception#task#3 (also #task#6).

=== Cause 2 of 4: a blob member carrying a non-blob value is refused as ValidationException instead of SerializationException, and the error body uses the wrong message key ===
AWS: Smithy: PutRecord.Data targets Data (blob, @length 0..10485760, required). A body whose Data member holds a string that is not a valid blob/byte-buffer encoding is a *deserialization* fault, not a constraint violation: AWS answers HTTP 400 with __type SerializationException and the error member spelled with a capital M, Message, whose text is the fixed template "'<value>' can not be converted to a ByteBuffer" — the only variable part is the offending value taken verbatim from the request body; the words "can not be converted to a ByteBuffer" are fixed (note "can not", two words).
Emulator: answers 400 with __type ValidationException and no Message member at all (it emits the lowercase 'message' member used by the modeled kinesis exceptions), so a client reading Message gets nothing and a client switching on __type sees a constraint error where AWS reports a serialization error.
Why: SDKs classify SerializationException separately from ValidationException (it is not a modeled, retryable or constraint-shaped error), and the capital-M Message member is where their error-message extraction looks for this class; the emulator both mislabels the class and drops the message, so a malformed-body bug surfaces to the caller as an empty constraint failure.
Fix: the kinesis protocol layer's body deserializer — raise SerializationException with a Message member when a member's wire value cannot be coerced to the modeled type, instead of routing the failure into the constraint-validation path.
Traces: kinesis-cbor-error-encoding-and-serialization-exception#task#4.

=== Cause 3 of 4: a string member carrying a numeric value is likewise refused as ValidationException instead of SerializationException ===
AWS: Smithy: DescribeStreamSummary.StreamName targets StreamName (string, @length 1..128, @pattern ^[a-zA-Z0-9_.-]+$). A body whose StreamName member holds a number rather than a string fails deserialization before constraints are evaluated: AWS answers HTTP 400, __type SerializationException, Message = "'<value>' can not be converted to a CharSequence" — fixed text "can not be converted to a CharSequence" with the offending wire value as the only variable part.
Emulator: 400 __type ValidationException with no Message member, i.e. the same misrouting as Cause 2 but for the string/CharSequence coercion, confirming the defect is in the shared deserializer rather than in one member.
Why: same as Cause 2 — the error class a client must distinguish (bad encoding vs. bad value) is lost, and the diagnostic text that names the member value is not emitted at all.
Fix: same deserializer path as Cause 2; the CharSequence and ByteBuffer coercions must both produce SerializationException/Message.
Traces: kinesis-cbor-error-encoding-and-serialization-exception#task#5.

=== Cause 4 of 4: responses to a CBOR request are serialized as application/x-amz-json-1.1, so even a would-be success is unreadable ===
AWS: Smithy: protocol awsJson1_1. The response encoding mirrors the request encoding: a request sent with Content-Type application/x-amz-cbor-1.1 is answered with Content-Type: application/x-amz-cbor-1.1 and a CBOR-encoded body. For GetShardIterator on an existing shard (Smithy: output ShardIterator, @length 1..512) that response is HTTP 200 carrying ShardIterator.
Emulator: answers every request with Content-Type: application/x-amz-json-1.1 regardless of the request's content type, and in this step answers HTTP 400 with no ShardIterator at all where the recording shows 200 and an iterator — the body never decoded (Cause 1), and the response the client does get is in an encoding it did not ask for.
Why: a CBOR client cannot parse a JSON body, so even after Cause 1 is fixed every response is garbage to it; the content-type header is also what SDK response handlers dispatch on, so the mismatch is detected as a protocol error rather than as data.
Fix: response serialization in the kinesis protocol layer — carry the negotiated encoding from request to response and emit the matching Content-Type for both success and error bodies.
Traces: kinesis-cbor-error-encoding-and-serialization-exception#task#7.

## Evidence
Cause 1 — fixture kinesis-cbor-error-encoding-and-serialization-exception, step task#3, DescribeStreamSummary, expected 400 / actual 400:
  $.__type: expected 'ResourceNotFoundException', got 'ValidationException'
  $.message: expected 'Stream <streamName> under account <accountId> not found.', got 'json parse error in lookahead'
  $headers.content-type: expected 'application/x-amz-cbor-1.1', got 'application/x-amz-json-1.1'
  (cluster also covers step task#6 of the same fixture)

Cause 2 — step task#4, PutRecord, expected 400 / actual 400:
  $.__type: expected 'SerializationException', got 'ValidationException'
  $.Message: missing — expected "'<value>' can not be converted to a ByteBuffer", got None
  $headers.content-type: expected 'application/x-amz-cbor-1.1', got 'application/x-amz-json-1.1'

Cause 3 — step task#5, DescribeStreamSummary, expected 400 / actual 400:
  $.__type: expected 'SerializationException', got 'ValidationException'
  $.Message: missing — expected "'<value>' can not be converted to a CharSequence", got None
  $headers.content-type: expected 'application/x-amz-cbor-1.1', got 'application/x-amz-json-1.1'

Cause 4 — step task#7, GetShardIterator:
  status: expected 200, got 400
  $.ShardIterator: missing (expected an iterator value, got None)
  $headers.content-type: expected 'application/x-amz-cbor-1.1', got 'application/x-amz-json-1.1'

## Steps to Reproduce
All four causes reproduce against the kinesis endpoint with a sigv4-signed POST carrying X-Amz-Target: Kinesis_20131202.<Operation> and Content-Type: application/x-amz-cbor-1.1 with a CBOR-encoded body.

Cause 1: CBOR-encode {"StreamName": "<a stream that does not exist>"} and call DescribeStreamSummary. Observe 400 ValidationException / 'json parse error in lookahead' instead of 400 ResourceNotFoundException / "Stream <streamName> under account <accountId> not found."; also observe the response Content-Type.

Cause 2: call PutRecord with the Data member set to a value that is not a valid blob encoding (e.g. the string 'notbytes') and read __type and Message: expect SerializationException with Message "'notbytes' can not be converted to a ByteBuffer".

Cause 3: call DescribeStreamSummary with StreamName set to a number (e.g. 12) and read __type and Message: expect SerializationException with Message "'12' can not be converted to a CharSequence".

Cause 4: CreateStream, wait for ACTIVE, then call GetShardIterator(StreamName, ShardId, ShardIteratorType='TRIM_HORIZON') over CBOR: expect HTTP 200, a ShardIterator member, and response Content-Type: application/x-amz-cbor-1.1.
