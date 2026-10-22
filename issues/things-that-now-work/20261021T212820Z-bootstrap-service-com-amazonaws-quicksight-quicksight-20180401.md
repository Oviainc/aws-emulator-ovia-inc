Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the QuickSight emulator

_Filed: 2026-10-21T21:28:20Z_
_Operation: service-com-amazonaws-quicksight-quicksight-20180401 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 299 QuickSight operations is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state (`//! SCAFFOLDED, NOT IMPLEMENTED:` header in each `src/ops/*.rs`). Implement a FIRST SMALL SLICE: the data-source -> data-set lifecycle, which is the core QuickSight ingestion workflow and the prerequisite for everything else (analyses, dashboards, ingestions).

Happy path to implement, in order (protocol: restJson1, sigv4 name `quicksight`):

1. **CreateDataSource** — `POST /accounts/{AwsAccountId}/data-sources`. Input: `AwsAccountId` (@httpLabel, 12-digit), `DataSourceId` (required), `Name` (required, 1..128), `Type` (DataSourceType enum, required), plus optional `DataSourceParameters`, `Credentials`, `Permissions`, `SslProperties`, `VpcConnectionProperties`, `Tags`. Store the record; respond with `Arn` (`arn:aws:quicksight:<region>:<AwsAccountId>:datasource/<DataSourceId>`), `DataSourceId`, `CreationStatus` = `CREATION_SUCCESSFUL`, `RequestId`, `Status` = 200. Re-creating an existing `DataSourceId` in the same account must return `ResourceExistsException` (409).
2. **DescribeDataSource** — `GET /accounts/{AwsAccountId}/data-sources/{DataSourceId}`. Must return the `DataSource` structure rebuilt from the stored record: `DataSourceId`, `Name`, `Arn`, `Type`, `Status`, `CreatedTime`, `LastUpdatedTime`, `DataSourceParameters`, `SslProperties`. Unknown id -> `ResourceNotFoundException` (404). `Credentials` are never echoed back.
3. **CreateDataSet** — `POST /accounts/{AwsAccountId}/data-sets`. Input: `DataSetId` (required), `Name` (required, 1..128), `PhysicalTableMap` (map<string,PhysicalTable>, required), `ImportMode` (`SPICE`|`DIRECT_QUERY`, required), optional `LogicalTableMap`, `ColumnGroups`, `Permissions`, `Tags`, `RowLevelPermissionDataSet`. Persist it (including the physical/logical table maps) and return `Arn`, `DataSetId`, `RequestId`, `Status` = 200; for `ImportMode=SPICE` also mint and store an `IngestionId`/`IngestionArn`. A `PhysicalTable` whose referenced `DataSourceArn` is not a stored data source must fail with `ResourceNotFoundException` (404) rather than succeed.
4. **DescribeDataSet** — `GET /accounts/{AwsAccountId}/data-sets/{DataSetId}`. Return the `DataSet` structure from state: `DataSetId`, `Name`, `Arn`, `CreatedTime`, `LastUpdatedTime`, `ImportMode`, `PhysicalTableMap`, `LogicalTableMap`, `OutputColumns`, `ConsumedSpiceCapacityInBytes`. Unknown id -> 404.
5. **ListDataSets** — `GET /accounts/{AwsAccountId}/data-sets`, `@paginated(inputToken=NextToken, outputToken=NextToken, items=DataSetSummaries)`. Return one `DataSetSummary` (`DataSetId`, `Name`, `Arn`, `CreatedTime`, `LastUpdatedTime`, `ImportMode`, `RowLevelPermissionDataSet`) per stored data set in the account, honouring `MaxResults` (1..100) and returning/consuming a real `NextToken`; an unparseable token -> `InvalidNextTokenException` (400).
6. **DeleteDataSet** / **DeleteDataSource** — `DELETE /accounts/{AwsAccountId}/data-sets/{DataSetId}` and `.../data-sources/{DataSourceId}`. Remove the record from state and echo `Arn`, the id, `RequestId`, `Status` = 200. A subsequent Describe must return `ResourceNotFoundException` (404), and the deleted item must disappear from ListDataSets/ListDataSources.

NEVER STUBS: each of these must compute its response from the persisted store — no canned bodies, no echoing the request back, no minting an id that is never stored. `AwsAccountId` is part of every key, so resources created under one account id are invisible under another. Validate the modelled constraints (12-digit account id, id patterns, name lengths, enum values) and return `InvalidParameterValueException` (400) when they are violated.

