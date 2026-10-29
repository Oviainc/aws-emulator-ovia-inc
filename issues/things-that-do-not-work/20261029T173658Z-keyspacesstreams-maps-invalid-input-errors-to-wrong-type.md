Issue-Source: trace-sweep-divergence
Issue-Trace: keyspacesstreams-error-get-records-malformed-iterator-and-maxresults
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: keyspacesstreams-error-get-records-malformed-iterator-and-maxresults keyspacesstreams-error-get-shard-iterator-bad-shard-and-missing-sequence keyspacesstreams-error-get-stream-invalid-and-not-found keyspacesstreams-error-list-streams-invalid-filters
Issue-Shape-Id: com.amazonaws.keyspacesstreams#KeyspacesStreams
Issue-Key: keyspacesstreams|GetRecords|4xx->4xx|$.__type
Issue-Key: keyspacesstreams|GetRecords|4xx->4xx|$.errorCode
Issue-Key: keyspacesstreams|GetRecords|4xx->4xx|$.message
Issue-Key: keyspacesstreams|GetRecords|4xx->4xx|status
Issue-Key: keyspacesstreams|GetRecords|4xx->4xx|$.Message
Issue-Key: keyspacesstreams|GetShardIterator|4xx->4xx|$.message
Issue-Key: keyspacesstreams|GetShardIterator|4xx->4xx|status
Issue-Key: keyspacesstreams|GetStream|4xx->4xx|$.__type
Issue-Key: keyspacesstreams|GetStream|4xx->4xx|$.errorCode
Issue-Key: keyspacesstreams|GetStream|4xx->4xx|$.message
Issue-Key: keyspacesstreams|GetStream|4xx->4xx|status
Issue-Key: keyspacesstreams|ListStreams|4xx->4xx|$.message
Issue-Commit: 42eb24959c6a4eb33ac3ba3d48b702ee83f6032d

# KeyspacesStreams maps invalid-input errors to wrong type, status and message

