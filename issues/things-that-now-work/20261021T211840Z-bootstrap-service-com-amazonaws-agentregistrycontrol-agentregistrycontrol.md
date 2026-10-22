Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the agentregistrycontrol emulator

_Filed: 2026-10-21T21:18:40Z_
_Operation: service-com-amazonaws-agentregistrycontrol-agentregistrycontrol | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 15 handlers under `services/service-com-amazonaws-agentregistrycontrol-agentregistrycontrol/src/ops/` are unmodified scaffolds: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, holding no state. No client workflow works today.

Implement a FIRST, SMALL slice: the registry + registry-record happy path (restJson1 wire protocol, sigv4 signing name `agent-registry`). These 6 operations, in order:

1. **CreateRegistry** — `POST /registries` -> 202. Input: `name` (required, 1..64, `^[a-zA-Z0-9][a-zA-Z0-9_\-\.\/]*$`), `description`, `encryptionConfiguration`, `discoveryConfiguration`, `approvalConfiguration`, `customMetadataSchemaConfiguration`, `autoDetectionConfiguration`, `clientToken` (@idempotencyToken), `tags`. Must mint a registryId matching `^[a-zA-Z0-9]{12,16}$`, build `registryArn` = `arn:aws:agent-registry:<region>:<account>:registry/<registryId>`, persist the whole input plus `status`, `createdAt`, `updatedAt`, and return `registryArn` (required). Duplicate `name` -> `ConflictException` (409); same `clientToken` + same input -> same registryArn (idempotent replay), not a second registry.
2. **GetRegistry** — `GET /registries/{registryId}` -> 200. Must look up by the persisted id AND accept the full ARN form (the label pattern allows `arn:...:registry/` prefix) and return the stored `name`, `description`, `registryId`, `registryArn`, `discoveryConfiguration`, `encryptionConfiguration`, `approvalConfiguration`, `customMetadataSchemaConfiguration`, `status` (RegistryStatus, `READY` once created), `createdAt`, `updatedAt` — all echoing what CreateRegistry stored, not defaults. Unknown id -> `ResourceNotFoundException` (404).
3. **CreateRegistryRecord** — `POST /registries/{registryId}/records` -> 202. Input: `name` (required), `recordType` (required, AGENT|CUSTOM|GATEWAY|MCP|SKILL), `descriptors` (required), `displayName`, `description`, `recordVersion`, `provenance`, `customMetadata`, `tags`, `clientToken`. Must 404 when the parent registry does not exist, mint a recordId matching `^[a-zA-Z0-9]{12}$`, build `recordArn` = `<registryArn>/record/<recordId>`, persist everything with `createdAt`/`updatedAt` and an initial `status` of `DRAFT`, and return `recordArn` + `status` computed from the stored record.
4. **GetRegistryRecord** — `GET /registries/{registryId}/records/{recordId}` -> 200. Must return the persisted record: `registryArn`, `recordArn`, `recordId`, `name`, `displayName`, `description`, `recordType`, `descriptors`, `recordVersion`, `status`, `createdAt`, `updatedAt`, `statusReason`, `provenance`, `customMetadata`. Unknown registry or record -> `ResourceNotFoundException` (404).
5. **ListRegistryRecords** — `POST /registries/{registryId}/records-list` -> 200. Must return `registryRecords` (list of RegistryRecordSummary built from stored records of that registry only) and honour `maxResults` (1..100) by truncating and emitting an opaque `nextToken` that resumes exactly where the previous page stopped; absent next page -> no `nextToken`. A record created in step 3 must appear here; one deleted in step 6 must not.
6. **DeleteRegistryRecord** — `DELETE /registries/{registryId}/records/{recordId}` -> 200, empty body. Must remove the record from state so a following GetRegistryRecord returns 404 and ListRegistryRecords no longer lists it. @idempotent: deleting an already-absent record should not panic (404 is the modeled answer for an unknown record).

Also implement **DeleteRegistry** (`DELETE /registries/{registryId}` -> 202, returns `status`) if it comes cheaply, so the lifecycle closes: after it, GetRegistry returns 404.

