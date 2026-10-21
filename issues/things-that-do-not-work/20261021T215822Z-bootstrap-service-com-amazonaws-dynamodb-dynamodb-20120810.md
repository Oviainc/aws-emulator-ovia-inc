Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the DynamoDB emulator

_Filed: 2026-10-21T21:58:22Z_
_Operation: service-com-amazonaws-dynamodb-dynamodb-20120810 | Class: stub_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 58 operations in `services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no DynamoDB client can do anything at all. Implement a FIRST, SMALL slice: the table + item happy path, driven entirely from persisted state (no canned, hardcoded or echoed responses). Wire protocol is `awsJson1_0`, `POST /` with `X-Amz-Target: DynamoDB_20120810.<OperationName>` (OPERATIONS.md 'Wire protocol').

Happy path, in order:

1. **CreateTable** — input `TableName` (required), `KeySchema` (HASH + optional RANGE), `AttributeDefinitions`, `BillingMode`/`ProvisionedThroughput`. Store a new table; return `TableDescription` built from the stored table (TableName, KeySchema, AttributeDefinitions, TableStatus=ACTIVE, TableArn `arn:aws:dynamodb:<region>:<account>:table/<name>`, CreationDateTime, ItemCount=0, TableSizeBytes=0, ProvisionedThroughput echoing stored values). A second CreateTable with the same name must fail `ResourceInUseException` (HTTP 400).
2. **DescribeTable** — input `TableName`; return `Table` (TableDescription) read back from the store, with `ItemCount` reflecting the items actually written. Unknown table ⇒ `ResourceNotFoundException` (HTTP 400).
3. **PutItem** — input `TableName` (required), `Item` (map<string,AttributeValue>, required). Validate that `Item` contains every key attribute declared in the table's KeySchema; key the stored item by its extracted primary key so a put over the same key REPLACES the prior item. `ReturnValues=ALL_OLD` must return the overwritten item in `Attributes` (and nothing when there was none). Unknown table ⇒ `ResourceNotFoundException`.
4. **GetItem** — input `TableName`, `Key` (required). Look the item up by the key extracted from the stored table; return `Item` with the EXACT AttributeValue map that PutItem stored (all types: S, N, B, BOOL, NULL, L, M, SS, NS, BS). A missing item is a 200 with `Item` absent — not an error. Unknown table ⇒ `ResourceNotFoundException`.
5. **Query** — input `TableName`, `KeyConditionExpression` + `ExpressionAttributeValues` (and `ExpressionAttributeNames`), plus `Limit`, `ScanIndexForward`, `ExclusiveStartKey`. Support at minimum `#pk = :pk` equality on the partition key, optionally ANDed with an `=`/`<`/`<=`/`>`/`>=`/`BETWEEN`/`begins_with` condition on the sort key. Return `Items` sorted by sort key (descending when `ScanIndexForward=false`), with `Count`/`ScannedCount` computed from the result and `LastEvaluatedKey` set when `Limit` truncated the result (and honored as `ExclusiveStartKey` on the next call).
6. **DeleteItem** — input `TableName`, `Key` (required). Remove the stored item; `ReturnValues=ALL_OLD` returns the removed attributes. A subsequent GetItem must report it gone and Query must no longer list it.
7. **DeleteTable** — input `TableName`; remove the table and all its items, return the final `TableDescription` with `TableStatus=DELETING`. Afterwards DescribeTable/GetItem/Query on that name must return `ResourceNotFoundException`, and ListTables (if included) must not list it.

Everything else (BatchGetItem/BatchWriteItem, UpdateItem, Scan, TransactWriteItems, indexes, backups, exports, streams, TTL, resource policies, PartiQL) stays 501 and is filed separately as individual gap issues.

## State Model

In `src/state.rs`, reachable via `crate::state::service_state(&*self.engine)` (the ONE shared engine — never construct a new engine in a handler):

