Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Amazon Forecast emulator

_Filed: 2026-10-21T21:08:27Z_
_Operation: service-com-amazonaws-forecast-amazonforecast | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 63 handlers in `services/service-com-amazonaws-forecast-amazonforecast/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds zero state, so no Forecast client can do anything at all. Implement a first, SMALL stateful slice: the dataset / dataset-group lifecycle, which is the entry point of every real Forecast workflow (datasets are created, grouped, inspected, listed, then deleted).

Happy path to implement, in order (wire protocol `awsJson1_1`, `POST /` with `X-Amz-Target: AmazonForecast.<Operation>`):

1. **CreateDataset** — input `DatasetName` (required, 1..63, `^[a-zA-Z][a-zA-Z0-9_]*$`), `Domain` (required enum CUSTOM|EC2_CAPACITY|INVENTORY_PLANNING|METRICS|RETAIL|WEB_TRAFFIC|WORK_FORCE), `DatasetType` (required enum ITEM_METADATA|RELATED_TIME_SERIES|TARGET_TIME_SERIES), `Schema` (required), optional `DataFrequency`, `EncryptionConfig`, `Tags`. Output `DatasetArn` — mint a real ARN of the form `arn:aws:forecast:<region>:<account>:dataset/<DatasetName>` and STORE the record under it. A second CreateDataset with the same name must fail `ResourceAlreadyExistsException` (HTTP 403).
2. **DescribeDataset** — input `DatasetArn` (required). Output must be read back from the stored record: `DatasetArn`, `DatasetName`, `Domain`, `DatasetType`, `DataFrequency`, `Schema`, `EncryptionConfig`, `Status` (`ACTIVE` for a created-but-unimported dataset), `CreationTime`, `LastModificationTime`. Unknown ARN -> `ResourceNotFoundException` (HTTP 404).
3. **CreateDatasetGroup** — input `DatasetGroupName` (required), `Domain` (required), optional `DatasetArns`, `Tags`. Output `DatasetGroupArn` (`arn:aws:forecast:<region>:<account>:dataset-group/<name>`), stored. Each ARN in `DatasetArns` must exist in state or the call fails `ResourceNotFoundException` (404); duplicate group name -> `ResourceAlreadyExistsException` (403).
4. **DescribeDatasetGroup** — input `DatasetGroupArn` (required). Output from state: `DatasetGroupName`, `DatasetGroupArn`, `DatasetArns` (exactly the membership last written, including by UpdateDatasetGroup), `Domain`, `Status` (`ACTIVE`), `CreationTime`, `LastModificationTime`. Unknown ARN -> 404.
5. **UpdateDatasetGroup** — input `DatasetGroupArn` (required), `DatasetArns` (required). REPLACES the group membership (not append) and bumps `LastModificationTime`; empty output body. Unknown group or unknown member dataset -> `ResourceNotFoundException` (404). The next DescribeDatasetGroup must show the new list.
6. **ListDatasets** / **ListDatasetGroups** — `@paginated(inputToken=NextToken, outputToken=NextToken, items=Datasets|DatasetGroups)`, input `MaxResults` (1..100), `NextToken` (1..3000). Must enumerate exactly what is in state as `DatasetSummary` / `DatasetGroupSummary` entries, honour `MaxResults` by truncating and returning an opaque `NextToken`, resume from that token, and omit `NextToken` on the last page. A malformed/stale token -> `InvalidNextTokenException` (HTTP 400).
7. **DeleteDataset** / **DeleteDatasetGroup** — input the respective ARN (required), empty output, `@idempotent`. Must remove the record from state so a following Describe returns `ResourceNotFoundException` (404) and the List no longer includes it. Deleting a dataset that is still a member of a dataset group must fail `ResourceInUseException` (HTTP 409).

No canned, echoed or hardcoded responses: every field of every Describe/List response must be computed from the persisted record written by the corresponding Create/Update call, and ARNs must be minted AND stored (a random ARN with no backing record is a failure). Timestamps must be set at write time and preserved across calls.

## State Model

In `src/state.rs` (`ComAmazonawsForecastAmazonforecastState`, reached via `crate::state::service_state(&*self.engine)`):

- `datasets: HashMap<String /*DatasetArn*/, Dataset>` where
  `Dataset { arn: String, name: String, domain: Domain, dataset_type: DatasetType, data_frequency: Option<String>, schema: Schema, encryption_config: Option<EncryptionConfig>, status: String /*"ACTIVE"*/, creation_time: SystemTime, last_modification_time: SystemTime, tags: Vec<Tag> }`
- `dataset_groups: HashMap<String /*DatasetGroupArn*/, DatasetGroup>` where
  `DatasetGroup { arn: String, name: String, domain: Domain, dataset_arns: Vec<String>, status: String /*"ACTIVE"*/, creation_time: SystemTime, last_modification_time: SystemTime, tags: Vec<Tag> }`
- name-uniqueness is enforced by scanning (or a secondary `HashMap<String /*name*/, String /*arn*/>` index per resource type) so duplicate Creates can raise `ResourceAlreadyExistsException`.
- `dataset_arns` on the group is the single source of truth for `ResourceInUseException` on DeleteDataset.

All other operations (predictors, forecasts, import jobs, explainability, what-if, monitors, tagging, Stop/ResumeResource, DeleteResourceTree) stay scaffolded at 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-forecast-amazonforecast/OPERATIONS.md` lists all 63 operations with their members: CreateDataset (lines 33-37: required DatasetName/Domain/DatasetType/Schema -> DatasetArn; errors InvalidInput 400, LimitExceeded 409, ResourceAlreadyExists 403), CreateDatasetGroup (39-43), DescribeDataset (216-221) and DescribeDatasetGroup (223-228) which return the full stored record plus Status/CreationTime/LastModificationTime and declare ResourceNotFoundException 404, ListDatasets/ListDatasetGroups (314-333) which carry `@paginated(inputToken=NextToken, outputToken=NextToken, items=...)`, MaxResults range 1..100 and InvalidNextTokenException 400, DeleteDataset/DeleteDatasetGroup (111-123) which are `@idempotent` and declare ResourceInUseException 409 + ResourceNotFoundException 404, and UpdateDatasetGroup (444-449, required DatasetGroupArn + DatasetArns). Wire protocol from OPERATIONS.md lines 10-15: awsJson1_1, target prefix `AmazonForecast.`, sdkId/signing name `forecast`. Every handler file still carries the scaffold marker — e.g. `src/ops/create_dataset.rs` line 4 `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 209 `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDataset"))`. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Forecast client at it.
2. CreateDataset DatasetName=my_ts Domain=RETAIL DatasetType=TARGET_TIME_SERIES Schema={Attributes:[{AttributeName:item_id,AttributeType:string},{AttributeName:timestamp,AttributeType:timestamp},{AttributeName:demand,AttributeType:float}]} -> expect 200 with a forecast dataset ARN (today: HTTP 501 NotImplemented).
3. DescribeDataset with that ARN -> expect DatasetName=my_ts, Domain=RETAIL, DatasetType=TARGET_TIME_SERIES, the Schema submitted, Status=ACTIVE, CreationTime set.
4. CreateDataset again with DatasetName=my_ts -> expect ResourceAlreadyExistsException (HTTP 403).
5. CreateDatasetGroup DatasetGroupName=my_group Domain=RETAIL DatasetArns=[<dataset arn>] -> expect 200 with a dataset-group ARN; DescribeDatasetGroup must echo DatasetArns=[<dataset arn>] from state.
6. UpdateDatasetGroup DatasetGroupArn=<group arn> DatasetArns=[] -> expect 200; DescribeDatasetGroup now reports an empty DatasetArns and a bumped LastModificationTime.
7. ListDatasets with MaxResults=1 after creating two datasets -> expect one DatasetSummary plus a NextToken; call again with that NextToken -> expect the second dataset and no NextToken. A garbage NextToken -> InvalidNextTokenException (HTTP 400).
8. DeleteDataset while the dataset is still listed in the group's DatasetArns -> expect ResourceInUseException (HTTP 409); after UpdateDatasetGroup removes it, DeleteDataset -> 200 and DescribeDataset -> ResourceNotFoundException (HTTP 404), and ListDatasets no longer includes it.
9. DeleteDatasetGroup <group arn> -> 200; DescribeDatasetGroup -> ResourceNotFoundException (HTTP 404).
