Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the AWS Control Tower (AWSControlTowerApis) emulator

_Filed: 2026-10-21T21:38:16Z_
_Operation: service-com-amazonaws-controltower-awscontroltowerapis | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 28 operations in `services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, holding no state. Nothing in this service works end to end.

This issue scopes the FIRST, SMALL slice: the landing zone lifecycle, which is Control Tower's primary resource. Implement these 5 operations statefully, in this order (all `restJson1`, POST, per OPERATIONS.md):

1. **CreateLandingZone** — `POST /create-landingzone`. Input: `version` (required, `^\d+.\d+$`, len 3..10), `manifest` (document), `remediationTypes` (list, len 1..1), `tags` (map, 0..200). Must mint and PERSIST a landing zone: generate `arn` (`arn:aws:controltower:<region>:<account>:landingzone/<ID>`, len 20..2048, pattern `^arn:aws[0-9a-zA-Z_\-:\/]+$`) and a lowercase-hex UUID `operationIdentifier` (len exactly 36, pattern `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`). Store the manifest, version, tags and remediationTypes verbatim; record a CREATE operation. AWS allows ONE landing zone per account, so a second CreateLandingZone while one exists must return the modeled `ValidationException` (400) (`ConflictException` 409 is also modeled and acceptable for a create that collides with an in-flight operation). Return both `arn` and `operationIdentifier` — both are `@required`.
2. **GetLandingZoneOperation** — `POST /get-landingzone-operation`. Input: `operationIdentifier` (required). Must look the operation up in persisted state and return `operationDetails` (`LandingZoneOperationDetail`, required) with `operationType` = CREATE/DELETE/UPDATE/RESET matching the call that produced it, `operationIdentifier` echoed from state, `status` (`SUCCEEDED` | `IN_PROGRESS` | `FAILED`), `startTime` and, once terminal, `endTime` (both ISO-8601 date-time). An unknown identifier must be `ResourceNotFoundException` (404), NOT a synthesized SUCCEEDED record.
3. **GetLandingZone** — `POST /get-landingzone`. Input: `landingZoneIdentifier` (required). Must return `landingZone` (`LandingZoneDetail`, required) built from stored state: `version` and `manifest` (both `@required` in the shape), `arn`, `status` (`ACTIVE` once the CREATE operation completed, `PROCESSING` while in flight), `latestAvailableVersion`, `remediationTypes`, optional `driftStatus`. The manifest returned must be byte-equal to the one supplied to CreateLandingZone. Unknown identifier -> `ResourceNotFoundException` (404).
4. **ListLandingZones** — `POST /list-landingzones`. Input: `nextToken`, `maxResults` (range 1..1 — reject out-of-range with `ValidationException` 400). Output: `landingZones` (required list of `LandingZoneSummary{arn}`) derived from the store — an empty list before any create, exactly the created landing zone's ARN after.
5. **DeleteLandingZone** — `POST /delete-landingzone` (`@idempotent`). Input: `landingZoneIdentifier` (required). Must remove the landing zone from state, record a DELETE operation, and return that `operationIdentifier`. After it, GetLandingZone must return `ResourceNotFoundException` (404) and ListLandingZones must return an empty `landingZones` list. Deleting an unknown identifier -> `ResourceNotFoundException` (404).

NEVER STUBS: each of the five must compute its response from the shared store reached via `crate::state::service_state(&*self.engine)` (see README.md "State"). Hardcoded ARNs, echoed inputs, or a random `operationIdentifier` that is never stored are failures — every identifier returned by a mutating call must be resolvable by GetLandingZoneOperation, and every field returned by GetLandingZone must come from what CreateLandingZone persisted.

## State Model

In `src/state.rs` (`ComAmazonawsControltowerAwscontroltowerapisState`):

- `landing_zones: HashMap<String /* arn */, LandingZone>` where
  `LandingZone { arn: String, version: String, latest_available_version: String, manifest: serde_json::Value, remediation_types: Vec<String>, status: LandingZoneStatus /* ACTIVE|PROCESSING|FAILED */, drift_status: Option<LandingZoneDriftStatusSummary>, tags: BTreeMap<String, String>, created_at: SystemTime }`
  (keyed by ARN; `landingZoneIdentifier` inputs resolve against this key. At most one entry — enforce the one-landing-zone-per-account rule from this map's size.)
- `landing_zone_operations: HashMap<String /* operationIdentifier, 36-char lowercase-hex UUID */, LandingZoneOperation>` where
  `LandingZoneOperation { operation_identifier: String, operation_type: LandingZoneOperationType /* CREATE|UPDATE|RESET|DELETE */, landing_zone_arn: String, status: LandingZoneOperationStatus /* IN_PROGRESS|SUCCEEDED|FAILED */, start_time: SystemTime, end_time: Option<SystemTime>, status_message: Option<String> }`
  (append-only log; survives deletion of the landing zone so GetLandingZoneOperation still resolves the DELETE id, and it is the backing list for the later ListLandingZoneOperations work.)
- `tags: HashMap<String /* resourceArn */, BTreeMap<String, String>>` — populated by CreateLandingZone's `tags` so the later TagResource/ListTagsForResource slice has a home.

Out of scope for this first slice (stay 501, filed separately later): UpdateLandingZone, ResetLandingZone, ListLandingZoneOperations, the baseline family (EnableBaseline / GetBaseline / ListBaselines / GetEnabledBaseline / UpdateEnabledBaseline / ResetEnabledBaseline / DisableBaseline / ListEnabledBaselines / GetBaselineOperation), the control family (EnableControl / GetEnabledControl / UpdateEnabledControl / ResetEnabledControl / DisableControl / ListEnabledControls / GetControlOperation / ListControlOperations), and TagResource / UntagResource / ListTagsForResource.

## Evidence

OPERATIONS.md (crate root) lists all 28 operations with routes and members: CreateLandingZone `POST /create-landingzone` -> required `arn` + `operationIdentifier`; GetLandingZone `POST /get-landingzone` -> required `landingZone` (LandingZoneDetail); GetLandingZoneOperation `POST /get-landingzone-operation` -> required `operationDetails` (LandingZoneOperationDetail); ListLandingZones `POST /list-landingzones` -> required `landingZones`, `maxResults` range 1..1; DeleteLandingZone `POST /delete-landingzone` (@idempotent) -> required `operationIdentifier`; all five declare `ResourceNotFoundException` (404) / `ValidationException` (400) / `ConflictException` (409). Shape detail from smithy_query: `LandingZoneDetail` requires `version` and `manifest` and carries `arn`/`status`/`latestAvailableVersion`/`driftStatus`/`remediationTypes`; `LandingZoneStatus` = ACTIVE|PROCESSING|FAILED; `LandingZoneOperationDetail` = operationType/operationIdentifier/status/startTime/endTime/statusMessage; `LandingZoneOperationType` = DELETE|CREATE|UPDATE|RESET; `LandingZoneOperationStatus` = SUCCEEDED|FAILED|IN_PROGRESS; `LandingZoneSummary` = {arn}. Current implementation: `grep -n '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/*.rs` matches all 28 handler files (create_landing_zone.rs:4, get_landing_zone.rs:4, get_landing_zone_operation.rs:4, list_landing_zones.rs:4, delete_landing_zone.rs:4, ...), each returning `Scaffolded::not_implemented` = HTTP 501. No tests exist beyond the generated `tests/state_roundtrip.rs` example.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `controltower` client at it.
2. `aws controltower create-landing-zone --version 3.3 --manifest file://manifest.json --tags Team=platform` — expect HTTP 200 with a new `arn` and a 36-char lowercase-hex `operationIdentifier`; today it returns HTTP 501 NotImplemented.
3. `aws controltower get-landing-zone-operation --operation-identifier <id from step 2>` — expect `operationDetails.operationType == CREATE`, a `status` of IN_PROGRESS or SUCCEEDED, and a `startTime`.
4. `aws controltower list-landing-zones` — expect exactly one `landingZones` entry whose `arn` equals the ARN from step 2.
5. `aws controltower get-landing-zone --landing-zone-identifier <arn>` — expect `landingZone.version == 3.3`, `landingZone.manifest` byte-equal to manifest.json, and `landingZone.status == ACTIVE`.
6. Call create-landing-zone a second time — expect a modeled `ValidationException` (HTTP 400), not a second landing zone.
7. `aws controltower delete-landing-zone --landing-zone-identifier <arn>` — expect 200 with an `operationIdentifier`; then `get-landing-zone-operation` on it must report `operationType == DELETE`.
8. `aws controltower get-landing-zone --landing-zone-identifier <arn>` — expect `ResourceNotFoundException` (HTTP 404); `aws controltower list-landing-zones` must now return an empty `landingZones` list.
9. Repeat steps 2-5 in a fresh process to confirm responses are computed from the shared store, not canned.
