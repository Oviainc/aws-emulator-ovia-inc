Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 020e8396bf5d8d2121f9bed74857dd02e33cf6c0

# Bootstrap the Personalize emulator

_Filed: 2026-10-22T00:33:06Z_
_Operation: service-com-amazonaws-personalize-amazonpersonalize | Class: missing_

> _Note: this issue was identified against the code base at commit `020e8396bf5d8d2121f9bed74857dd02e33cf6c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-personalize-amazonpersonalize/src/ops/` (71 files) is an unmodified scaffold carrying `//! SCAFFOLDED, NOT IMPLEMENTED:` and returning HTTP 501 `NotImplemented`, so no Personalize workflow works at all and the service holds zero state.

Implement a FIRST minimal slice: the dataset-group lifecycle, which is the entry point of every real Personalize client (schema -> dataset group -> dataset -> list -> teardown). Wire protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: AmazonPersonalize.<OperationName>`.

Happy path to implement, in order:

1. **CreateSchema** — in: `name` (required, 1..63, `^[a-zA-Z0-9][a-zA-Z0-9\-_]*$`), `schema` (required, 0..20000 JSON text), `domain` (ECOMMERCE|VIDEO_ON_DEMAND). out: `schemaArn`. Mint a deterministic ARN `arn:aws:personalize:<region>:<account>:schema/<name>`, store the schema document; a second create with the same name must fail `ResourceAlreadyExistsException` (HTTP 403).
2. **CreateDatasetGroup** — in: `name` (required), `roleArn`, `kmsKeyArn`, `domain`, `tags`. out: `datasetGroupArn`, `domain` (echoed from the stored record, not from the request path). Mint `arn:aws:personalize:<region>:<account>:dataset-group/<name>`, persist with `status` `ACTIVE` and `creationDateTime`/`lastUpdatedDateTime`. Duplicate name -> `ResourceAlreadyExistsException`.
3. **DescribeDatasetGroup** — in: `datasetGroupArn` (required). out: `datasetGroup` (DatasetGroup{name, datasetGroupArn, status, roleArn, kmsKeyArn, creationDateTime, lastUpdatedDateTime, failureReason, domain}) reconstructed from the stored record. Unknown ARN -> `ResourceNotFoundException` (HTTP 404).
4. **CreateDataset** — in: `name` (required), `schemaArn` (required), `datasetGroupArn` (required), `datasetType` (required, e.g. Interactions|Items|Users|Actions|Action_Interactions), `tags`. out: `datasetArn` = `arn:aws:personalize:<region>:<account>:dataset/<datasetGroupName>/<DATASET_TYPE>`. Must validate that `datasetGroupArn` and `schemaArn` exist (else `ResourceNotFoundException`) and that the group does not already hold a dataset of that type (`ResourceAlreadyExistsException`). Persist `status` `ACTIVE` + timestamps.
5. **DescribeDataset** — in: `datasetArn` (required). out: `dataset` (Dataset{name, datasetArn, datasetGroupArn, datasetType, schemaArn, status, creationDateTime, lastUpdatedDateTime}) built from state; unknown ARN -> `ResourceNotFoundException`.
6. **ListDatasets** — in: `datasetGroupArn` (filter), `maxResults` (1..100), `nextToken` (0..1500). out: `datasets` (list<DatasetSummary>{name, datasetArn, datasetType, status, creationDateTime, lastUpdatedDateTime}), `nextToken` when a page was truncated. Must actually filter by group, honour `maxResults` and resume from an opaque token (bad token -> `InvalidNextTokenException`, HTTP 400). Also implement the sibling **ListDatasetGroups** (same pagination contract, items `datasetGroups` of DatasetGroupSummary) since it is how clients discover what step 2 created.
7. **DeleteDatasetGroup** / **DeleteDataset** — in: `datasetGroupArn` / `datasetArn` (required). out: no members. Delete must remove the record so the subsequent Describe/List no longer sees it; deleting a group that still holds datasets -> `ResourceInUseException` (HTTP 409); unknown ARN -> `ResourceNotFoundException`.

NEVER STUBS: each of these must compute its response from persisted state. No hardcoded ARNs that are not stored, no echoing the request back, no minting an ID without recording it — a create followed by a describe must return exactly what was created, and a delete followed by a describe must 404.

## State Model

In `src/state.rs`, on `ComAmazonawsPersonalizeAmazonpersonalizeState` (reached via `crate::state::service_state(&*self.engine)`):

- `schemas: BTreeMap<String /*schemaArn*/, Schema { name, schema_arn, schema: String, domain: Option<Domain>, creation_date_time, last_updated_date_time }>`
- `dataset_groups: BTreeMap<String /*datasetGroupArn*/, DatasetGroupRecord { name, dataset_group_arn, status: String /*"ACTIVE"*/, role_arn: Option<String>, kms_key_arn: Option<String>, domain: Option<Domain>, failure_reason: Option<String>, creation_date_time, last_updated_date_time, tags: Vec<Tag> }>`
- `datasets: BTreeMap<String /*datasetArn*/, DatasetRecord { name, dataset_arn, dataset_group_arn, dataset_type, schema_arn, status, creation_date_time, last_updated_date_time, tags: Vec<Tag> }>`
- plus a name index per resource kind (or derive ARNs from names deterministically) so duplicate-name creates can be rejected, and an `account_id`/`region` used for ARN minting.

Use ordered maps (or sort on read) so list pagination is stable and the `nextToken` can encode the last-returned key.

Done = an integration test doing CreateSchema -> CreateDatasetGroup -> DescribeDatasetGroup (status ACTIVE, name/domain match) -> CreateDataset -> DescribeDataset -> ListDatasets (dataset present, filtered by group, maxResults=1 pagination works) -> DeleteDataset -> DeleteDatasetGroup -> DescribeDatasetGroup returns 404, all with correct bodies, not just 2xx.

All other operations (solutions, solution versions, campaigns, recommenders, filters, event trackers, import/export/batch jobs, metric attributions, tagging) stay 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md (crate root) lists 71 operations for `awsJson1_1` service `Personalize`: CreateSchema (in `name`/`schema`/`domain`, out `schemaArn`, errors InvalidInput/LimitExceeded/ResourceAlreadyExists), CreateDatasetGroup (out `datasetGroupArn`+`domain`), DescribeDatasetGroup (out `datasetGroup`, errors InvalidInput/ResourceNotFound), CreateDataset (required `name`/`schemaArn`/`datasetGroupArn`/`datasetType`), DescribeDataset (out `dataset`), ListDatasets and ListDatasetGroups (`@paginated(inputToken=nextToken, outputToken=nextToken, items=datasets|datasetGroups)`, `maxResults` range 1..100, error `InvalidNextTokenException`), DeleteDataset/DeleteDatasetGroup (`@idempotent`, no output members, errors InvalidInput/ResourceInUse/ResourceNotFound). Shape members cited from the Smithy model: `DatasetGroup{name,datasetGroupArn,status,roleArn,kmsKeyArn,creationDateTime,lastUpdatedDateTime,failureReason,domain}`, `Dataset{name,datasetArn,datasetGroupArn,datasetType,schemaArn,status,creationDateTime,lastUpdatedDateTime,latestDatasetUpdate,trackingId}`, `DatasetGroupSummary`, `DatasetSummary`. Handler `src/ops/create_dataset_group.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 191 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateDatasetGroup"))`; all 71 files in `src/ops/` match that pattern. The model doc for CreateDatasetGroup states the status progression `CREATE PENDING > CREATE IN_PROGRESS > ACTIVE` and that DescribeDatasetGroup reports it, and that a group holds at most one dataset per type.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Personalize client at it.
2. CreateSchema name=my-interactions schema='{"type":"record","name":"Interactions","fields":[...]}' -> expect 200 with a `schemaArn`; currently 501 NotImplemented.
3. CreateDatasetGroup name=my-dsg -> expect 200 with `datasetGroupArn`.
4. DescribeDatasetGroup datasetGroupArn=<arn> -> expect `datasetGroup.name == my-dsg`, `status == ACTIVE`, timestamps populated.
5. CreateDataset name=my-interactions datasetType=Interactions schemaArn=<schemaArn> datasetGroupArn=<arn> -> expect 200 with `datasetArn`.
6. DescribeDataset datasetArn=<arn> -> expect the schemaArn/datasetGroupArn/datasetType exactly as created.
7. ListDatasets datasetGroupArn=<arn> maxResults=1 -> expect the dataset in `datasets` and a usable `nextToken` only when more remain; an arbitrary bogus nextToken must give InvalidNextTokenException (400).
8. DeleteDatasetGroup while the dataset still exists -> expect ResourceInUseException (409).
9. DeleteDataset then DeleteDatasetGroup -> expect 200 with empty bodies.
10. DescribeDatasetGroup on the deleted ARN -> expect ResourceNotFoundException (404), and ListDatasetGroups must no longer include it.
