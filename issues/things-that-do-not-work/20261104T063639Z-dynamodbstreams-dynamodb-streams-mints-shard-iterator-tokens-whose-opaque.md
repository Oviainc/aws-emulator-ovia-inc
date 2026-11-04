Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-onyx-traj-read-the-stream-insert-carries-no-old-image
Issue-Step: task#7
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-onyx-traj-read-the-stream-insert-carries-no-old-image
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodbstreams|GetShardIterator|2xx->2xx|$.ShardIterator
Issue-Key: dynamodbstreams|GetRecords|2xx->2xx|$.NextShardIterator
Issue-Commit: d1f4edec6ba14859dec4c3d1c4b0bae1a6751e21

# DynamoDB Streams mints shard iterator tokens whose opaque segment omits the AWS envelope preamble and payload length

> _Note: this issue was identified against the code base at commit `d1f4edec6ba14859dec4c3d1c4b0bae1a6751e21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetShardIterator

## Description
Both operations that mint a shard iterator emit the same malformed token, so this is ONE fault in the shared minting helper with TWO causes to close (one per operation / output member).

Shared fault: a DynamoDB Streams shard iterator is a three-segment, pipe-delimited value `<StreamArn>|<version>|<opaque-payload>`. The emulator reproduces the first two segments correctly but the third segment it mints is a bare 44-character base64 SHA-256 digest with no envelope, where AWS's third segment is a long base64 envelope (roughly 950 characters in the recorded responses) that begins with the fixed preamble `AAAAAAAAAAH`. The individual payload BYTES are a per-run opaque token and are NOT a requirement here; the segment's envelope preamble and its order-of-magnitude length are, because that is the part of the shape a client or proxy can observe and validate.

Cause 1 of 2: GetShardIterator.ShardIterator.
AWS: Smithy GetShardIterator output member `ShardIterator` targets `ShardIterator` with `@length min 1, max 2048`; the model says nothing further about the token's internal layout, so beyond the length bound this is recorded behavior only. The recording shows HTTP 200 with `ShardIterator` = `<StreamArn>|1|` followed by a ~950-char base64 envelope beginning `AAAAAAAAAAH…`.
Emulator: HTTP 200 with `ShardIterator` = `<StreamArn>|1|` followed by a 44-char base64 digest (e.g. a value of the form `2WOqGk9HpswPq/PVG/eEbU3RL9AWhtQj5a42bYM+Dfo=`) — correct ARN and version segments, no envelope preamble, payload two orders of magnitude shorter. `get_shard_iterator.rs` documents this deliberately: "The token is a digest, not an encoding of the input".
Why: a client that persists, forwards or validates iterators against AWS's observed token shape (length budgeting toward the 2048 bound, the `AAAAAAAAAAH` envelope preamble, proxies that sanity-check the third segment) accepts the emulator's token where it would reject or mis-size AWS's, and vice versa; recorded-fixture comparisons on the iterator value diverge even though the ARN/version prefix matches.
Fix: the shared iterator minting in this service's state (`ComAmazonawsDynamodbstreamsDynamodbstreams20120810State::mint_iterator`, services/service-com-amazonaws-dynamodbstreams-dynamodbstreams-20120810/src/state.rs), which `GetShardIterator::handle` calls at the end of its handler; the store-side cursor lookup can stay keyed by the digest, only the emitted token's envelope needs to match AWS's layout.
Traces: dynamodbstreams-onyx-traj-read-the-stream-insert-carries-no-old-image#task#7 (representative), and the same divergence at #task#9.

Cause 2 of 2: GetRecords.NextShardIterator.
AWS: Smithy GetRecords output member `NextShardIterator` targets the same `ShardIterator` shape with `@length min 1, max 2048`; layout again recorded behavior only. The recording shows HTTP 200 with `NextShardIterator` = `<StreamArn>|1|` followed by a ~950-char base64 envelope beginning `AAAAAAAAAAH…`.
Emulator: HTTP 200 with `NextShardIterator` = `<StreamArn>|1|` followed by a 44-char base64 digest (e.g. a value of the form `fNKU7VprrMEhKgX7KVxch9WuNDhKFbJEJ8qqVfYE6e4=`) — same missing envelope as Cause 1, so a client paging a shard sees the wrong token shape on every page as well as on the first iterator.
Why: same as Cause 1, but on the continuation token every polling loop carries, so the divergence is observed on each GetRecords call rather than once per shard.
Fix: same minting helper, reached from `GetRecords::handle` when it mints the token for the position after the returned page (services/service-com-amazonaws-dynamodbstreams-dynamodbstreams-20120810/src/ops/get_records.rs).
Traces: dynamodbstreams-onyx-traj-read-the-stream-insert-carries-no-old-image#task#8 (representative), and the same divergence at #task#10.

No open issue covers either cause: the replay report lists both clusters with an empty `owned_by`. The neighbouring open dynamodbstreams issues (20261029T172902Z-dynamodbstreams-constraint-violation-messages-interpolate-the-rejected-value-which.md, 20261029T172902Z-dynamodbstreams-describestream-projection-omits-keyschema-type-and-never-emits.md, 20261103T015503Z-dynamodbstreams-dynamodb-streams-accepts-a-table-arn-where-a.md) are about validation-message interpolation, the DescribeStream projection and table-ARN-vs-stream-ARN acceptance, not the iterator token layout.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 73.8%, 21 mismatched steps, 10 divergence clusters; these are the only two clusters with no owning issue and no stub).

Cluster 4 — service=dynamodbstreams|GetShardIterator|status=200->200|keys=$.ShardIterator, 2 affected steps:
  expected $.ShardIterator: 'arn:aws:dynamodb:us-west-2:000000000000:table/<table>/stream/<label>|1|AAAAAAAAAAHhI0Fg5+WndHy+46RQCWdMpZgcrTkGEvgu7XCC…' (third segment ~950 base64 chars, fixed preamble 'AAAAAAAAAAH')
  actual   $.ShardIterator: 'arn:aws:dynamodb:us-west-2:000000000000:table/<table>/stream/<label>|1|2WOqGk9HpswPq/PVG/eEbU3RL9AWhtQj5a42bYM+Dfo=' (third segment 44 base64 chars, no preamble)
  Status matches (200 vs 200); the ARN and version segments match exactly. The divergence is confined to the third segment's envelope and length.

Cluster 5 — service=dynamodbstreams|GetRecords|status=200->200|keys=$.NextShardIterator, 2 affected steps:
  expected $.NextShardIterator: 'arn:aws:dynamodb:us-west-2:000000000000:table/<table>/stream/<label>|1|AAAAAAAAAAH3MbrqfrJgpmMmo+wia6xXZ4gu8vouQERelXfs…' (~950 base64 chars, preamble 'AAAAAAAAAAH')
  actual   $.NextShardIterator: 'arn:aws:dynamodb:us-west-2:000000000000:table/<table>/stream/<label>|1|fNKU7VprrMEhKgX7KVxch9WuNDhKFbJEJ8qqVfYE6e4=' (44 base64 chars, no preamble)

Model citations: Smithy GetShardIterator.ShardIterator -> ShardIterator @length 1..2048; GetRecords.NextShardIterator -> ShardIterator @length 1..2048; GetRecords.ShardIterator (input) -> same shape, required. The model does not specify the token's internal layout, so the layout requirement above is taken from the recorded responses only, and the payload bytes themselves are explicitly not asserted as a requirement.

## Steps to Reproduce
1. Replay fixture dynamodbstreams-onyx-traj-read-the-stream-insert-carries-no-old-image against the emulator.
2. Cause 1 — at step task#7 (and again at task#9) the fixture calls GetShardIterator on the registered stream's shard. Read `$.ShardIterator` from the 200 response: it is `<StreamArn>|1|<44-char base64 digest>`, where the recording has `<StreamArn>|1|<~950-char base64 envelope beginning AAAAAAAAAAH>`.
3. Cause 2 — at step task#8 (and again at task#10) the fixture calls GetRecords with that iterator. Read `$.NextShardIterator` from the 200 response: same truncated third segment instead of the recorded envelope.
4. Both responses are HTTP 200 and their ARN/version segments match, so the only diverging part is the opaque segment's envelope preamble and length.
