Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the glue emulator

_Filed: 2026-10-21T21:09:01Z_
_Operation: service-com-amazonaws-glue-awsglue | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 306 operations in `services/service-com-amazonaws-glue-awsglue/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, and the crate holds no state. No client workflow works at all.

The first buildable slice is the **Glue Data Catalog** lifecycle — the primary resource pair (database -> table) that Athena, EMR, Spark, Terraform and `aws glue` CLI users touch first. Implement these 6 operations statefully, in this order (wire protocol: `awsJson1_1`, `POST /` with `X-Amz-Target: AWSGlue.<OperationName>`):

1. **CreateDatabase** — input `CatalogId` (optional, default to the request account id), `DatabaseInput` (required: `Name`, plus `Description`, `LocationUri`, `Parameters`, `CreateTableDefaultPermissions`, `TargetDatabase`), `Tags`. Output has no members. Must persist a new Database record keyed by (CatalogId, Name) with `CreateTime` set to now; a second create of the same name must fail with `AlreadyExistsException` (HTTP 400), not succeed.
2. **GetDatabase** — input `CatalogId`, `Name` (required). Output `Database` must be read back from the stored record and echo exactly what CreateDatabase persisted (`Name`, `Description`, `LocationUri`, `Parameters`, `CatalogId`, `CreateTime`). Unknown name => `EntityNotFoundException` (HTTP 400).
3. **CreateTable** — input `CatalogId`, `DatabaseName` (required), `TableInput` (`Name` required, plus `Description`, `Owner`, `StorageDescriptor`, `PartitionKeys`, `TableType`, `Parameters`, `Retention`), `PartitionIndexes`. Output has no members. Must persist the table inside its parent database and set server-derived fields `DatabaseName`, `CatalogId`, `CreateTime`, `UpdateTime`, `VersionId` (start at "1"). Nonexistent `DatabaseName` => `EntityNotFoundException`; duplicate table name => `AlreadyExistsException`.
4. **GetTable** — input `CatalogId`, `DatabaseName` (required), `Name` (required). Output `Table` must be rebuilt from the stored record: the `StorageDescriptor` (incl. `Columns`, `Location`, `InputFormat`, `SerdeInfo`) and `PartitionKeys` sent to CreateTable must come back byte-for-byte, with `CreateTime`/`UpdateTime`/`VersionId`/`DatabaseName` populated by the server. Unknown table or database => `EntityNotFoundException`.
5. **GetTables** — input `CatalogId`, `DatabaseName` (required), `Expression`, `MaxResults` (1..100), `NextToken`. Output `TableList` + `NextToken`. Must list the tables actually stored in that database, honour `MaxResults` by truncating and returning a `NextToken` that resumes where the page ended, and apply the `Expression` glob filter to table names. Must NOT return a fixed-size or canned list.
6. **DeleteTable** / **DeleteDatabase** — input `CatalogId`, `DatabaseName`+`Name` (DeleteTable) or `Name` (DeleteDatabase); no output members. Must remove the record so a subsequent GetTable/GetDatabase returns `EntityNotFoundException`, and DeleteDatabase must cascade-remove the database's tables (real Glue deletes contained tables). Deleting something absent => `EntityNotFoundException`.

Every one of these must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)`. No canned responses, no echoing the input back without storing it, no generated identifier that is not written to the store: a `CreateTable` followed by `GetTable` in a separate request must return what was created. Done = a client script (boto3 / aws CLI against the emulator) runs create-database -> get-database -> create-table -> get-table -> get-tables -> delete-table -> delete-database with correct bodies at each step, plus the four documented not-found/already-exists error cases.

## State Model

In `src/state.rs` (`ComAmazonawsGlueAwsglueState`), add a Data Catalog store:

- `catalogs: HashMap<String /*CatalogId, default = account id*/, Catalog>`
- `Catalog { databases: HashMap<String /*database name, lowercased for Hive compat*/, DatabaseEntry> }`
- `DatabaseEntry { name: String, description: Option<String>, location_uri: Option<String>, parameters: HashMap<String,String>, create_table_default_permissions: Vec<PrincipalPermissions>, target_database: Option<DatabaseIdentifier>, catalog_id: String, create_time: SystemTime, tags: HashMap<String,String>, tables: HashMap<String /*table name*/, TableEntry> }`
- `TableEntry { name: String, database_name: String, catalog_id: String, description: Option<String>, owner: Option<String>, table_type: Option<String>, retention: i32, parameters: HashMap<String,String>, storage_descriptor: Option<StorageDescriptor>, partition_keys: Vec<Column>, view_original_text: Option<String>, view_expanded_text: Option<String>, create_time: SystemTime, update_time: SystemTime, last_access_time: Option<SystemTime>, version_id: String, created_by: Option<String> }`

`TableEntry.partitions` and a `versions: Vec<TableEntry>` history field are the natural next additions (CreatePartition/GetPartitions, GetTableVersions) but are OUT of scope for this slice.

