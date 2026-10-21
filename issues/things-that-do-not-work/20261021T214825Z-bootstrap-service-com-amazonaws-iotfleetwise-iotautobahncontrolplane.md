Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the IoTFleetWise (IoTAutobahnControlPlane) emulator

_Filed: 2026-10-21T21:48:25Z_
_Operation: service-com-amazonaws-iotfleetwise-iotautobahncontrolplane | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate is a fresh scaffold: all 57 operations under `services/service-com-amazonaws-iotfleetwise-iotautobahncontrolplane/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answer HTTP 501, and `src/state.rs` declares `ComAmazonawsIotfleetwiseIotautobahncontrolplaneState` with no fields (`// TODO: add service-specific state fields`). Nothing is stored, so no client workflow works.

This issue scopes the FIRST, SMALL slice: the signal-catalog -> model-manifest -> vehicle core loop, implemented statefully so a realistic client script runs end to end. Wire protocol is `awsJson1_0`, `POST /` with `X-Amz-Target: IoTAutobahnControlPlane.<OperationName>` (OPERATIONS.md "Wire protocol").

Happy path to implement, in order:

1. **CreateSignalCatalog** — input `name` (required, 1..100, `^[a-zA-Z\d\-_:]+$`), `description`, `nodes` (list<Node>, 0..500), `tags`. Insert a new catalog into state; output `name` + `arn` where the arn is generated as `arn:aws:iotfleetwise:<region>:<account>:signal-catalog/<name>` and is the SAME arn later reads return. Re-creating an existing name -> `ConflictException` (409).
2. **CreateModelManifest** — input `name` (required), `nodes` (list<string>, required), `signalCatalogArn` (required), `description`, `tags`. Must look up `signalCatalogArn` in state and return `ResourceNotFoundException` (404) when no catalog with that arn was created; store the node list and the owning catalog arn. Output `name` + generated `arn` (`.../model-manifest/<name>`). Duplicate name -> `ConflictException`.
3. **CreateVehicle** — input `vehicleName` (required), `modelManifestArn` (required), `decoderManifestArn` (required), `attributes` (map<string,string>), `associationBehavior`, `stateTemplates`, `tags`. Must resolve `modelManifestArn` against stored model manifests (404 `ResourceNotFoundException` if absent); store `attributes` verbatim, record `creationTime`/`lastModificationTime`. Output `vehicleName`, `arn` (`.../vehicle/<name>`), `thingArn`. Duplicate vehicleName -> `ConflictException` (409). For this first slice `decoderManifestArn` is stored as given (decoder-manifest lifecycle ops stay stubbed and are filed separately).
4. **GetVehicle** — input `vehicleName`. Must read the stored vehicle and return `vehicleName`, `arn`, `modelManifestArn`, `decoderManifestArn`, `attributes` (exactly what CreateVehicle stored, not an echo of this request), `stateTemplates`, `creationTime`, `lastModificationTime`. Unknown name -> `ResourceNotFoundException` (404).
5. **ListVehicles** — `@paginated(inputToken=nextToken, outputToken=nextToken, items=vehicleSummaries)`. Must enumerate vehicles from state into `vehicleSummaries`, honour `maxResults` (1..100) by truncating and returning an opaque `nextToken` that resumes exactly where the previous page stopped (omit `nextToken` on the last page), and filter by `modelManifestArn` / `attributeNames`+`attributeValues` when supplied. An empty store returns an empty list, never a canned entry.
6. **DeleteVehicle** — input `vehicleName`; remove the vehicle from state and return the required `vehicleName` + `arn` of what was removed. It is `@idempotent`: deleting an already-absent vehicle succeeds. After it, GetVehicle must 404 and ListVehicles must not include the vehicle.

No canned, hardcoded or echoed responses: every one of the six must compute its response from the persisted state, and ARNs/timestamps minted at create time must be the ones returned by subsequent reads. A random id that is never stored, or a `GetVehicle` that reflects its own input, counts as a failure of this issue.

