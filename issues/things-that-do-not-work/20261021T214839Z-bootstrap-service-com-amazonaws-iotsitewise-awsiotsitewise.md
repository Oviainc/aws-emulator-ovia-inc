Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the IoTSiteWise emulator

_Filed: 2026-10-21T21:48:39Z_
_Operation: service-com-amazonaws-iotsitewise-awsiotsitewise | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 149 operations in `services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented`, and the service holds no state. Implement a FIRST, SMALL happy-path slice of the primary resource pair (asset model -> asset -> property values) statefully, so a realistic client script works end-to-end. Wire protocol is `restJson1`, signing name `iotsitewise`.

### Happy path to implement (in order)

1. **CreateAssetModel** — `POST /asset-models` -> 202. Input: `assetModelName` (required), optional `assetModelType` (ASSET_MODEL | COMPONENT_MODEL | INTERFACE, default ASSET_MODEL), `assetModelDescription`, `assetModelProperties` (list<AssetModelPropertyDefinition>), `assetModelHierarchies`, `assetModelExternalId`, `tags`, `clientToken`. Must mint a fresh UUIDv4 `assetModelId` (or honor a caller-supplied `assetModelId`), build `assetModelArn` = `arn:aws:iotsitewise:<region>:<account>:asset-model/<assetModelId>`, assign each property definition a generated `id`, and store the record. Return `assetModelId`, `assetModelArn`, `assetModelStatus` (`{state: ACTIVE}` is acceptable for the emulator — no async CREATING dance required, but the state returned must match what DescribeAssetModel then reports). Second create with an already-used `assetModelId` -> `ResourceAlreadyExistsException` (409).

