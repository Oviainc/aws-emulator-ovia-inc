Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the Keyspaces emulator

_Filed: 2026-10-21T21:47:55Z_
_Operation: service-com-amazonaws-keyspaces-keyspacesservice | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 19 operations in `services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this emulator. Implement a FIRST SMALL SLICE: the keyspace/table lifecycle, stateful end-to-end, over the `awsJson1_0` wire protocol (`POST /`, `X-Amz-Target: KeyspacesService.<OperationName>`, sigv4 signing name `cassandra`).

### Happy path to implement (in order)

1. **CreateKeyspace** — input `keyspaceName` (required, len 1..48, pattern `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`), optional `tags`, `replicationSpecification`. Must insert a new keyspace record and return `resourceArn` (required) of the form `arn:aws:cassandra:<region>:<account>:/keyspace/<keyspaceName>/`. Creating a name that already exists must fail with `ConflictException` (409); a name violating the length/pattern constraint must fail with `ValidationException` (400).
2. **CreateTable** — input `keyspaceName` + `tableName` (both required, same pattern) and `schemaDefinition` (required; `allColumns` + `partitionKeys`, optional `clusteringKeys`/`staticColumns`), plus optional `comment`, `capacitySpecification`, `encryptionSpecification`, `pointInTimeRecovery`, `ttl`, `defaultTimeToLive`, `tags`, `clientSideTimestamps`. Must persist the ENTIRE schemaDefinition and the specs under the owning keyspace and return `resourceArn` (`.../keyspace/<ks>/table/<tbl>`). Unknown `keyspaceName` → `ResourceNotFoundException` (404); duplicate table → `ConflictException` (409); missing/empty `partitionKeys` → `ValidationException` (400).
3. **GetTable** — input `keyspaceName` + `tableName`. Must return the stored values, not canned ones: `keyspaceName`, `tableName`, `resourceArn`, `creationTimestamp` (the time CreateTable ran), `status` = `ACTIVE`, the exact `schemaDefinition` supplied at create, `capacitySpecification` (as a CapacitySpecificationSummary; default `throughputMode: PAY_PER_REQUEST` when not supplied), `encryptionSpecification` (default `type: AWS_OWNED_KMS_KEY`), `pointInTimeRecovery` (default `status: DISABLED`), `ttl`, `defaultTimeToLive`, `comment`. Unknown keyspace or table → `ResourceNotFoundException` (404).
4. **ListTables** — input `keyspaceName` (required), optional `maxResults` (1..1000) and `nextToken` (1..2048). Must return a `tables` list of `TableSummary{keyspaceName, tableName, resourceArn}` built from stored state, truncated at `maxResults` with an opaque `nextToken` that resumes exactly where the previous page stopped (and is absent on the last page). Unknown keyspace → `ResourceNotFoundException` (404).
5. **DeleteTable** — input `keyspaceName` + `tableName`. Must remove the table from state (empty output body). A subsequent GetTable/ListTables must no longer see it; deleting an absent table → `ResourceNotFoundException` (404).
6. **DeleteKeyspace** — input `keyspaceName`. Must remove the keyspace (empty output). Deleting an absent keyspace → `ResourceNotFoundException` (404). A keyspace that still has tables must not silently drop them — return `ConflictException` (409) (or delete only when empty) rather than reporting success while state is inconsistent.

Every one of these must compute its response from persisted state held in the shared engine (reached via `crate::state::service_state(&*self.engine)`). No canned responses, no echoing the input back, and no minting an ARN without storing the resource: a create followed by a get/list must return exactly what was created, and a delete must be observable by the next call.

## State Model

In `src/state.rs`, on `ComAmazonawsKeyspacesKeyspacesserviceState`:

- `keyspaces: BTreeMap<String, Keyspace>` — keyed by `keyspaceName` (BTreeMap so ListKeyspaces/ListTables paging is deterministic).
- `Keyspace { name: String, resource_arn: String, replication_strategy: String /* SINGLE_REGION | MULTI_REGION */, replication_regions: Vec<String>, tags: Vec<Tag>, tables: BTreeMap<String, Table> }` — `tables` keyed by `tableName`.
- `Table { keyspace_name: String, table_name: String, resource_arn: String, creation_timestamp: SystemTime, status: String /* ACTIVE|CREATING|DELETING|... */, schema_definition: SchemaDefinition, capacity_specification: CapacitySpecification, encryption_specification: EncryptionSpecification, point_in_time_recovery: PointInTimeRecovery, ttl: Option<TimeToLive>, default_time_to_live: Option<i32>, comment: Option<Comment>, client_side_timestamps: Option<ClientSideTimestamps>, tags: Vec<Tag> }`.
- Optional helper for paging: encode `nextToken` as the last-returned key (base64 of `keyspaceName#tableName`) and resume with a range scan over the BTreeMap.