All other Glue operations (jobs, crawlers, triggers, workflows, sessions, registries/schemas, connections, data quality, integrations, glossaries, ML transforms, …) stay at 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-glue-awsglue/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-glue-awsglue.json`) states "306 operations" and documents the exact contracts cited above: CreateDatabase (line 233: `DatabaseInput` required, `AlreadyExistsException`), DeleteDatabase (461), DeleteTable (584), GetDatabase (799: output `Database`, `EntityNotFoundException`), GetDatabases (805: `@paginated(inputToken=NextToken, outputToken=NextToken)`, `MaxResults` range 1..100), CreateTable (344: `DatabaseName` required, `TableInput`), GetTable (1117: output `Table`), GetTables (1129: `TableList` + `NextToken`, `Expression`, `MaxResults` 1..100). `smithy_query shape=Database,DatabaseInput,Table,TableInput` confirms the member sets and the server-derived members (`Database.CreateTime`, `Table.CreateTime/UpdateTime/VersionId/CatalogId/DatabaseName` exist on the output shapes but not on the `*Input` shapes, so the handler must synthesize and persist them). `grep` of `src/ops/create_database.rs` shows the scaffold header at line 4 and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDatabase"))` at line 172 — representative of all 306 handler files. README.md documents the awsJson1_1 `X-Amz-Target: AWSGlue.<Op>` routing and the `crate::state::service_state(&*self.engine)` shared-store access pattern. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point a Glue client at it (`aws --endpoint-url=http://localhost:<port> glue ...`).
2. `CreateDatabase {"DatabaseInput": {"Name": "bootstrap_db", "Description": "demo", "LocationUri": "s3://bucket/db/"}}` — expect HTTP 200 with an empty JSON body; today it returns HTTP 501 NotImplemented.
3. `GetDatabase {"Name": "bootstrap_db"}` — expect `Database.Name == "bootstrap_db"`, `Description == "demo"`, `LocationUri` echoed from the create, and a populated `CreateTime`/`CatalogId`.
4. `CreateDatabase` the same name again — expect `AlreadyExistsException` (HTTP 400).
5. `CreateTable {"DatabaseName": "bootstrap_db", "TableInput": {"Name": "events", "TableType": "EXTERNAL_TABLE", "StorageDescriptor": {"Columns": [{"Name": "id", "Type": "string"}], "Location": "s3://bucket/events/"}, "PartitionKeys": [{"Name": "dt", "Type": "string"}]}}` — expect HTTP 200.
6. `GetTable {"DatabaseName": "bootstrap_db", "Name": "events"}` — expect `Table.StorageDescriptor.Columns` and `PartitionKeys` identical to what was sent, plus server-set `DatabaseName`, `CatalogId`, `CreateTime`, `UpdateTime`, `VersionId`.
7. `CreateTable` two more tables, then `GetTables {"DatabaseName": "bootstrap_db", "MaxResults": 2}` — expect exactly 2 entries and a `NextToken`; pass that token back and expect the remaining table and no token.
8. `GetTables {"DatabaseName": "bootstrap_db", "Expression": "ev*"}` — expect only `events`.
9. `DeleteTable {"DatabaseName": "bootstrap_db", "Name": "events"}` then `GetTable` the same table — expect `EntityNotFoundException` (HTTP 400).
10. `DeleteDatabase {"Name": "bootstrap_db"}` then `GetDatabase {"Name": "bootstrap_db"}` — expect `EntityNotFoundException`, and `GetTables` on it to fail the same way (cascade delete).

## Resolution

Implemented: the Data Catalog slice is stateful in
`services/service-com-amazonaws-glue-awsglue/` — `CreateDatabase`, `GetDatabase`,
`CreateTable`, `GetTable`, `GetTables`, `DeleteTable` and `DeleteDatabase` all compute
their answer from the catalog -> database -> table store in `src/state.rs`, with the
modelled `AlreadyExistsException` / `EntityNotFoundException` / `InvalidInputException` /
`ResourceNumberLimitExceededException` failures. Covered end-to-end over `awsJson1_1` by
`tests/all/data_catalog.rs`. All ten reproduction steps behave as written.

`GetTables`'s `Expression` is served as the Hive-style pattern the Hive-metastore
compatible catalog takes — `*` matches any run of characters, `|` separates
alternatives, every other character is a literal, and the WHOLE stored name must match
— so step 8's `ev*` selects the `ev`-prefixed tables. The model's prose calls the
member "A regular expression pattern"
(`smithy_query(query="doc", shape="GetTablesRequest", member="Expression")`); the two
readings differ only on a regex metacharacter, and the pattern language is stated in
`src/ops/get_tables.rs`'s module header and pinned by the test table (prefix, suffix,
both-sided wildcard, alternation, match-everything, empty, bare prefix, and
metacharacters as literals). A pattern carrying more wildcards than the handler's
`MAX_PATTERN_WILDCARDS` ceiling is refused with `InvalidInputException`, so the work
request text can buy stays bounded.