- `tables: HashMap<String /*TableName*/, Table>`
- `Table { name: String, arn: String, key_schema: Vec<KeySchemaElement{attribute_name, key_type: HASH|RANGE}>, attribute_definitions: Vec<AttributeDefinition{attribute_name, attribute_type: S|N|B}>, billing_mode: Option<BillingMode>, provisioned_throughput: Option<ProvisionedThroughput{read_capacity_units, write_capacity_units}>, table_status: TableStatus, creation_date_time: SystemTime, items: BTreeMap<ItemKey, HashMap<String, AttributeValue>>, table_size_bytes: i64 }`
- `ItemKey { partition: AttributeValueKey, sort: Option<AttributeValueKey> }` — a comparable, order-preserving encoding of the key AttributeValues so `BTreeMap` iteration yields DynamoDB sort-key order and `ExclusiveStartKey` pagination is a range scan.
- `ItemCount` and `TableSizeBytes` in every TableDescription are DERIVED from `items` at response time, never stored constants.

Done = a client script that runs CreateTable → DescribeTable → PutItem (several items sharing a partition key) → GetItem → Query (with Limit + pagination) → DeleteItem → DeleteTable succeeds with response bodies that reflect exactly what was written, and the post-delete calls return ResourceNotFoundException.

## Evidence

All 58 handler files under `services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker at line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) — e.g. create_table.rs, put_item.rs, get_item.rs, query.rs, delete_item.rs, delete_table.rs. Field names, requiredness and error sets above are taken from the crate's OPERATIONS.md: CreateTable (lines 57-61: TableName required, KeySchema, AttributeDefinitions, output TableDescription, errors ResourceInUseException/LimitExceededException), DescribeTable (147-151: TableName required, output Table, ResourceNotFoundException), GetItem (195-199: TableName+Key required, output Item), PutItem (259-263: TableName+Item required, output Attributes, ConditionalCheckFailedException/ResourceNotFoundException), Query (271-275: @paginated(inputToken=ExclusiveStartKey, outputToken=LastEvaluatedKey, items=Items), Count/ScannedCount required with default 0), DeleteItem (69-73: TableName+Key required, output Attributes), DeleteTable (81-84: TableName required, output TableDescription). Protocol (awsJson1_0, X-Amz-Target DynamoDB_20120810.*) from OPERATIONS.md lines 8-15. No tests exist for these behaviors (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. POST / with X-Amz-Target: DynamoDB_20120810.CreateTable and body {"TableName":"Music","KeySchema":[{"AttributeName":"Artist","KeyType":"HASH"},{"AttributeName":"SongTitle","KeyType":"RANGE"}],"AttributeDefinitions":[{"AttributeName":"Artist","AttributeType":"S"},{"AttributeName":"SongTitle","AttributeType":"S"}],"BillingMode":"PAY_PER_REQUEST"} — expect 200 with TableDescription{TableName:Music, TableStatus:ACTIVE, TableArn, KeySchema}; actual 501 NotImplemented.
2. DescribeTable {"TableName":"Music"} — expect the same table read back from state with ItemCount 0; actual 501.
3. PutItem {"TableName":"Music","Item":{"Artist":{"S":"Acme"},"SongTitle":{"S":"A"},"Year":{"N":"2001"}}} and again with SongTitle "B" — expect 200 each; actual 501.
4. GetItem {"TableName":"Music","Key":{"Artist":{"S":"Acme"},"SongTitle":{"S":"A"}}} — expect Item identical to what was put, including Year {"N":"2001"}; actual 501.
5. Query {"TableName":"Music","KeyConditionExpression":"Artist = :a","ExpressionAttributeValues":{":a":{"S":"Acme"}},"Limit":1} — expect Items length 1, Count 1, ScannedCount 1 and a LastEvaluatedKey; re-issue with that as ExclusiveStartKey to get the second item; actual 501.
6. DeleteItem {"TableName":"Music","Key":{"Artist":{"S":"Acme"},"SongTitle":{"S":"A"}},"ReturnValues":"ALL_OLD"} — expect the deleted attributes in Attributes, then GetItem for that key returns 200 with no Item; actual 501.
7. DeleteTable {"TableName":"Music"} — expect TableDescription with TableStatus DELETING, then DescribeTable {"TableName":"Music"} returns 400 ResourceNotFoundException; actual 501 for both.