Out of scope for this first slice (stay 501, filed separately later): CreateType/GetType/DeleteType/ListTypes, GetKeyspace/ListKeyspaces/UpdateKeyspace, UpdateTable, RestoreTable, GetTableAutoScalingSettings, TagResource/UntagResource/ListTagsForResource.

### Done when
A realistic integration test drives CreateKeyspace → CreateTable → GetTable → ListTables → DeleteTable → DeleteKeyspace against one running emulator and asserts not just 2xx but the body contents: the returned schemaDefinition equals the one submitted, the ARNs match between create and get, paging is honoured, and the post-delete reads return 404 ResourceNotFoundException.

## Evidence

1) `grep -rn '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/` matches all 19 operation files (create_keyspace.rs, create_table.rs, get_table.rs, list_tables.rs, delete_table.rs, delete_keyspace.rs, …), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501 with zero stored state.
2) `services/service-com-amazonaws-keyspaces-keyspacesservice/OPERATIONS.md` (rendered from `smithy-model/com-amazonaws-keyspaces-keyspacesservice.json`) documents the wire protocol (`awsJson1_0`, target prefix `KeyspacesService.`, sigv4 name `cassandra`) and, for each of the six chosen operations, the required members and outputs cited above: CreateKeyspace→`resourceArn` required; CreateTable→`keyspaceName`/`tableName`/`schemaDefinition` required, `resourceArn` required; GetTable→`keyspaceName`/`tableName`/`resourceArn` required plus `creationTimestamp`, `status` (ACTIVE|CREATING|DELETED|DELETING|…), `schemaDefinition`, `capacitySpecification`, `encryptionSpecification`, `pointInTimeRecovery`, `ttl`, `defaultTimeToLive`, `comment`; ListTables→`@paginated(inputToken=nextToken, outputToken=nextToken, items=tables)` with `maxResults` range 1..1000; DeleteTable/DeleteKeyspace→no output members. All six declare `ResourceNotFoundException (404)`, `ValidationException (400)`, and the create/delete ops additionally `ConflictException (409)`, which grounds the error semantics requested.
3) Well-established Keyspaces behaviour grounds the defaults named above (PAY_PER_REQUEST throughput mode, AWS_OWNED_KMS_KEY encryption, point-in-time-recovery DISABLED, table status ACTIVE after creation).
4) No tests exist for these operations: `tests/` contains only the generated `state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Keyspaces client at it (`aws keyspaces --endpoint-url <emulator>`).
2. `CreateKeyspace {"keyspaceName":"demo_ks"}` → expect 200 with a `resourceArn` like `arn:aws:cassandra:us-east-1:000000000000:/keyspace/demo_ks/`; today: HTTP 501 NotImplemented.
3. `CreateTable {"keyspaceName":"demo_ks","tableName":"demo_tbl","schemaDefinition":{"allColumns":[{"name":"pk","type":"text"},{"name":"v","type":"int"}],"partitionKeys":[{"name":"pk"}]}}` → expect 200 with the table `resourceArn`; today: 501.
4. `GetTable {"keyspaceName":"demo_ks","tableName":"demo_tbl"}` → expect 200 echoing the stored schemaDefinition exactly, `status":"ACTIVE"`, a real `creationTimestamp`, and the same ARN returned by CreateTable; today: 501.
5. `ListTables {"keyspaceName":"demo_ks","maxResults":1}` → expect the table in `tables` as a TableSummary; after adding a second table, maxResults=1 must return one item plus a `nextToken`, and the follow-up call with that token must return the other item and no token; today: 501.
6. `DeleteTable {"keyspaceName":"demo_ks","tableName":"demo_tbl"}` → expect 200 empty body, then `GetTable` on it → 404 ResourceNotFoundException and `ListTables` no longer lists it; today: 501.
7. `DeleteKeyspace {"keyspaceName":"demo_ks"}` → expect 200 empty body; repeating it → 404 ResourceNotFoundException; today: 501.
8. `GetTable` with `keyspaceName":"no_such_ks"` → expect 404 ResourceNotFoundException, and `CreateKeyspace {"keyspaceName":"bad name!"}` → expect 400 ValidationException.
