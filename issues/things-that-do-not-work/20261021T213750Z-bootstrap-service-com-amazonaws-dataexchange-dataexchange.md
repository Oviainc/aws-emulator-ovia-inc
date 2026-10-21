Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the dataexchange emulator

_Filed: 2026-10-21T21:37:50Z_
_Operation: service-com-amazonaws-dataexchange-dataexchange | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 37 operations in `services/service-com-amazonaws-dataexchange-dataexchange/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns HTTP 501 `NotImplemented`, and the crate holds no state. Nothing works end-to-end.

Implement a FIRST SLICE of six operations covering the lifecycle of the service's primary resource, the data set (and its revisions). Wire protocol is `restJson1`, sigv4 signing name `dataexchange`.

1. **CreateDataSet** — `POST /v1/data-sets -> 201`. Input: `AssetType` (required enum: API_GATEWAY_API | LAKE_FORMATION_DATA_PERMISSION | REDSHIFT_DATA_SHARE | S3_DATA_ACCESS | S3_SNAPSHOT), `Description` (required), `Name` (required), `Tags`. Must mint a new `Id` matching `^[a-zA-Z0-9]{30,40}$`, derive `Arn` (`arn:aws:dataexchange:<region>:<account>:data-sets/<Id>`), set `Origin` = `OWNED`, set `CreatedAt`/`UpdatedAt` to now, persist the record, and return the persisted values (not an echo of the request alone).
2. **GetDataSet** — `GET /v1/data-sets/{DataSetId} -> 200`. Must look the id up in state and return the stored `Arn`, `AssetType`, `CreatedAt`, `Description`, `Id`, `Name`, `Origin`, `Tags`, `UpdatedAt`. Unknown id -> `ResourceNotFoundException` (HTTP 404); id not matching the `^[a-zA-Z0-9]{30,40}$` pattern -> `ValidationException` (HTTP 400).
3. **UpdateDataSet** — `PATCH /v1/data-sets/{DataSetId} -> 200`. Optional `Name`, `Description`. Must mutate the stored record in place (only the supplied members), bump `UpdatedAt`, and return the merged record so a following GetDataSet shows the new values. Unknown id -> 404.
4. **ListDataSets** — `GET /v1/data-sets -> 200`, `@paginated(inputToken=NextToken, outputToken=NextToken, items=DataSets)`. Query params `maxResults` (1..200), `nextToken`, `origin`. Must enumerate the data sets actually in state as `DataSetEntry` items, honour `maxResults` as a page bound, return an opaque `NextToken` only when more remain and resume correctly from it, and filter by `Origin` when `origin` is given.
5. **CreateRevision** — `POST /v1/data-sets/{DataSetId}/revisions -> 201`. Input: `Comment`, `DataSetId` (@httpLabel, required), `Tags`. Must reject an unknown `DataSetId` with `ResourceNotFoundException`, mint a revision `Id`/`Arn`, set `Finalized=false`, `Revoked=false`, `CreatedAt`/`UpdatedAt`, and attach the revision to that data set in state.
6. **ListDataSetRevisions** — `GET /v1/data-sets/{DataSetId}/revisions -> 200`, paginated the same way (`items=Revisions`). Must return the revisions created in step 5 as `RevisionEntry` items, newest first, bounded by `maxResults` with a working `NextToken`; unknown `DataSetId` -> 404.
7. **DeleteDataSet** — `DELETE /v1/data-sets/{DataSetId} -> 204` (no body). Must remove the data set and its revisions from state; a subsequent GetDataSet must return `ResourceNotFoundException` (404) and ListDataSets must no longer include it. Deleting an already-deleted id -> 404.

Every response must be computed from persisted state. No canned bodies, no echoing the input back without storing it, no random id returned without a stored record behind it — a create followed by a get/list must return exactly what was created.

## State Model

In `src/state.rs`, on `ComAmazonawsDataexchangeDataexchangeState`:

- `data_sets: HashMap<String /* DataSetId */, DataSet>` where
  `DataSet { id: String, arn: String, name: String, description: String, asset_type: AssetType, origin: Origin /* OWNED */, origin_details: Option<OriginDetails>, source_id: Option<String>, tags: HashMap<String,String>, created_at: DateTime, updated_at: DateTime, revisions: HashMap<String /* RevisionId */, Revision>, revision_order: Vec<String> }`
- `Revision { id: String, arn: String, data_set_id: String, comment: Option<String>, finalized: bool /* false at create */, revoked: bool /* false */, revocation_comment: Option<String>, revoked_at: Option<DateTime>, source_id: Option<String>, tags: HashMap<String,String>, created_at: DateTime, updated_at: DateTime, assets: HashMap<String, Asset> /* empty in this slice, reserved for assets/jobs later */ }`
- `data_set_order: Vec<String>` — creation order, so ListDataSets pagination is stable.
- Pagination: `NextToken` is an opaque cursor (e.g. base64 of the index/last-seen id) resolved against `data_set_order` / `revision_order`; an unparsable token -> `ValidationException`.
- Id minting: a helper producing 32-char `[a-zA-Z0-9]` ids so every returned id satisfies the modeled pattern, plus an arn builder using the request's region/account.

The remaining 30 operations (data grants, jobs, assets, event actions, SendApiAsset, tagging, notifications) stay 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-dataexchange-dataexchange/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-dataexchange-dataexchange.json`) lists all 37 operations with routes, members and errors: CreateDataSet (line 47, `POST /v1/data-sets -> 201`, required AssetType/Description/Name, output Arn/Id/Origin/CreatedAt/UpdatedAt), GetDataSet (line 126, @readonly, 404 ResourceNotFoundException), UpdateDataSet (line 280, PATCH, optional Name/Description), ListDataSets (line 182, @paginated inputToken=NextToken items=DataSets, maxResults 1..200, origin query param), CreateRevision (line 68, `POST /v1/data-sets/{DataSetId}/revisions -> 201`, Finalized default false, Revoked default false), ListDataSetRevisions (line 174, @paginated items=Revisions), DeleteDataSet (line 89, `DELETE /v1/data-sets/{DataSetId} -> 204`). All handler files under `src/ops/` match `^//! SCAFFOLDED, NOT IMPLEMENTED:` (38 files globbed, 37 operations + mod.rs), i.e. every operation answers HTTP 501 today; the crate README documents that marker as the stub census. No tests exist beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI dataexchange client at it.
2. `aws dataexchange create-data-set --asset-type S3_SNAPSHOT --name demo --description 'demo set'` — expect HTTP 201 with a 30-40 char alphanumeric Id and an Arn; today HTTP 501 NotImplemented.
3. `aws dataexchange get-data-set --data-set-id <Id>` — expect 200 with Name=demo, Description='demo set', AssetType=S3_SNAPSHOT, Origin=OWNED, the same Id/Arn/CreatedAt.
4. `aws dataexchange update-data-set --data-set-id <Id> --name demo2` then get-data-set again — expect Name=demo2 and a bumped UpdatedAt.
5. `aws dataexchange list-data-sets --max-results 1` with a second data set created — expect one entry plus a NextToken; pass the token back and expect the other entry and no token.
6. `aws dataexchange create-revision --data-set-id <Id> --comment first` — expect 201, Finalized=false, Revoked=false, a revision Id/Arn.
7. `aws dataexchange list-data-set-revisions --data-set-id <Id>` — expect the revision just created to appear with the same Id and Comment.
8. `aws dataexchange create-revision --data-set-id 00000000000000000000000000000000` — expect ResourceNotFoundException (HTTP 404).
9. `aws dataexchange delete-data-set --data-set-id <Id>` — expect HTTP 204 with an empty body; then get-data-set the same id and expect ResourceNotFoundException (404), and list-data-sets must not include it.
