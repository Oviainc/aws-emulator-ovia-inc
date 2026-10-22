Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the WorkSpaces Thin Client emulator

_Filed: 2026-10-21T21:18:31Z_
_Operation: service-com-amazonaws-workspacesthinclient-thinclient | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-workspacesthinclient-thinclient/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL, stateful slice covering the environment lifecycle — the service's primary resource (restJson1, sigv4 signing name `thinclient`).

### Happy path to implement (in order)

1. **CreateEnvironment** — `POST /environments` -> 201. Inputs: `desktopArn` (required), `name`, `desktopEndpoint`, `softwareSetUpdateSchedule` (APPLY_IMMEDIATELY|USE_MAINTENANCE_WINDOW), `softwareSetUpdateMode` (USE_DESIRED|USE_LATEST), `desiredSoftwareSetId`, `maintenanceWindow`, `kmsKeyArn`, `clientToken`, `tags`, `deviceCreationTags`. Must MINT and PERSIST a new environment: generate an `id` matching `^[a-z0-9]{9}$`, an `arn` of the form `arn:aws:thinclient:<region>:<account>:environment/<id>`, an `activationCode` matching `^[a-z]{2}[a-z0-9]{6}$`, `createdAt`/`updatedAt` timestamps, `registeredDevicesCount` = 0, and a `desktopType` derived from `desktopArn`'s service component (workspaces / workspaces-web / appstream). Return the stored record projected into `EnvironmentSummary` (`id`, `name`, `arn`, `desktopArn`, `desktopEndpoint`, `desktopType`, `activationCode`, `softwareSetUpdateSchedule`, `softwareSetUpdateMode`, `desiredSoftwareSetId`, `createdAt`, `updatedAt`). `clientToken` must be idempotent: a repeat with the same token returns the SAME environment id, not a new one.
2. **GetEnvironment** — `GET /environments/{id}` -> 200, output `environment` (Environment). Must look the record up in state and return every field as stored (including `kmsKeyArn`, `deviceCreationTags`, `maintenanceWindow`, `registeredDevicesCount`). Unknown/valid-shaped id -> `ResourceNotFoundException` 404.
3. **ListEnvironments** — `GET /environments` -> 200, output `environments` (list<EnvironmentSummary>) + `nextToken`. Must enumerate the environments actually in state, honour `maxResults` (1..50) by truncating, and emit an opaque `nextToken` that resumes exactly where the previous page stopped; the final page omits `nextToken`. A deleted environment must not appear.
4. **UpdateEnvironment** — `PATCH /environments/{id}` -> 200, output `environment` (EnvironmentSummary). Must mutate the PERSISTED record for the supplied members only (`name`, `desktopArn`, `desktopEndpoint`, `softwareSetUpdateSchedule`, `maintenanceWindow`, `softwareSetUpdateMode`, `desiredSoftwareSetId`, `deviceCreationTags`), leave omitted members untouched, bump `updatedAt`, and return the updated projection. A subsequent GetEnvironment must show the new values. Unknown id -> 404.
5. **DeleteEnvironment** — `DELETE /environments/{id}` -> 204 (empty body). Must REMOVE the record from state; a following GetEnvironment -> `ResourceNotFoundException` 404 and the id disappears from ListEnvironments. Delete of an already-deleted id -> 404.
6. **ListTagsForResource / TagResource** — `GET /tags/{resourceArn}` and `POST /tags/{resourceArn}`. Tags passed to CreateEnvironment, plus tags added by TagResource, must be stored against the environment's ARN and returned verbatim by ListTagsForResource. Unknown ARN -> 404.

Every one of these MUST compute its response from persisted state. No canned bodies, no echoing the request back, no minting an id that is never stored. 501 for any of the six is a failure of this issue.

## State Model