Every one of these MUST compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)`. No canned responses, no echoing the request back, no minting an id that is never stored. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line from each file you implement.

## State Model

In `src/state.rs`, on `ComAmazonawsAgentregistrycontrolAgentregistrycontrolState`:

- `registries: HashMap<String /* registryId */, Registry>`
- `registry_name_index: HashMap<String /* name */, String /* registryId */>` — for ConflictException on duplicate create
- `idempotency: HashMap<String /* clientToken */, String /* registryArn or recordArn */>` — for @idempotencyToken replay

`Registry { registry_id: String, registry_arn: String, name: String, description: Option<String>, encryption_configuration: Option<EncryptionConfiguration>, discovery_configuration: Option<DiscoveryConfiguration>, approval_configuration: Option<ApprovalConfiguration>, custom_metadata_schema_configuration: Option<CustomMetadataSchemaConfiguration>, auto_detection: Option<AutoDetection>, status: RegistryStatus, status_reason: Option<String>, created_at: DateTime, updated_at: DateTime, tags: BTreeMap<String,String>, records: BTreeMap<String /* recordId */, RegistryRecord> }`

`RegistryRecord { record_id: String, record_arn: String, registry_arn: String, name: String, display_name: Option<String>, description: Option<String>, record_type: RecordType, descriptors: Option<Descriptors>, record_version: Option<String>, status: RegistryRecordStatus, status_reason: Option<String>, provenance: Vec<Provenance>, custom_metadata: Option<Document>, custom_metadata_schema_compliance_status: Option<CustomMetadataSchemaComplianceStatus>, created_by: Option<String>, created_by_auto_detection: Option<bool>, created_at: DateTime, updated_at: DateTime, tags: BTreeMap<String,String> }`

Using `records` as a `BTreeMap` keyed by recordId gives ListRegistryRecords a stable order, which is what makes the `nextToken` resume point well defined. Tags stored on each resource are what the later TagResource/UntagResource/ListTagsForResource slice will read, so put the maps in now even if those ops stay stubbed.

Out of scope for this first slice (stay 501, filed separately later): UpdateRegistry, UpdateRegistryRecord, UpdateRegistryRecordStatus, SubmitRegistryRecordForApproval, ListRegistries filters, TagResource, UntagResource, ListTagsForResource, approval/auto-detection state machines, custom-metadata schema compliance evaluation.

## Evidence

OPERATIONS.md (crate root) lists 15 operations with routes, required members and errors: CreateRegistry `POST /registries` -> 202 returning required `registryArn`; GetRegistry `GET /registries/{registryId}` -> 200 with required `name`/`registryId`/`registryArn`/`status`/`createdAt`/`updatedAt`; CreateRegistryRecord `POST /registries/{registryId}/records` -> 202 with required `name`/`recordType`/`descriptors` and required output `recordArn`/`status`; GetRegistryRecord `GET /registries/{registryId}/records/{recordId}` -> 200; ListRegistryRecords `POST /registries/{registryId}/records-list` -> 200 with `@paginated(inputToken=nextToken, outputToken=nextToken, items=registryRecords)` and `maxResults` range 1..100; DeleteRegistryRecord `DELETE /registries/{registryId}/records/{recordId}` -> 200; DeleteRegistry `DELETE /registries/{registryId}` -> 202 returning required `status`. Each declares `ResourceNotFoundException (HTTP 404)` and `ValidationException (HTTP 400)`; the create ops declare `ConflictException (HTTP 409)` and carry `clientToken` with `@idempotencyToken`. Handler census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-agentregistrycontrol-agentregistrycontrol/src/ops/` matches all 15 files, each returning `Scaffolded::not_implemented(..)` (HTTP 501) with zero state access. README.md confirms the restJson1 protocol and that handlers reach state via `crate::state::service_state(&*self.engine)`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `agent-registry-control` at it.
2. CreateRegistry with name="my-registry", description="demo" — expect 202 and a `registryArn` of the form `arn:aws:agent-registry:<region>:<account>:registry/<12-16 alnum>` (today: HTTP 501).
3. GetRegistry with the registryId from that ARN — expect 200 with name="my-registry", description="demo", status=READY and the createdAt/updatedAt stored at create time.
4. CreateRegistry again with the same name — expect ConflictException (409). Re-send the original CreateRegistry with the same clientToken — expect the SAME registryArn, and only one registry in state.
5. CreateRegistryRecord on that registry with name="checkout-agent", recordType=AGENT and descriptors set — expect 202 with a `recordArn` = <registryArn>/record/<12 alnum> and status=DRAFT.
6. GetRegistryRecord for that recordId — expect 200 echoing the persisted name, recordType, descriptors, status and timestamps.
7. ListRegistryRecords on the registry — expect the record present in `registryRecords`; create a second record and call again with maxResults=1 — expect one item plus a `nextToken`, and passing that token returns the other record and no token.
8. DeleteRegistryRecord for the first record — expect 200; GetRegistryRecord on it now returns ResourceNotFoundException (404) and ListRegistryRecords no longer lists it.
9. GetRegistryRecord/GetRegistry on a never-created id — expect ResourceNotFoundException (404), not 501 and not an empty 200.
10. DeleteRegistry on the registry — expect 202 with a `status`; GetRegistry then returns 404.
