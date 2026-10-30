Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-shard-iterator-request-validation
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-shard-iterator-request-validation
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodb|CreateTable|2xx->2xx|$.TableDescription.LatestStreamArn
Issue-Commit: ee48b235bb960b62f99a59f2c73c375019d6987c

# CreateTable LatestStreamArn stream label omits the millisecond fraction of its ISO 8601 timestamp

> _Note: this issue was identified against the code base at commit `ee48b235bb960b62f99a59f2c73c375019d6987c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Cause 1 of 1: the stream label component of TableDescription.LatestStreamArn returned by CreateTable is formatted with second precision instead of the millisecond-precision ISO 8601 timestamp AWS uses, so the ARN the emulator hands back is not a well-formed DynamoDB stream ARN and does not match the stream ARN the same emulator later serves for that table.

AWS: Smithy (com-amazonaws-dynamodb-dynamodb-20120810) TableDescription.LatestStreamLabel is documented as "A timestamp, in ISO 8601 format, for this stream", and LatestStreamArn is "The Amazon Resource Name (ARN) that uniquely identifies the latest stream for this table". AWS builds that ARN as arn:aws:dynamodb:<region>:<account>:table/<TableName>/stream/<LatestStreamLabel>, where the label is an ISO 8601 timestamp carrying a three-digit fractional-seconds field, i.e. yyyy-MM-ddTHH:mm:ss.SSS. The recorded CreateTable response shows exactly that shape for the stream label: the fixed parts are the ARN scaffolding (`arn:aws:dynamodb:`, `:table/`, `/stream/`) and the `yyyy-MM-ddTHH:mm:ss.SSS` label layout; the variable parts are region, account id, table name and the table's own stream-creation instant. The instant itself is a per-run value and is NOT part of the requirement — the millisecond-precision layout is.

Emulator: CreateTable returns a stream label truncated to whole seconds — `.../stream/2026-10-30T04:37:01` — with no `.SSS` fraction, while DescribeStream in the same corpus serves labels that do carry it (`.../stream/2026-10-30T04:37:08.298`, and StreamDescription.StreamLabel `2026-10-30T04:37:08.298`). So the label the create path mints and the label the streams path reports are formatted differently.

Why: a client that stores LatestStreamArn/LatestStreamLabel from CreateTable and then passes it to DynamoDB Streams (DescribeStream, GetShardIterator, ListStreams filtering, or an event-source mapping that pins a stream ARN) is holding a string that is not the stream's ARN under the documented label format, so the stream cannot be addressed from the create response alone; label-equality checks and any client that parses the label as an ISO 8601 instant with fractional seconds also break.

Fix: the dynamodb CreateTable handler's stream-metadata construction / TableDescription serializer — the stream label must be rendered with the same millisecond-precision ISO 8601 formatter the streams side uses, and the stored label must be the single source for both LatestStreamArn and the stream ARN DescribeStream reports. Hypothesis: the create path formats the creation instant with a seconds-resolution formatter while the streams path uses a milliseconds-resolution one.

Traces: dynamodbstreams-shard-iterator-request-validation#setup#1 (CreateTable, 200/200, $.TableDescription.LatestStreamArn value_mismatch).

## Evidence
Fixture dynamodbstreams-shard-iterator-request-validation, step setup#1, operation CreateTable, expected status 200 / actual status 200.

Diverging field: $.TableDescription.LatestStreamArn
  expected: arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-probe-dbs-validate-bd9aa231/stream/2026-10-11T20:14:17.506
  actual:   arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-probe-dbs-validate-bd9aa231/stream/2026-10-30T04:37:01

The date/time itself is a per-run clock value and is not asserted as a requirement here. The environment-independent divergence is the stream label's format: expected label `2026-10-11T20:14:17.506` has the three-digit fractional-seconds field of an ISO 8601 timestamp; the emulator's label `2026-10-30T04:37:01` has none. Corroborating actual values from the same replay, showing the emulator's own streams path does emit the fraction: dynamodbstreams-stream-shard-page-walk#task#2 DescribeStream actual StreamArn `arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-probe-dbs-pagewalk-1006404f/stream/2026-10-30T04:37:08.298` and StreamLabel `2026-10-30T04:37:08.298`.

No open issue covers this: the DynamoDB Streams issues open against this corpus cover DescribeStream's KeySchema __type / LastEvaluatedShardId projection (20261029T172902Z-dynamodbstreams-describestream-projection-omits-keyschema-type-and-never-emits.md), constraint-message value interpolation (20261029T172902Z-dynamodbstreams-constraint-violation-messages-interpolate-the-rejected-value-which.md), missing parameter upper-bound refusals (20261029T172902Z-dynamodbstreams-describestream-getrecords-and-getsharditerator-skip-documented-parameter-upper.md) and UpdateTable's TableStatus (20261029T172902Z-dynamodb-updatetable-reports-tablestatus-active-instead-of-updating-for.md) — none of them concerns the stream-label format on the CreateTable response.

## Steps to Reproduce
1. Replay fixture dynamodbstreams-shard-iterator-request-validation against the emulator and stop at step setup#1.
2. That step calls DynamoDB_20120810.CreateTable for a table with a StreamSpecification enabled (StreamViewType KEYS_ONLY) and a single HASH key `pk`.
3. Read $.TableDescription.LatestStreamArn (and LatestStreamLabel) from the 200 response: the component after `/stream/` is `YYYY-MM-DDTHH:MM:SS` with no `.SSS` fraction.
4. Compare it with the stream label the same emulator reports for an enabled stream, e.g. replay dynamodbstreams-stream-shard-page-walk and read $.StreamDescription.StreamLabel / StreamArn from task#2's DescribeStream — those carry the three-digit fraction.
5. Expected: both are rendered as a millisecond-precision ISO 8601 label (yyyy-MM-ddTHH:mm:ss.SSS) and the CreateTable ARN is byte-identical to the stream ARN DescribeStream serves for that table.
