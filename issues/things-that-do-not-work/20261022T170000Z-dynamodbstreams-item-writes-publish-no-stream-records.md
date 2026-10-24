Issue-Source: fixer-observation
Issue-Severity: HIGH
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Related: 20261022T150920Z-dynamodbstreams-dynamodb-streams-are-not-registered-or-validated-liststreams.md

# DynamoDB item writes publish no stream records: a discovered stream stays empty forever

> _Note: this issue was filed while fixing `20261022T150920Z-dynamodbstreams-dynamodb-streams-are-not-registered-or-validated-liststreams.md`, which made a table's stream discoverable and readable. It is the next step of the same seam and was deliberately left out of that change._

## Operation
GetRecords (DynamoDB Streams), published by DynamoDB PutItem / UpdateItem / DeleteItem / BatchWriteItem / TransactWriteItems

## Description
A table created (or updated) with `StreamSpecification {StreamEnabled: true}` now registers a stream: `ListStreams` reports it, `DescribeStream` describes its shard, `GetShardIterator` mints an iterator and `GetRecords` serves it. What nothing does yet is PUBLISH: the DynamoDB data plane never calls the streams service's publishing seam
(`services/service-com-amazonaws-dynamodbstreams-dynamodbstreams-20120810/src/state.rs`,
`ComAmazonawsDynamodbstreamsDynamodbstreams20120810State::append_record`), so every
`GetRecords` page is empty however many items the table takes.

AWS: "DynamoDB Streams captures a time-ordered sequence of item-level modifications in any DynamoDB table" — one stream record per item-level change, with `eventName` `INSERT` for an item that did not exist, `MODIFY` for one that did and `REMOVE` for a delete that removed one, `dynamodb.Keys` always populated, and `NewImage`/`OldImage` populated per the stream's `StreamViewType` (`KEYS_ONLY` carries neither). The seam already implements all of that (`RecordAppend`, the per-view-type image filter, sequence numbers, `SizeBytes`, `eventID`, `eventSource`/`eventVersion`); it has no caller.

Emulator: a consumer that discovers the stream and polls it is told 200 with `Records: []`
forever — a success carrying a plausibly wrong answer, which is worse for a change-data-capture
client than an error it could retry.

Fix: call the seam from the DynamoDB write path — `put_item`, `update_item`, `delete_item` and
the batch/transaction writers — for a table whose `state::Table::stream` is enabled, passing the
item's key map and the before/after images the operation already computes. The two services'
`AttributeValue` shapes are distinct generated types, so the conversion between them belongs in
one shared helper, and it must be ITERATIVE over `M`/`L` nesting for the reason
`attribute_map_size`/`attribute_map_depth` already document in the streams state. A write to a
table whose stream is `DISABLED` publishes nothing (its shard is closed).

## Evidence
`grep -rn "append_record" services/` matches only the streams crate's own state and tests: no
operation of `service-com-amazonaws-dynamodb-dynamodb-20120810` calls it. The streams crate's
`src/state.rs` module doc names `append_record` as a seam "DynamoDB's
`PutItem`/`UpdateItem`/`DeleteItem` call", and the bootstrap report
`issues/things-that-now-work/20261021T215807Z-bootstrap-service-com-amazonaws-dynamodbstreams-dynamodbstreams-20120810.md`
records the same wiring as a follow-up.

## Steps to Reproduce
1. CreateTable with `StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES}`.
2. PutItem, then UpdateItem, then DeleteItem on the same key.
3. ListStreams -> the table's stream; DescribeStream -> its shard; GetShardIterator
   (`TRIM_HORIZON`) -> an iterator.
4. GetRecords with that iterator: observe 200 with an empty `Records` list, where AWS reports
   three records (`INSERT`, `MODIFY`, `REMOVE`) with the keys and the images the view type
   captures.