## State Model

In `src/state.rs` (`ComAmazonawsQuicksightQuicksight20180401State`), reached through `crate::state::service_state(&*self.engine)`:

- `data_sources: HashMap<(AwsAccountId, DataSourceId), DataSourceRecord>` where `DataSourceRecord { arn, data_source_id, name, r#type: DataSourceType, status: ResourceStatus, created_time, last_updated_time, data_source_parameters, ssl_properties, vpc_connection_properties, permissions: Vec<ResourcePermission>, tags: Vec<Tag>, error_info }` (credentials deliberately not retained in responses).
- `data_sets: HashMap<(AwsAccountId, DataSetId), DataSetRecord>` where `DataSetRecord { arn, data_set_id, name, created_time, last_updated_time, import_mode, physical_table_map: HashMap<String, PhysicalTable>, logical_table_map: HashMap<String, LogicalTable>, output_columns: Vec<OutputColumn>, column_groups, row_level_permission_data_set, permissions: Vec<ResourcePermission>, tags: Vec<Tag>, consumed_spice_capacity_in_bytes }`.
- `ingestions: HashMap<(AwsAccountId, DataSetId, IngestionId), IngestionRecord { arn, ingestion_id, ingestion_status, created_time, request_source }>` so the SPICE `IngestionId` returned by CreateDataSet is a real stored object.
- A monotonic `request_counter` (or UUID generator) used to mint each response's `RequestId`; ARNs are derived from the account id + resource id, never random.

Every other QuickSight operation (analyses, dashboards, templates, folders, users/groups, namespaces, topics, Q, embedding URLs, ...) stays 501 for now and will be filed as individual gap issues.

## Evidence

services/service-com-amazonaws-quicksight-quicksight-20180401/OPERATIONS.md declares all 299 operations with protocol restJson1 (sdkId QuickSight, sigv4 `quicksight`), including CreateDataSource (line 132), CreateDataSet (line 125), DescribeDataSet (line 811), DescribeDataSource (line 832), ListDataSets (line 1275, @paginated NextToken/DataSetSummaries), ListDataSources (line 1283), DeleteDataSet (line 390) and DeleteDataSource (line 404) with the exact input/output members and the ResourceNotFoundException (404) / ResourceExistsException (409) / InvalidParameterValueException (400) / InvalidNextTokenException (400) errors cited above. `grep -n 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/*data_s*.rs` shows create_data_source.rs, describe_data_source.rs, create_data_set.rs, describe_data_set.rs, list_data_sets.rs, list_data_sources.rs, delete_data_set.rs and delete_data_source.rs are all unmodified scaffolds returning `Scaffolded::not_implemented(..)` (HTTP 501). README.md confirms the 501-until-implemented contract and that handlers must use the shared engine store. No tests assert any of this behaviour (only the generated tests/state_roundtrip.rs exists).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI QuickSight client at it with account id 123456789012.
2. CreateDataSource with DataSourceId=ds-src-1, Name='My Athena', Type=ATHENA -> expect 200 with Arn arn:aws:quicksight:<region>:123456789012:datasource/ds-src-1 and CreationStatus=CREATION_SUCCESSFUL (currently 501).
3. DescribeDataSource for ds-src-1 -> expect 200 with DataSource.Name='My Athena', Type=ATHENA and a CreatedTime (must come from state, not be echoed).
4. Repeat CreateDataSource with DataSourceId=ds-src-1 -> expect ResourceExistsException (HTTP 409).
5. CreateDataSet with DataSetId=dset-1, Name='Sales', ImportMode=SPICE and a PhysicalTableMap entry referencing the ds-src-1 ARN -> expect 200 with Arn, DataSetId and a non-empty IngestionId.
6. DescribeDataSet for dset-1 -> expect 200 with the same Name, ImportMode=SPICE and the PhysicalTableMap that was submitted.
7. ListDataSets with MaxResults=1 -> expect exactly one DataSetSummary for dset-1 plus a NextToken; follow the token to exhaustion and confirm no duplicates.
8. DeleteDataSet dset-1 then DescribeDataSet dset-1 -> expect ResourceNotFoundException (HTTP 404), and ListDataSets no longer returns it.
9. DeleteDataSource ds-src-1 then DescribeDataSource ds-src-1 -> expect ResourceNotFoundException (HTTP 404).
10. Repeat the DescribeDataSource/DescribeDataSet calls with AwsAccountId=210987654321 -> expect 404, proving state is keyed per account.