2. **DescribeAssetModel** — `GET /asset-models/{assetModelId}` -> 200. Must look the model up in state (accepting both the raw UUID and the `externalId:<id>` form) and return the STORED `assetModelName`, `assetModelArn`, `assetModelDescription`, `assetModelType`, `assetModelProperties` (with the ids minted at create, incl. each property's `name`, `dataType`, `unit`, `type`), `assetModelHierarchies`, `assetModelCreationDate`, `assetModelLastUpdateDate`, `assetModelStatus`. Unknown id -> `ResourceNotFoundException` (404).

3. **CreateAsset** — `POST /assets` -> 202. Input: `assetName` (required), `assetModelId` (required), optional `assetId`, `assetExternalId`, `assetDescription`, `tags`, `clientToken`. Must VALIDATE that `assetModelId` resolves to a stored asset model (else `ResourceNotFoundException` 404), mint `assetId` + `assetArn` (`arn:aws:iotsitewise:<region>:<account>:asset/<assetId>`), and materialize one `AssetProperty` per property of the referenced model (property id inherited from the model property) so DescribeAsset and property writes can find them. Return `assetId`, `assetArn`, `assetStatus`.

4. **DescribeAsset** — `GET /assets/{assetId}?excludeProperties=` -> 200. Must return the stored `assetId`, `assetArn`, `assetName`, `assetModelId`, `assetProperties` (from the model-derived property list; omitted/empty when `excludeProperties=true`), `assetHierarchies`, `assetCreationDate`, `assetLastUpdateDate`, `assetStatus`, `assetDescription`. Unknown id -> `ResourceNotFoundException` (404).

5. **BatchPutAssetPropertyValue** — `POST /properties` -> 200. Input: `entries` (list<PutAssetPropertyValueEntry>: `entryId`, `assetId`, `propertyId`/`propertyAlias`, `propertyValues` = list of `AssetPropertyValue{value: Variant, timestamp: TimeInNanos, quality}`). Must append each accepted value to the per-(assetId, propertyId) time-series in state, keyed and sorted by `timestamp.timeInSeconds`/`offsetInNanos`, and return `errorEntries` — empty on full success, and one entry (`entryId`, `errors` with `errorCode` e.g. `ResourceNotFoundException`/`InvalidRequestException` and `errorMessage`) per entry whose asset or property is unknown. The response must be computed from the lookup result, not a hardcoded empty list.

6. **GetAssetPropertyValue** — `GET /properties/latest?assetId=&propertyId=` -> 200. Must return `propertyValue` = the LATEST stored `AssetPropertyValue` (highest timestamp) for that asset/property, with the exact variant member (`doubleValue`/`integerValue`/`stringValue`/`booleanValue`) and timestamp that was written in step 5. No stored values -> the modeled empty/absent `propertyValue`; unknown asset or property -> `ResourceNotFoundException` (404).

7. **ListAssets** — `GET /assets?assetModelId=&filter=&maxResults=&nextToken=` -> 200. Must enumerate assets from state as `assetSummaries` (`id`, `arn`, `name`, `assetModelId`, `creationDate`, `lastUpdateDate`, `status`, `hierarchies`), filtering by `assetModelId` when given, honoring `maxResults` (1..250) and returning an opaque `nextToken` that resumes the enumeration.

8. **DeleteAsset** / **DeleteAssetModel** — `DELETE /assets/{assetId}` and `DELETE /asset-models/{assetModelId}` -> 202. Must remove the record from state (DeleteAssetModel must reject with `ConflictingOperationException`/`InvalidRequestException` while assets still reference the model) and return `assetId`/`assetModelId` + status with state `DELETING`/removed. A subsequent DescribeAsset / DescribeAssetModel / ListAssets must no longer see it (404 / absent).

### Requirements

- NEVER STUBS: each of these operations must compute its response entirely from persisted state in the shared engine store (`crate::state::service_state(&*self.engine)`). Hardcoded ids, echoed inputs, or a minted id that is never stored are failures. A create followed by a describe must return exactly what was created.
- Ids must match the modeled patterns: `assetId`/`assetModelId` are 36-char UUIDs and must not be the all-zero UUID; ARNs must match `^arn:aws(-cn|-us-gov)?:[a-zA-Z0-9-:\/_\.]+$`.
- `@httpLabel` id members accept the `externalId:<externalId>` alias form (len 13..139 pattern) — resolve it through the stored `assetExternalId`/`assetModelExternalId`.
- Errors must use the modeled shapes/status codes: `ResourceNotFoundException` 404, `InvalidRequestException` 400, `ResourceAlreadyExistsException` 409, `ConflictingOperationException` 409.
- Done = an integration test that runs CreateAssetModel -> DescribeAssetModel -> CreateAsset -> DescribeAsset -> BatchPutAssetPropertyValue -> GetAssetPropertyValue -> ListAssets -> DeleteAsset -> DeleteAssetModel passes with correct bodies, not just 2xx.

## State Model

In `src/state.rs` (`ComAmazonawsIotsitewiseAwsiotsitewiseState`), under the single shared engine:

- `asset_models: HashMap<String /*assetModelId*/, AssetModelRecord>` where `AssetModelRecord { id, external_id: Option<String>, arn, name, description: Option<String>, model_type: AssetModelType, properties: Vec<ModelProperty>, hierarchies: Vec<ModelHierarchy>, composite_models: Vec<...>, creation_date: SystemTime, last_update_date: SystemTime, status: AssetModelStatus, version: u32, tags: HashMap<String,String> }`
- `ModelProperty { id: String /*minted uuid*/, external_id: Option<String>, name, data_type, data_type_spec: Option<String>, unit: Option<String>, property_type: PropertyType }`
- `ModelHierarchy { id: String, external_id: Option<String>, name, child_asset_model_id }`
- `assets: HashMap<String /*assetId*/, AssetRecord>` where `AssetRecord { id, external_id: Option<String>, arn, name, asset_model_id, description: Option<String>, properties: Vec<AssetPropertyRecord>, hierarchies: Vec<AssetHierarchy>, creation_date, last_update_date, status: AssetStatus, tags }`
- `AssetPropertyRecord { id /*= model property id*/, external_id, name, alias: Option<String>, data_type, unit, notification: Option<PropertyNotification> }`
- `property_values: HashMap<(String /*assetId*/, String /*propertyId*/), Vec<AssetPropertyValue>>` kept sorted ascending by timestamp (serves GetAssetPropertyValue = last element, and later GetAssetPropertyValueHistory)
- `property_alias_index: HashMap<String /*propertyAlias*/, (String assetId, String propertyId)>` so `propertyAlias` lookups resolve
- `external_id_index: HashMap<String /*externalId*/, String /*id*/>` for assets and for asset models, so `externalId:` labels resolve

All other operations (portals, projects, dashboards, gateways, datasets, computation models, queries, bulk import, interfaces, tagging, etc.) stay 501 for now and will be filed as individual gap issues.

## Evidence

1) `services/service-com-amazonaws-iotsitewise-awsiotsitewise/OPERATIONS.md` — 149 operations, protocol `restJson1`, SDK id `IoTSiteWise`. It gives the exact contracts cited above: CreateAssetModel (lines 151-156, `POST /asset-models` -> 202, required output `assetModelId`/`assetModelArn`/`assetModelStatus`), CreateAsset (144-149, required output `assetId`/`assetArn`/`assetStatus`), DescribeAsset (390-395), DescribeAssetModel (404-409), BatchPutAssetPropertyValue (99-104, required output `errorEntries`), GetAssetPropertyValue (631-636, output `propertyValue`), ListAssets (749-755, `@paginated(inputToken=nextToken, outputToken=nextToken, items=assetSummaries)`), DeleteAsset (267-272), DeleteAssetModel (274-279).
2) `src/ops/create_asset.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`` — the same marker every file in `src/ops/` carries, so no operation holds or reads state today.
3) README.md documents the stub contract (501 until written, marker line deleted on implementation) and that handlers must reach the ONE shared engine via `crate::state::service_state(&*self.engine)`.
4) Well-established IoT SiteWise semantics: an asset cannot be created without an existing asset model, an asset's properties are derived from its model's property definitions, and the latest value returned by GetAssetPropertyValue is the highest-timestamp value previously written by BatchPutAssetPropertyValue.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with endpoint prefix `iotsitewise`.
2. `aws iotsitewise create-asset-model --asset-model-name Boiler --asset-model-properties '[{"name":"Temperature","dataType":"DOUBLE","unit":"Celsius","type":{"measurement":{}}}]'` — expect 202 with a UUID `assetModelId`, an `assetModelArn` and an `assetModelStatus`; today: HTTP 501 NotImplemented.
3. `aws iotsitewise describe-asset-model --asset-model-id <id>` — expect the stored name `Boiler` and the `Temperature` property with the id minted at create.
4. `aws iotsitewise create-asset --asset-name Boiler1 --asset-model-id <id>` — expect 202 with `assetId`/`assetArn`/`assetStatus`; an unknown `--asset-model-id` must return ResourceNotFoundException (404).
5. `aws iotsitewise describe-asset --asset-id <assetId>` — expect name `Boiler1`, the `assetModelId` used, and an `assetProperties` entry for `Temperature`.
6. `aws iotsitewise batch-put-asset-property-value --entries '[{"entryId":"e1","assetId":"<assetId>","propertyId":"<propId>","propertyValues":[{"value":{"doubleValue":42.5},"timestamp":{"timeInSeconds":1700000000},"quality":"GOOD"}]}]'` — expect 200 with `errorEntries: []`.
7. `aws iotsitewise get-asset-property-value --asset-id <assetId> --property-id <propId>` — expect `propertyValue.value.doubleValue == 42.5` with timestamp 1700000000 (read back from state, not canned).
8. `aws iotsitewise list-assets --asset-model-id <id>` — expect the one asset in `assetSummaries`; with `--max-results 1` and two assets, expect a `nextToken` that pages to the second.
9. `aws iotsitewise delete-asset --asset-id <assetId>` then `describe-asset --asset-id <assetId>` — expect 202 then ResourceNotFoundException (404), and the asset absent from list-assets.
10. `aws iotsitewise delete-asset-model --asset-model-id <id>` then `describe-asset-model` — expect 202 then 404; deleting while an asset still references the model must fail with a 409 conflict.
