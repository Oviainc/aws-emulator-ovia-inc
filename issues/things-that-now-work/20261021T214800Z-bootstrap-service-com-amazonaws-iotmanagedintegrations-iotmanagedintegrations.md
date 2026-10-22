Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the iotmanagedintegrations emulator

_Filed: 2026-10-21T21:48:00Z_
_Operation: service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 83 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL slice: the managed thing (the service's primary resource) create -> read -> list -> update -> delete lifecycle, backed by real persisted state in `ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState`.

Happy path to implement, in order (restJson1 wire protocol, sigv4 name `iotmanagedintegrations`):

1. **CreateManagedThing** — `POST /managed-things` -> 201. Required input: `Role` (CONTROLLER|DEVICE), `AuthenticationMaterial`, `AuthenticationMaterialType` (e.g. ZWAVE_QR_BAR_CODE). Optional and all to be STORED, not discarded: `Owner`, `CredentialLockerId`, `SerialNumber`, `Brand`, `Model`, `Name`, `Classification`, `Capabilities`, `CapabilityReport`, `CapabilitySchemas`, `MetaData`, `Tags`, `WiFiSimpleSetupConfiguration`, `ClientToken` (@idempotencyToken). Output must be computed: a freshly generated `Id` matching `^[a-zA-Z0-9:_-]*$` (len 1..64), an `Arn` of the form `arn:aws:iotmanagedintegrations:<region>:<account>:managed-thing/<Id>`, and `CreatedAt` set to the insert time. The returned Id must be the key under which the record is stored.

2. **GetManagedThing** — `GET /managed-things/{Identifier}` -> 200. Must look the record up by the Id returned from step 1 and echo back from STORED state: `Id`, `Arn`, `Role`, `Owner`, `CredentialLockerId`, `Name`, `Brand`, `Model`, `SerialNumber`, `Classification`, `MetaData`, `Tags`, `WiFiSimpleSetupConfiguration`, `CreatedAt`, `UpdatedAt`, and a `ProvisioningStatus` the create step assigned (e.g. `UNASSOCIATED`). Unknown identifier -> `ResourceNotFoundException` (HTTP 404), not 200 and not 501.

3. **ListManagedThings** — `GET /managed-things` -> 200. `Items` is a list of `ManagedThingSummary` built from the stored map (Id, Arn, Role, Name, Brand, Model, SerialNumber, Owner, CredentialLockerId, Classification, ProvisioningStatus, CreatedAt, UpdatedAt). Honor `MaxResults` (range 1..1000) by truncating and emitting an opaque `NextToken` (pattern `^[a-zA-Z0-9=_-]+$`) that resumes exactly where the page stopped; honor at least the `OwnerFilter`, `RoleFilter`, `SerialNumberFilter` and `ProvisioningStatusFilter` query params as server-side filters. Empty store -> empty `Items` and no `NextToken`.

4. **UpdateManagedThing** — `PUT /managed-things/{Identifier}` -> 204, @idempotent, empty body. Must mutate the stored record in place for whichever of `Owner`, `CredentialLockerId`, `SerialNumber`, `Brand`, `Model`, `Name`, `Classification`, `Capabilities`, `CapabilityReport`, `CapabilitySchemas`, `HubNetworkMode`, `MetaData`, `WiFiSimpleSetupConfiguration` are present (absent members left untouched), and bump `UpdatedAt`. A subsequent GetManagedThing must observe the new values. Unknown identifier -> `ResourceNotFoundException` (404).

5. **DeleteManagedThing** — `DELETE /managed-things/{Identifier}` -> 204, @idempotent. Must remove the record from the store (honoring the `Force` query flag at least by accepting it). After deletion, GetManagedThing on the same Id must return `ResourceNotFoundException` (404) and ListManagedThings must no longer include it.

Every one of these five must compute its response from persisted state. No canned bodies, no echoing the request back, and no generating an Id without storing anything behind it — a create followed by a get must return what was created, and a delete followed by a get must 404. All five share ONE engine-held store reached via `crate::state::service_state(&*self.engine)`.

## State Model

In `src/state.rs`, on the service state struct:

- `managed_things: HashMap<String /* Id */, ManagedThing>` — the authoritative store, keyed by the generated managed-thing Id.
- `ManagedThing { id: String, arn: String, role: Role, provisioning_status: ProvisioningStatus, authentication_material: String, authentication_material_type: AuthMaterialType, owner: Option<String>, credential_locker_id: Option<String>, serial_number: Option<String>, brand: Option<String>, model: Option<String>, name: Option<String>, classification: Option<String>, capabilities: Option<String>, capability_report: Option<CapabilityReport>, capability_schemas: Vec<CapabilitySchemaItem>, hub_network_mode: Option<HubNetworkMode>, wifi_simple_setup_configuration: Option<WiFiSimpleSetupConfiguration>, meta_data: HashMap<String,String>, tags: HashMap<String,String>, created_at: SystemTime, updated_at: SystemTime, activated_at: Option<SystemTime> }`
- `managed_thing_order: Vec<String>` — insertion order of Ids, so ListManagedThings pages deterministically.
- `idempotency_tokens: HashMap<String /* ClientToken */, String /* Id */>` — so a replayed CreateManagedThing with the same `@idempotencyToken` returns the original Id/Arn/CreatedAt instead of creating a duplicate.
- `list_tokens: HashMap<String /* NextToken */, usize /* offset into managed_thing_order */>` (or an encoded offset) — so pagination tokens are honored rather than ignored.

Everything else (credential lockers, cloud connectors, connector/account associations, destinations, OTA tasks, provisioning profiles, device discovery, notification/event-log/hub/encryption configuration, schema versions, SendManagedThingCommand, tagging) stays 501 for now and will be filed as individual gap issues.

Done = an integration test that drives the five operations above end-to-end against the emulator passes with full fidelity — not merely 2xx, but correct bodies: the Id/Arn from create reappear in get and list, the update is visible in a later get, and the delete makes the resource 404.

## Evidence

All 83 files under `services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header at line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) — verified by grep over the whole ops directory, including `create_managed_thing.rs`, `get_managed_thing.rs`, `list_managed_things.rs`, `update_managed_thing.rs` and `delete_managed_thing.rs`. The operation contracts, routes, status codes, required/optional members, pagination traits and error lists cited above come from the crate's generated `OPERATIONS.md` (Wire protocol section; CreateManagedThing lines 399-404, GetManagedThing 406-412, UpdateManagedThing 414-420, DeleteManagedThing 422-428, ListManagedThings 430-436), and the `ManagedThingSummary` member list comes from `smithy_query shape=ManagedThingSummary` against `smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json`. No tests exercise these operations today (only the generated `tests/state_roundtrip.rs` example exists).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `iotmanagedintegrations` at it.
2. CreateManagedThing with Role=DEVICE, AuthenticationMaterialType=ZWAVE_QR_BAR_CODE, AuthenticationMaterial=<qr payload>, Name="living-room-lamp", Brand="Acme", Model="L1", SerialNumber="SN-0001", MetaData={"room":"living"} — expect 201 with a generated Id, a managed-thing Arn containing that Id, and CreatedAt. Actual today: HTTP 501 NotImplemented.
3. GetManagedThing with Identifier=<Id from step 2> — expect 200 echoing the stored Name/Brand/Model/SerialNumber/MetaData/Role plus Arn, CreatedAt and ProvisioningStatus. Actual today: HTTP 501.
4. ListManagedThings with MaxResults=1 — expect the created thing in Items as a ManagedThingSummary, and a NextToken only if more remain. Actual today: HTTP 501.
5. UpdateManagedThing with Identifier=<Id> and Name="den-lamp" — expect 204, then GetManagedThing returns Name="den-lamp" with a bumped UpdatedAt. Actual today: HTTP 501.
6. DeleteManagedThing with Identifier=<Id> — expect 204, then GetManagedThing on the same Id returns ResourceNotFoundException (HTTP 404) and ListManagedThings returns empty Items. Actual today: HTTP 501.
7. GetManagedThing with Identifier="does-not-exist" — expect ResourceNotFoundException (HTTP 404).
