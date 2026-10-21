Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the AWS Marketplace Catalog emulator

_Filed: 2026-10-21T22:08:24Z_
_Operation: service-com-amazonaws-marketplacecatalog-awsmpseymour | Class: missing_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 15 operations in `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. This issue defines the FIRST minimal, stateful happy-path slice: the change-set lifecycle of the Marketplace Catalog (the service's primary resource), plus the entity reads that a change set produces. Operations outside this slice (BatchDescribeEntities, Put/Get/DeleteResourcePolicy, Tag/Untag/ListTagsForResource, List/DescribeAssessment) stay stubbed and will be filed as individual gap issues later.

Protocol: `restJson1`, sigv4 signing name `aws-marketplace`, endpoint prefix `catalog.marketplace` (per README/OPERATIONS.md wire-protocol section).

### Happy path to implement (in order)

1. **StartChangeSet** — `POST /StartChangeSet`. Input: `Catalog` (required, e.g. `AWSMarketplace`), `ChangeSet` (required, 1..20 `Change`), optional `ChangeSetName`, `ClientRequestToken` (@idempotencyToken), `ChangeSetTags`, `Intent` (`APPLY`|`VALIDATE`). Each `Change` carries required `ChangeType` and required `Entity{Type (required), Identifier}`, plus `Details`/`DetailsDocument`, `EntityTags`, `ChangeName`. Must MINT and persist a new change set: generate `ChangeSetId` matching `^[\w\-]+$` and `ChangeSetArn` matching `^[a-zA-Z0-9:*/-]+$` (`arn:aws:aws-marketplace:<region>:<account>:AWSMarketplace/ChangeSet/<id>`), store the full submitted `ChangeSet` list, `StartTime`, status, and the resolved entity ids. Return only `ChangeSetId` + `ChangeSetArn` — both read back from the stored record. Same `ClientRequestToken` + same catalog must return the SAME `ChangeSetId` instead of creating a second change set.
2. **DescribeChangeSet** — `GET /DescribeChangeSet?catalog=&changeSetId=`. Must return the stored record: `ChangeSetId`, `ChangeSetArn`, `ChangeSetName`, `Intent`, `StartTime`, `EndTime`, `Status` (`PREPARING`|`APPLYING`|`SUCCEEDED`|`FAILED`|`CANCELLED`), `ChangeSet` as a list of `ChangeSummary{ChangeType, Entity, Details, DetailsDocument, ChangeName, ErrorDetailList}` echoing what was submitted, and `FailureCode`/`FailureDescription` only when `Status == FAILED`. An unknown `ChangeSetId` must be `ResourceNotFoundException` (HTTP 404), not 501 or an empty 200.
3. **ListChangeSets** — `POST /ListChangeSets`. Input: `Catalog` (required), `FilterList` (1..8 `Filter`), `Sort`, `MaxResults` (1..20), `NextToken`. Must return `ChangeSetSummaryListItem{ChangeSetId, ChangeSetArn, ChangeSetName, StartTime, EndTime, Status, EntityIdList, FailureCode}` for every change set actually created in that catalog, honour `MaxResults` by truncating and emitting an opaque `NextToken` (pattern `^[\w+=.:@\-\/]+$`) that resumes exactly where the page ended, and return no `NextToken` on the last page.
4. **DescribeEntity** — `GET /DescribeEntity?catalog=&entityId=`. After a change set reaches `SUCCEEDED`, the entity it created/updated must be readable here: `EntityType`, `EntityIdentifier`, `EntityArn`, `LastModifiedDate` (ISO-8601 `....-..-..T..:..:..Z`), and `Details`/`DetailsDocument` derived from the applied change's details. Unknown entity id → `ResourceNotFoundException` (404).
5. **ListEntities** — `POST /ListEntities`. Input: `Catalog` (required), `EntityType` (required), optional `FilterList`, `Sort`, `MaxResults` (1..50), `NextToken`, `OwnershipType`. Must return `EntitySummary{Name, EntityType, EntityId, EntityArn, LastModifiedDate, Visibility}` for the stored entities whose type matches `EntityType` — filtered from state, never a canned list — with the same `MaxResults`/`NextToken` paging contract as ListChangeSets.
6. **CancelChangeSet** — `PATCH /CancelChangeSet?catalog=&changeSetId=`. Must flip a change set still in `PREPARING`/`APPLYING` to `CANCELLED`, set `EndTime`, and return its `ChangeSetId`/`ChangeSetArn`. Cancelling a change set that already reached `SUCCEEDED`/`FAILED` must raise `ResourceInUseException` (HTTP 423); unknown id → `ResourceNotFoundException` (404). A subsequent DescribeChangeSet must report `CANCELLED`.

All six must compute their responses from persisted state. No canned bodies, no echoing the request back, no generating a `ChangeSetId` that is never stored. Required-member absence and the modeled length/pattern/range constraints (`Catalog` `^[a-zA-Z]+$` len 1..64, `ChangeSetId` `^[\w\-]+$`, `ChangeSet` len 1..20, `MaxResults` ranges) must produce `ValidationException` (HTTP 422).

## State Model

One per-account/per-region store, keyed by catalog name (`AWSMarketplace`):

- `catalogs: Map<String /*catalog*/, Catalog>`
- `Catalog { change_sets: Map<String /*change_set_id*/, ChangeSetRecord>, entities: Map<String /*entity_id*/, EntityRecord>, idempotency: Map<String /*ClientRequestToken*/, String /*change_set_id*/> }`
- `ChangeSetRecord { id: String, arn: String, name: Option<String>, intent: Intent, status: ChangeStatus, start_time: String /*ISO-8601 Z*/, end_time: Option<String>, failure_code: Option<FailureCode>, failure_description: Option<String>, changes: Vec<ChangeRecord>, entity_ids: Vec<String>, tags: Vec<Tag> }`
- `ChangeRecord { change_type: String, entity_type: String, entity_identifier: Option<String>, details: Option<String>, details_document: Option<Json>, change_name: Option<String>, entity_tags: Vec<Tag>, error_details: Vec<ErrorDetail> }`
- `EntityRecord { id: String, entity_type: String, arn: String, name: Option<String>, visibility: String /*Public|Limited|Restricted*/, last_modified_date: String, details: Option<String>, details_document: Option<Json>, tags: Vec<Tag> }`

Applying a change set with `Intent=APPLY` inserts/updates the `EntityRecord` for each change (minting an `entity_id` when `Entity.Identifier` is absent, otherwise mutating the existing record and bumping `last_modified_date`), records the ids in `ChangeSetRecord.entity_ids`, then sets `status=SUCCEEDED` and `end_time`. `Intent=VALIDATE` must NOT mutate `entities`. Pagination tokens encode an offset/last-key into the deterministic iteration order of `change_sets` / `entities`.

Done = an integration script can StartChangeSet, see the change set in ListChangeSets and DescribeChangeSet with the submitted changes intact, read the resulting entity via DescribeEntity and find it in ListEntities, and cancel an in-flight change set — all with correct bodies, not just 2xx.

## Evidence

All 15 handler files under `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501 (confirmed by grep across the ops dir: batch_describe_entities, cancel_change_set, delete_resource_policy, describe_assessment, describe_change_set, describe_entity, get_resource_policy, list_assessments, list_change_sets, list_entities, list_tags_for_resource, put_resource_policy, start_change_set, tag_resource, untag_resource). Routes, required members, constraints and error/status mappings above are taken from the crate's generated `OPERATIONS.md` (StartChangeSet/DescribeChangeSet/ListChangeSets/DescribeEntity/ListEntities/CancelChangeSet sections, incl. `ValidationException` 422, `ResourceNotFoundException` 404, `ResourceInUseException` 423 and the `@paginated(inputToken=NextToken, outputToken=NextToken)` traits), and the `Change`/`ChangeSummary`/`Entity`/`EntitySummary`/`ChangeSetSummaryListItem` member lists come from the Smithy model via smithy_query. Change-set semantics (a change set is asynchronous, carries a status, and its applied changes become visible as catalog entities) are the documented behavior of AWS Marketplace Catalog.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace, ChangeSetName="bootstrap-test", ClientRequestToken="tok-1", Intent=APPLY and ChangeSet=[{ChangeType:"CreateProduct", Entity:{Type:"AmiProduct@1.0"}, Details:"{}"}] — expect 200 with a fresh ChangeSetId and matching ChangeSetArn (today: 501).
2. Repeat the identical StartChangeSet call with ClientRequestToken="tok-1" — expect the SAME ChangeSetId, and exactly one change set in state.
3. GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> — expect 200 echoing ChangeSetName="bootstrap-test", Intent=APPLY, a StartTime, a Status, and a ChangeSet list containing the submitted ChangeType/Entity/Details.
4. POST /ListChangeSets with Catalog=AWSMarketplace, MaxResults=1 — expect the change set as a ChangeSetSummaryListItem including EntityIdList, plus a NextToken when more exist; follow the NextToken and expect the next page with no repeats.
5. GET /DescribeEntity?catalog=AWSMarketplace&entityId=<id from EntityIdList> — expect 200 with EntityType, EntityIdentifier, EntityArn, LastModifiedDate and Details.
6. POST /ListEntities with Catalog=AWSMarketplace, EntityType="AmiProduct" — expect the created entity in EntitySummaryList; with EntityType="SaaSProduct" expect an empty list (state-filtered, not canned).
7. PATCH /CancelChangeSet?catalog=AWSMarketplace&changeSetId=<id of a PREPARING change set> — expect 200 with ChangeSetId/ChangeSetArn, then DescribeChangeSet reports Status=CANCELLED with an EndTime; cancelling it again expects ResourceInUseException (423).
8. GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=cs-does-not-exist — expect ResourceNotFoundException (404).
9. POST /StartChangeSet with an empty ChangeSet list or a Catalog of "AWS_Marketplace" — expect ValidationException (422).
