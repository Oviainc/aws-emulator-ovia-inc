Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-stream-enable-describe-and-iterate
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-stream-enable-describe-and-iterate dynamodbstreams-stream-lookup-error-envelopes
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams|ListStreams|2xx->2xx|$.Streams[*]
Issue-Key: dynamodbstreams|GetRecords|4xx->4xx|$.__type
Issue-Key: dynamodbstreams|GetRecords|4xx->4xx|$.message
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# DynamoDB Streams are not registered or validated: ListStreams empty and iterator errors misclassified

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStreams

## Description
This bundle has 2 causes; both must be closed.

Cause 1 of 2: after a table is created with a stream enabled, ListStreams answers 200 but with an empty/absent Streams list, so the stream is invisible.
AWS: Smithy (com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810): ListStreams takes an optional TableName (length 3..255, pattern ^[a-zA-Z0-9_.-]+$), Limit and ExclusiveStartStreamArn, and returns ListStreamsOutput.Streams (StreamList) plus LastEvaluatedStreamArn; ResourceNotFoundException (400) and InternalServerError (500) are the only modeled errors. The recording shows 200 with Streams[0] = {StreamArn: <the table's latest stream ARN>, StreamLabel: <generated>, TableName: <the table under test>} - i.e. one entry per stream-enabled table.
Emulator: 200, but $.Streams[0] is missing (no stream entry is returned for a table whose StreamSpecification.StreamEnabled is true).
Why: ListStreams is the discovery entry point for a streams consumer. With an empty list the client concludes the table has no stream, so it never calls DescribeStream/GetShardIterator/GetRecords and silently processes no change records - a data-loss-shaped failure with no error to retry on.
Fix: the dynamodbstreams service state / stream registry - enabling a stream on CreateTable (or UpdateTable) must register a stream keyed by table name and expose it through the ListStreams handler, with StreamArn, StreamLabel and TableName populated.
Traces: dynamodbstreams-stream-enable-describe-and-iterate#task#2.

Cause 2 of 2: a malformed ShardIterator passed to GetRecords is reported as an expired iterator instead of a validation error, so the error CODE is wrong.
AWS: Smithy: GetRecords.ShardIterator is required with @length min 1 max 2048; modeled errors are ExpiredIteratorException, TrimmedDataAccessException, ResourceNotFoundException, LimitExceededException (all 400) and InternalServerError (500). The recording shows 400 with __type 'com.amazon.coral.validate#ValidationException' (message example 'Invalid ShardIterator') for an iterator token that is not a well-formed iterator - ExpiredIteratorException is reserved for a once-valid iterator past its lifetime.
Emulator: 400 with __type 'ExpiredIteratorException' (message example 'The shard iterator is not valid or has expired; a shard iterator expires 15 minutes after it is retrieved'). The status matches; the error code does not.
Why: the two codes drive opposite client behavior. ExpiredIteratorException tells an SDK/consumer to re-acquire a shard iterator and retry, so a client fed a permanently malformed token retries forever instead of surfacing a non-retriable input error. The AWS SDK streams adapters and KCL branch on exactly this code.
Fix: GetRecords' ShardIterator validation in the dynamodbstreams handler - an iterator that fails to parse/decode must raise ValidationException (__type com.amazon.coral.validate#ValidationException, 400); ExpiredIteratorException must be raised only for a structurally valid iterator whose lifetime has passed.
Traces: dynamodbstreams-stream-lookup-error-envelopes#task#3.

## Evidence
Cause 1 - fixture dynamodbstreams-stream-enable-describe-and-iterate, step task#2, ListStreams, expected 200 / actual 200:
  $.Streams[0]: missing (expected {'StreamArn': <latest stream ARN of the table>, 'StreamLabel': <shaped, not asserted>, 'TableName': <table under test>}, got None)

Cause 2 - fixture dynamodbstreams-stream-lookup-error-envelopes, step task#3, GetRecords, expected 400 / actual 400:
  $.__type: expected 'com.amazon.coral.validate#ValidationException', got 'ExpiredIteratorException'
  $.message: expected 'Invalid ShardIterator', got 'The shard iterator is not valid or has expired; a shard iterator expires 15 minutes after it is retrieved' (messages quoted as examples; the requirement is the 400 status and the __type)

Stream ARNs, stream labels and table names in the recordings are environment-specific examples; the requirements are the 200 status with a populated Streams entry, and the 400 status with the ValidationException error code.

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture dynamodbstreams-stream-enable-describe-and-iterate - CreateTable with StreamSpecification {StreamEnabled: true, StreamViewType: NEW_IMAGE}, then at task#2 call dynamodbstreams ListStreams (optionally scoped by TableName). Observe 200 with no Streams entry for the table.
3. Cause 2: fixture dynamodbstreams-stream-lookup-error-envelopes - at task#3 call GetRecords with a ShardIterator token that is not a valid iterator. Observe 400 with __type 'ExpiredIteratorException' instead of 'com.amazon.coral.validate#ValidationException'.