> _Note: this issue was identified against the code base at commit `42eb24959c6a4eb33ac3ba3d48b702ee83f6032d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecords

## Description
Shared fault: the KeyspacesStreams emulator routes every bad identifier/token/payload through its own "resource lookup failed" path. It therefore (a) picks ResourceNotFoundException where AWS raises ValidationException or SerializationException, (b) serializes a 404 where the recorded AWS wire status for this service's client errors is 400, (c) drops the ValidationException `errorCode` member, and (d) writes self-invented, request-interpolated messages instead of AWS's fixed error text. Six distinct causes, all in this one service.

Cause 1 of 6 - GetRecords with a malformed shardIterator (fixture keyspacesstreams-error-get-records-malformed-iterator-and-maxresults, step task#1).
AWS: a shardIterator that is syntactically not an iterator this service could ever have issued is an input-format failure, not a missing resource. Smithy: GetRecords.shardIterator is @required, target ShardIterator @length min 1 max 4096; modeled error ValidationException carries members `message` and `errorCode` of enum ValidationExceptionType (InvalidFormat, TrimmedDataAccess, ExpiredIterator, ExpiredNextToken). Recorded response: HTTP 400, awsJson1_0 `__type` = "com.amazon.ddbhelenusstreamtransformer#ValidationException" (AWS serializes this error under its internal transformer namespace, not the public com.amazonaws.keyspacesstreams one), `errorCode` = "InvalidFormat", `message` = "Invalid ShardIterator". That message is entirely fixed text - it interpolates nothing from the request.
Emulator: HTTP 404, `__type` = "ResourceNotFoundException", no `errorCode` member at all, `message` = "the shard iterator was not issued by this service".
Why: an SDK caller sees a retry/not-found class error for what is a permanent client-side format bug. Callers that branch on ValidationException.errorCode == InvalidFormat (to distinguish a corrupted iterator from an expired one, which is retryable by re-fetching the iterator) never take that branch, and 404-vs-400 flips the error into the resource-missing bucket of most error handlers.
Fix: GetRecords input validation plus the error serializer - validate shardIterator format before any state lookup and emit ValidationException with errorCode InvalidFormat at status 400.
Traces: keyspacesstreams-error-get-records-malformed-iterator-and-maxresults#task#1.

Cause 2 of 6 - GetRecords with a non-numeric maxResults (same fixture, step task#2).
AWS: maxResults targets Integer (@range min 1 max 1000), so a JSON string in that position fails awsJson1_0 deserialization before the operation's own validation runs. Recorded response: HTTP 400, `__type` = "com.amazon.coral.service#SerializationException", and the body member is the capital-M `Message` = "STRING_VALUE cannot be converted to Integer". Fixed text is "<value> cannot be converted to Integer"; the leading token is the offending value copied from the request, and the target type name comes from the modeled member's target.
Emulator: HTTP 400 (status correct) but `__type` = "ValidationException" and no `Message` member - the protocol-level deserialization failure is reported as a modeled business validation error, and the key casing differs (AWS uses `Message` for SerializationException, not `message`).
Why: clients that treat SerializationException as a non-retryable framing bug distinct from ValidationException mis-classify it, and any client reading the message finds the member absent, so the error surfaces with no text.
Fix: the service's awsJson1_0 request deserializer - a type-coercion failure on a modeled member must short-circuit to SerializationException with the `Message` member, before handler validation.
Traces: keyspacesstreams-error-get-records-malformed-iterator-and-maxresults#task#2.

Cause 3 of 6 - GetShardIterator against a stream/shard that does not exist (fixture keyspacesstreams-error-get-shard-iterator-bad-shard-and-missing-sequence, steps task#1 and task#2).
AWS: Smithy models ResourceNotFoundException for this operation (members: `message` only); the model's httpError defaults it to 404, but the recording shows this service puts it on the wire with HTTP 400. Recorded response: HTTP 400, `message` = "Requested resource not found" - wholly fixed text, with no keyspace, table, shard or stream ARN interpolated from the request.
Emulator: HTTP 404, and `message` echoes the request's stream ARN back ("stream <streamArn> does not exist"), i.e. invented wording plus a request-derived value AWS never includes.
Why: a caller matching on the status (400 vs 404) or asserting the documented message text fails against the emulator; the echoed ARN also makes the message environment-dependent, so recorded-response assertions can never be satisfied.
Fix: GetShardIterator handler's not-found path and the error serializer - emit ResourceNotFoundException at status 400 with the fixed message "Requested resource not found". Same step covers the missing-sequenceNumber variant, which clusters identically.
Traces: keyspacesstreams-error-get-shard-iterator-bad-shard-and-missing-sequence#task#1 (cluster also contains #task#2).

Cause 4 of 6 - GetStream with a syntactically invalid streamArn (fixture keyspacesstreams-error-get-stream-invalid-and-not-found, step task#1).
AWS: Smithy: GetStream.streamArn is @required, target StreamArn @length min 37 max 1024 - a value that satisfies the length bound but is not an ARN is a format failure. Recorded response: HTTP 400, `__type` = "com.amazon.ddbhelenusstreamtransformer#ValidationException", `errorCode` = "InvalidFormat", `message` = "Invalid stream ARN." (fixed text including the trailing period; nothing interpolated).
Emulator: HTTP 404, `__type` = "ResourceNotFoundException", no `errorCode`, `message` = "stream <streamArn> does not exist" - it skipped ARN syntax validation entirely and went straight to a state lookup miss.
Why: a malformed ARN is a caller bug that will never succeed on retry, but the emulator reports it as a missing resource, so clients retry or go create the stream instead of fixing the ARN.
Fix: GetStream input validation - ARN syntax check ahead of the stream lookup, raising ValidationException/InvalidFormat at 400.
Traces: keyspacesstreams-error-get-stream-invalid-and-not-found#task#1.

Cause 5 of 6 - GetStream with a well-formed but nonexistent streamArn (same fixture, step task#2).
AWS: ResourceNotFoundException, served - per the recording - with HTTP 400 and the fixed `message` "Requested resource not found", with no identifier from the request interpolated.
Emulator: HTTP 400 expected vs 404 served, and `message` = "stream <streamArn> does not exist".
Why: same as cause 3 but on GetStream - status class and message text both diverge, so error-class routing and message assertions break.
Fix: GetStream handler's not-found path plus the shared error serializer's status mapping for ResourceNotFoundException.
Traces: keyspacesstreams-error-get-stream-invalid-and-not-found#task#2.

Cause 6 of 6 - ListStreams with an unrecognized nextToken (fixture keyspacesstreams-error-list-streams-invalid-filters, step task#2).
AWS: Smithy: ListStreams.nextToken targets StreamArnToken @length min 80 max 3000; a token the service did not issue is a ValidationException. Recorded response: HTTP 400, ValidationException, `message` = "Invalid nextToken." - fixed text, no token value echoed. Status and error type already match here; only the message diverges.
Emulator: `message` = "nextToken is not a pagination token this service issued" - invented wording.
Why: callers and tests that key off AWS's documented message text for an invalid pagination token do not match, and the differing wording is the only signal distinguishing an invalid token from an expired one (ValidationExceptionType.ExpiredNextToken).
Fix: ListStreams pagination-token validation - use AWS's fixed message "Invalid nextToken.".
Traces: keyspacesstreams-error-list-streams-invalid-filters#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1: 8 steps, 1 passed, 7 mismatched (score 12.5%), 6 divergence clusters, all service=keyspacesstreams, none stub-blocked and none owned by an existing issue (list_existing_issues for 'keyspacesstreams' returns nothing).

1) GetRecords, keyspacesstreams-error-get-records-malformed-iterator-and-maxresults#task#1
   expected: 400 / __type 'com.amazon.ddbhelenusstreamtransformer#ValidationException' / errorCode 'InvalidFormat' / message 'Invalid ShardIterator'
   actual:   404 / __type 'ResourceNotFoundException' / errorCode absent / message 'the shard iterator was not issued by this service'

2) GetRecords, keyspacesstreams-error-get-records-malformed-iterator-and-maxresults#task#2
   expected: 400 / __type 'com.amazon.coral.service#SerializationException' / Message 'STRING_VALUE cannot be converted to Integer'
   actual:   400 / __type 'ValidationException' / Message absent

3) GetShardIterator, keyspacesstreams-error-get-shard-iterator-bad-shard-and-missing-sequence#task#1 (cluster also #task#2)
   expected: 400 / message 'Requested resource not found'
   actual:   404 / message 'stream <streamArn> does not exist' (recording shows the request's own ARN echoed back)

4) GetStream, keyspacesstreams-error-get-stream-invalid-and-not-found#task#1
   expected: 400 / __type 'com.amazon.ddbhelenusstreamtransformer#ValidationException' / errorCode 'InvalidFormat' / message 'Invalid stream ARN.'
   actual:   404 / __type 'ResourceNotFoundException' / errorCode absent / message 'stream <bad-arn> does not exist'

5) GetStream, keyspacesstreams-error-get-stream-invalid-and-not-found#task#2
   expected: 400 / message 'Requested resource not found'
   actual:   404 / message 'stream <streamArn> does not exist'

6) ListStreams, keyspacesstreams-error-list-streams-invalid-filters#task#2
   expected: 400 / message 'Invalid nextToken.'
   actual:   400 / message 'nextToken is not a pagination token this service issued'

Model references (services/service-com-amazonaws-keyspacesstreams-keyspacesstreams/smithy-model/com-amazonaws-keyspacesstreams-keyspacesstreams.json): protocol awsJson1_0; ValidationException members {message, errorCode: ValidationExceptionType = InvalidFormat|TrimmedDataAccess|ExpiredIterator|ExpiredNextToken}; ResourceNotFoundException member {message}; GetRecords.shardIterator @required @length(1,4096), GetRecords.maxResults Integer @range(1,1000); GetShardIterator.streamArn/shardId/shardIteratorType @required; GetStream.streamArn @required @length(37,1024); ListStreams.nextToken StreamArnToken @length(80,3000).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) - the four keyspacesstreams-error-* fixtures.
3. Cause 1: POST awsJson1_0 X-Amz-Target KeyspacesStreams.GetRecords with {"shardIterator":"<garbage string, 1..4096 chars>"} -> emulator answers 404 ResourceNotFoundException; AWS answers 400 ValidationException with errorCode InvalidFormat and message 'Invalid ShardIterator'.
4. Cause 2: same target with {"shardIterator":"<any>","maxResults":"STRING_VALUE"} -> emulator answers 400 ValidationException with no Message; AWS answers 400 com.amazon.coral.service#SerializationException with Message '<value> cannot be converted to Integer'.
5. Cause 3: KeyspacesStreams.GetShardIterator with a well-formed streamArn for a keyspace/table that does not exist (and shardIteratorType requiring a sequenceNumber that is absent) -> emulator answers 404 with an ARN-interpolated message; AWS answers 400 ResourceNotFoundException with message 'Requested resource not found'.
6. Cause 4: KeyspacesStreams.GetStream with streamArn set to a non-ARN string at least 37 chars long -> emulator answers 404 ResourceNotFoundException; AWS answers 400 ValidationException / InvalidFormat / 'Invalid stream ARN.'.
7. Cause 5: KeyspacesStreams.GetStream with a syntactically valid streamArn for a nonexistent stream -> emulator answers 404 with an interpolated message; AWS answers 400 with 'Requested resource not found'.
8. Cause 6: KeyspacesStreams.ListStreams with a nextToken of 80..3000 chars the service never issued -> emulator message 'nextToken is not a pagination token this service issued'; AWS message 'Invalid nextToken.'.
9. Re-replay all six fixture#step bindings named in issue_trace/issue_step and trace_ids to confirm every cause is closed.