## State Model

Add to `ComAmazonawsIotfleetwiseIotautobahncontrolplaneState` (all behind the existing engine lock):

- `signal_catalogs: HashMap<String /*name*/, SignalCatalog { name, arn, description: Option<String>, nodes: Vec<Node>, node_counts: NodeCounts, tags: Vec<Tag>, creation_time, last_modification_time }>`
- `model_manifests: HashMap<String /*name*/, ModelManifest { name, arn, description: Option<String>, signal_catalog_arn: String, nodes: Vec<String>, status: ManifestStatus (DRAFT on create), tags, creation_time, last_modification_time }>`
- `vehicles: HashMap<String /*vehicleName*/, Vehicle { vehicle_name, arn, thing_arn, model_manifest_arn, decoder_manifest_arn, attributes: BTreeMap<String,String>, state_templates: Vec<StateTemplateAssociation>, tags, creation_time, last_modification_time }>`
- insertion-ordered key vectors (or a `BTreeMap`) so ListVehicles/ListSignalCatalogs pagination is stable, plus a helper that maps an opaque `nextToken` to an offset/last-key.
- arn helpers that build `arn:aws:iotfleetwise:<region>:<account>:<type>/<name>` from the request context, and reverse lookups arn -> resource used by the CreateModelManifest / CreateVehicle existence checks.

Out of scope for this slice (stay 501, filed individually later): campaigns, decoder manifests, state templates, fleets and vehicle-fleet association, batch vehicle ops, tagging, RegisterAccount, logging/encryption configuration, GetVehicleStatus, and the Update*/Import*/List*Nodes families.

## Evidence

OPERATIONS.md for this crate: "57 operations", "Protocol: awsJson1_0 ... X-Amz-Target: IoTAutobahnControlPlane.<OperationName>"; CreateSignalCatalog (line 315), CreateModelManifest (line 267), CreateVehicle (line 411), GetVehicle (line 419), ListVehicles (line 442, `@paginated(items=vehicleSummaries)`), DeleteVehicle (line 434, `@idempotent`) give the exact required/optional members, outputs and the ConflictException/ResourceNotFoundException/ValidationException error sets cited above. `grep -r '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches every handler file in the crate, and `src/state.rs` lines 6-9 show an empty state struct, so no operation can be stateful today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK IoTFleetWise client at it.
2. CreateSignalCatalog(name="demo-catalog", nodes=[one branch + one sensor]) -> expect 200 with name and a generated arn; today 501 NotImplemented.
3. CreateModelManifest(name="demo-model", signalCatalogArn=<arn from step 2>, nodes=[the sensor's fully-qualified name]) -> expect 200 with name + arn; passing a bogus signalCatalogArn must give ResourceNotFoundException (404).
4. CreateVehicle(vehicleName="vin-123", modelManifestArn=<arn from step 3>, decoderManifestArn="arn:aws:iotfleetwise:us-east-1:123456789012:decoder-manifest/demo-decoder", attributes={"color":"red"}) -> expect 200/201 with vehicleName, arn, thingArn.
5. Repeat step 4 verbatim -> expect ConflictException (409), not a second create.
6. GetVehicle(vehicleName="vin-123") -> expect modelManifestArn, decoderManifestArn and attributes={"color":"red"} exactly as created, plus creationTime/lastModificationTime.
7. ListVehicles(maxResults=1) after creating a second vehicle -> expect one summary plus a nextToken; ListVehicles(nextToken=...) returns the other vehicle and no nextToken.
8. DeleteVehicle(vehicleName="vin-123") -> expect 200/204 with the deleted vehicleName + arn.
9. GetVehicle(vehicleName="vin-123") -> expect ResourceNotFoundException (404); ListVehicles no longer lists it; DeleteVehicle again still succeeds (idempotent).
