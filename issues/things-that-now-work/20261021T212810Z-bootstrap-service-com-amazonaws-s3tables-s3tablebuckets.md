Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the S3Tables (table buckets) emulator

_Filed: 2026-10-21T21:28:10Z_
_Operation: service-com-amazonaws-s3tables-s3tablebuckets | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 49 operations in `services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, holding zero state. Implement a FIRST, SMALL, stateful slice: the table-bucket -> namespace -> table lifecycle, which is the service's core workflow (restJson1, sigv4 signing name `s3tables`).

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateTableBucket** — `PUT /buckets`. Input: `name` (required, len 3..63, `^[0-9a-z-]*$`), optional `encryptionConfiguration`, `storageClassConfiguration`, `tags`. Output: `arn` (required) which MUST be a freshly minted, persisted ARN of the modeled form `arn:aws:s3tables:<region>:<12-digit-account>:bucket/<name>`. Record `createdAt`, `ownerAccountId`, `tableBucketId`, `type=customer`. Creating a bucket whose name already exists -> `ConflictException` (409).
2. **CreateNamespace** — `PUT /namespaces/{tableBucketARN}`. Input: `tableBucketARN` (@httpLabel, required), `namespace` (list<string>, required). Output echoes `tableBucketARN` + `namespace`, but only after the namespace is STORED under the bucket resolved from the ARN. Unknown bucket ARN -> `NotFoundException` (404); duplicate namespace -> `ConflictException` (409).
3. **CreateTable** — `PUT /tables/{tableBucketARN}/{namespace}`. Input: `tableBucketARN`, `namespace` (@httpLabel), `name` (required, len 1..255, `^[0-9a-z_]*$`), `format` (required, `ICEBERG`), optional `metadata`, `encryptionConfiguration`, `storageClassConfiguration`, `tags`. Output: `tableARN` (required, `.../bucket/<bucket>/table/<name-or-id>`) and `versionToken` (required, len 1..2048) — both generated and persisted. Unknown bucket/namespace -> 404; duplicate table name in the namespace -> 409.
4. **GetTable** — `GET /get-table` with query params `tableBucketARN`+`namespace`+`name` OR `tableArn`. Must look the table up in state and return the stored values: `name`, `type`, `tableARN`, `namespace` (list), `versionToken`, `warehouseLocation`, `metadataLocation` (optional), `createdAt`, `createdBy`, `modifiedAt`, `modifiedBy`, `ownerAccountId`, `format`, plus `namespaceId`/`tableBucketId`. Both addressing modes must resolve the SAME stored record. Unknown table -> `NotFoundException` (404).
5. **ListTables** — `GET /tables/{tableBucketARN}` with optional `namespace`, `prefix`, `maxTables` (1..1000), `continuationToken`. Must return `tables` as `TableSummary{namespace, name, type, tableARN, createdAt, modifiedAt, namespaceId, tableBucketId}` built from state, filtered by namespace/prefix, truncated to `maxTables`, with `continuationToken` set only when more results remain and honoured on the follow-up call.
6. **ListTableBuckets** — `GET /buckets` with optional `prefix`, `maxBuckets` (1..1000), `type`, `continuationToken`. Must return `tableBuckets` as `TableBucketSummary{arn, name, ownerAccountId, createdAt, tableBucketId, type}` from state, with the same filtering/pagination rules.
7. **DeleteTable** — `DELETE /tables/{tableBucketARN}/{namespace}/{name}` -> 204, removing the record so a subsequent GetTable returns 404. Then **DeleteTableBucket** — `DELETE /buckets/{tableBucketARN}` -> 204, which must refuse with `ConflictException` (409) while the bucket still contains namespaces/tables, and after success make GetTableBucket return 404 and drop the bucket from ListTableBuckets. (Include **GetTableBucket** — `GET /buckets/{tableBucketARN}` returning stored `arn`, `name`, `ownerAccountId`, `createdAt`, `tableBucketId`, `type` — as the read side of the bucket lifecycle.)

Every one of these MUST compute its response from persisted state held in the shared engine store reached via `crate::state::service_state(&*self.engine)`. No canned bodies, no echoing the input back, no minting an ARN/versionToken that is never stored. A create followed by a get/list must return exactly what was created; a delete followed by a get must 404.

## State Model

In `src/state.rs` (the per-service state struct reached through `service_state`):

- `table_buckets: BTreeMap<String /* bucket ARN */, TableBucket>` — insertion/name ordering stable for pagination; a secondary `name -> arn` lookup for duplicate-name conflict detection.
- `TableBucket { arn: String, name: String, owner_account_id: String /* 12 digits */, created_at: SystemTime, table_bucket_id: String, bucket_type: TableBucketType /* customer */, encryption_configuration: Option<EncryptionConfiguration>, storage_class_configuration: Option<StorageClassConfiguration>, tags: BTreeMap<String,String>, namespaces: BTreeMap<String /* namespace name */, Namespace> }`
- `Namespace { namespace: Vec<String>, namespace_id: String, created_at: SystemTime, created_by: String, owner_account_id: String, tables: BTreeMap<String /* table name */, Table> }`
- `Table { name: String, table_arn: String, table_type: TableType, namespace: Vec<String>, namespace_id: String, table_bucket_id: String, format: OpenTableFormat /* ICEBERG */, version_token: String /* regenerated on every mutation */, metadata_location: Option<String>, warehouse_location: String, created_at: SystemTime, created_by: String, modified_at: SystemTime, modified_by: String, owner_account_id: String, encryption_configuration: Option<EncryptionConfiguration>, storage_class_configuration: Option<StorageClassConfiguration>, tags: BTreeMap<String,String> }`
- A `table_arn -> (bucket_arn, namespace, table_name)` index so GetTable's `tableArn` addressing mode and the ARN-keyed replication/record-expiration ops later resolve without a scan.
- Pagination: `continuationToken` is an opaque encoding of the last-returned key (bucket ARN / namespace+table name) so a resumed list continues from the stored ordering rather than restarting.

All 40+ operations outside this slice (encryption, policies, maintenance, metrics, storage class, replication, record expiration, tagging, RenameTable, UpdateTableMetadataLocation, GetTableMetadataLocation, DeleteNamespace/GetNamespace/ListNamespaces beyond what the slice needs) stay at 501 and will be filed as individual gap issues later. Keep this first slice small.

## Evidence

Spec: `services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json` as denormalized in the crate's `OPERATIONS.md` (49 operations, protocol `restJson1`, SDK id `S3Tables`): CreateTableBucket `PUT /buckets` (in: name/encryptionConfiguration/storageClassConfiguration/tags; out: arn required), GetTableBucket `GET /buckets/{tableBucketARN}` (out: arn, name, ownerAccountId, createdAt, tableBucketId, type), ListTableBuckets `GET /buckets` (@paginated inputToken=continuationToken, items=tableBuckets, maxBuckets 1..1000), DeleteTableBucket `DELETE /buckets/{tableBucketARN}` -> 204, CreateNamespace `PUT /namespaces/{tableBucketARN}`, CreateTable `PUT /tables/{tableBucketARN}/{namespace}` (out: tableARN + versionToken required), GetTable `GET /get-table` (query tableBucketARN/namespace/name or tableArn), ListTables `GET /tables/{tableBucketARN}` (@paginated items=tables, maxTables 1..1000), all declaring NotFoundException (404) and ConflictException (409). Summary shapes confirmed via smithy_query: TableBucketSummary, TableSummary, NamespaceSummary field lists.
Implementation: every file under `src/ops/` matches the scaffold census marker — e.g. `src/ops/create_table_bucket.rs:4` `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 189 `Err(crate::scaffolded::Scaffolded::not_implemented("CreateTableBucket"))`. No handler reads or writes `crate::state::service_state`, so nothing persists and no test asserts any stateful behaviour (only the generated `tests/state_roundtrip.rs` example exists).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI s3tables client at it.
2. `aws s3tables create-table-bucket --name my-test-bucket` -> expect 200 with a stable `arn` of the form arn:aws:s3tables:<region>:<account>:bucket/my-test-bucket (currently 501 NotImplemented).
3. `aws s3tables list-table-buckets` -> expect the new bucket present in `tableBuckets` with name/arn/createdAt/ownerAccountId matching the create.
4. `aws s3tables get-table-bucket --table-bucket-arn <arn>` -> expect the same name/arn/createdAt that create persisted.
5. `aws s3tables create-namespace --table-bucket-arn <arn> --namespace my_ns` -> expect 200 echoing the stored namespace; repeating it -> 409 ConflictException.
6. `aws s3tables create-table --table-bucket-arn <arn> --namespace my_ns --name my_table --format ICEBERG` -> expect 200 with `tableARN` and `versionToken`.
7. `aws s3tables get-table --table-bucket-arn <arn> --namespace my_ns --name my_table` and `aws s3tables get-table --table-arn <tableARN>` -> expect both to return the SAME stored record (name, namespace, versionToken, format=ICEBERG, createdAt, warehouseLocation).
8. `aws s3tables list-tables --table-bucket-arn <arn> --namespace my_ns --max-tables 1` -> expect exactly 1 table and a `continuationToken` only if more exist; resume with the token and expect no duplicates or repeats.
9. `aws s3tables delete-table-bucket --table-bucket-arn <arn>` while the table exists -> expect 409 ConflictException (bucket not empty).
10. `aws s3tables delete-table --table-bucket-arn <arn> --namespace my_ns --name my_table` -> 204; then get-table -> 404 NotFoundException.
11. `aws s3tables delete-namespace --table-bucket-arn <arn> --namespace my_ns` then `delete-table-bucket` -> 204; then `get-table-bucket` -> 404 and the bucket is absent from `list-table-buckets`.