Service state lives behind `crate::state::service_state(&*self.engine)` (the guard shown in each scaffold's module docs) and should hold:

- `environments: HashMap<String /*environmentId, ^[a-z0-9]{9}$*/, Environment>` where `Environment { id, arn, name, desktop_arn, desktop_endpoint, desktop_type, activation_code, registered_devices_count: i32, software_set_update_schedule, software_set_update_mode, desired_software_set_id, pending_software_set_id, pending_software_set_version, software_set_compliance_status, maintenance_window, kms_key_arn, device_creation_tags: HashMap<String,String>, created_at, updated_at }`
- `environment_order: Vec<String>` — insertion order, so ListEnvironments pagination is stable and `nextToken` can encode an offset or last-seen id.
- `tags: HashMap<String /*resourceArn*/, HashMap<String,String>>` — backing ListTagsForResource/TagResource/UntagResource for environment (and later device/software-set) ARNs.
- `idempotency: HashMap<String /*clientToken*/, String /*environmentId*/>` — so a replayed CreateEnvironment/Delete returns the original result.
- Reserved for the next slice (may stay empty now): `devices: HashMap<String /*^[a-zA-Z0-9]{24}$*/, Device>` and `software_sets: HashMap<String /*^[0-9]{1,9}$*/, SoftwareSet>`.

Ids, ARNs, activation codes and timestamps are generated once at create time and stored; reads must never regenerate them.

### Explicitly out of scope for this first slice

GetDevice, ListDevices, UpdateDevice, DeleteDevice, DeregisterDevice, GetSoftwareSet, ListSoftwareSets, UpdateSoftwareSet and UntagResource stay at 501 and will be filed as individual gap issues later. (Devices in the real service only appear after physical hardware registers with an activation code, so there is no client-callable create for them — they are a poor fit for the first slice.)

## Evidence

`services/service-com-amazonaws-workspacesthinclient-thinclient/OPERATIONS.md` lists 16 operations with their routes, members and errors: CreateEnvironment (`POST /environments` -> 201, required `desktopArn`, output `environment`/EnvironmentSummary), GetEnvironment (`GET /environments/{id}` -> 200, output `environment`/Environment, ResourceNotFoundException 404), ListEnvironments (`GET /environments`, @paginated inputToken=nextToken/outputToken=nextToken/items=environments, `maxResults` range 1..50), UpdateEnvironment (`PATCH /environments/{id}` -> 200), DeleteEnvironment (`DELETE /environments/{id}` -> 204), TagResource/ListTagsForResource (`/tags/{resourceArn}`). The `Environment` and `EnvironmentSummary` shapes from the Smithy model (`smithy_query` query=shape) define the fields named above, including `id` pattern `^[a-z0-9]{9}$`, `activationCode` pattern `^[a-z]{2}[a-z0-9]{6}$`, `registeredDevicesCount`, `arn`, `createdAt`/`updatedAt`. Every handler file, e.g. `src/ops/create_environment.rs:4` (`//! SCAFFOLDED, NOT IMPLEMENTED:`) and line 198 (`Err(crate::scaffolded::Scaffolded::not_implemented("CreateEnvironment"))`), proves nothing is implemented and no state is touched. No tests exist for this crate.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `workspaces-thin-client` client at it.
2. `CreateEnvironment` with `desktopArn=arn:aws:workspaces:us-east-1:123456789012:workspace/ws-abc123` and `name=demo`, `tags={Owner:demo}` — expect 201 with `environment.id` matching ^[a-z0-9]{9}$, a non-empty `arn` and `activationCode`; actual: HTTP 501 NotImplemented.
3. `GetEnvironment` with the returned id — expect 200 with `environment.name == "demo"`, the same `desktopArn`, `registeredDevicesCount == 0` and the same `activationCode` as returned by create.
4. `ListEnvironments` with `maxResults=1` after creating two environments — expect 1 item plus a `nextToken`, and a second call with that token returning the other environment and no `nextToken`.
5. `UpdateEnvironment` on the id with `name=demo2`, then `GetEnvironment` again — expect `name == "demo2"`, `desktopArn` unchanged and a later `updatedAt`.
6. `ListTagsForResource` with the environment's `arn` — expect `{Owner: demo}`; after `TagResource` adds `{Env: test}`, expect both tags.
7. `DeleteEnvironment` on the id — expect 204; then `GetEnvironment` expects ResourceNotFoundException (HTTP 404) and `ListEnvironments` no longer includes the id.
