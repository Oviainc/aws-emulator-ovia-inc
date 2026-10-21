Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the Cloud9 (AWSCloud9WorkspaceManagementService) emulator

_Filed: 2026-10-21T21:57:49Z_
_Operation: service-com-amazonaws-cloud9-awscloud9workspacemanagementservice | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 13 operations in `src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding zero state. Nothing about the Cloud9 environment lifecycle works, so no client script can get past the first call.

Implement a FIRST SMALL SLICE: the create -> read -> list -> update -> delete lifecycle of the primary resource, the Cloud9 **environment**. Wire protocol is `awsJson1_1`, routed as `POST /` with `X-Amz-Target: AWSCloud9WorkspaceManagementService.<OperationName>` (see README.md / OPERATIONS.md).

Happy path, in order:

1. **CreateEnvironmentEC2** — input `name` (required, len 1..60), `instanceType` (required, pattern `^[a-z]+[1-9][.][a-z0-9]+$`), `imageId` (required), optional `description`, `subnetId`, `ownerArn`, `connectionType` (CONNECT_SSH|CONNECT_SSM, default CONNECT_SSH), `automaticStopTimeMinutes`, `tags`, `clientRequestToken`, `dryRun`. Must mint a fresh `environmentId` matching `^[a-zA-Z0-9]{8,32}$`, store the full environment record keyed by that id, and return `environmentId`. The id must be generated AND persisted — a random id with no store behind it is a failure. `dryRun=true` must validate and persist nothing.
2. **DescribeEnvironments** — input `environmentIds` (required list, len 1..25). Must return an `environments` list of `Environment` structures reconstructed from the store: `id`, `name`, `description`, `type` (required; `ec2` for environments made by CreateEnvironmentEC2), `connectionType`, `arn` (required, `arn:aws:cloud9:<region>:<account>:environment:<id>`), `ownerArn` (required), `lifecycle`, `managedCredentialsStatus`. The values must be the ones supplied at create time, not echoes of the describe request.
3. **DescribeEnvironmentStatus** — input `environmentId` (required). Must return the stored `status` (one of connecting|creating|deleting|error|ready|stopped|stopping) plus `message`, both required. A freshly created environment should settle on `ready` (a `creating` -> `ready` transition driven by the stored creation time is acceptable); a deleted one must NOT be reported.
4. **ListEnvironments** — input `nextToken`, `maxResults` (range 0..25). Must return `environmentIds` for every environment currently in the store, in a stable order, honouring `maxResults` and returning an opaque `nextToken` when more remain; the token must resume where the previous page ended, and must be absent on the last page.
5. **UpdateEnvironment** — input `environmentId` (required), optional `name`, `description`, `managedCredentialsAction` (ENABLE|DISABLE). Must mutate the stored record in place (empty output) so a following DescribeEnvironments shows the new name/description; absent members must leave the existing values untouched.
6. **DeleteEnvironment** — input `environmentId` (required). Must remove the environment (and its memberships/tags) from the store, return an empty body, and make every subsequent DescribeEnvironments / DescribeEnvironmentStatus / UpdateEnvironment on that id fail with the modeled `NotFoundException` (HTTP 400). ListEnvironments must no longer list it.

Error semantics required for the slice: an unknown `environmentId` on describe/status/update/delete -> `NotFoundException`; a missing required member or a value violating a modeled length/pattern/range constraint (e.g. `name` longer than 60, malformed `instanceType`, `maxResults` > 25) -> `BadRequestException`. All errors are the modeled awsJson1_1 shapes, not the scaffold 501.

NEVER STUBS: each of these six handlers must compute its response from persisted state reached via `crate::state::service_state(&*self.engine)` (the shared engine on `self`, never a fresh `InMemoryEngine`). No canned ids, no echoed inputs, no success without a corresponding store mutation. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line from each file you implement.

## State Model

In `src/state.rs`, on the service state struct:

- `environments: BTreeMap<String /* environmentId */, Environment>` — BTreeMap so ListEnvironments ordering and pagination tokens are stable.
- `Environment { id: String, name: String, description: Option<String>, env_type: EnvironmentType /* ec2|ssh */, connection_type: ConnectionType, arn: String, owner_arn: String, instance_type: String, image_id: String, subnet_id: Option<String>, automatic_stop_time_minutes: Option<i32>, status: EnvironmentStatus, status_message: String, lifecycle: EnvironmentLifecycle, managed_credentials_status: Option<ManagedCredentialsStatus>, created_at: SystemTime, members: Vec<EnvironmentMember>, tags: BTreeMap<String, String> }`
- `EnvironmentMember { environment_id: String, user_arn: String, user_id: String, permissions: Permissions /* owner|read-only|read-write */, last_access: Option<SystemTime> }` — the creator is seeded as a single `owner` member at create time so the later membership operations have a base.
- `idempotency: BTreeMap<String /* clientRequestToken */, String /* environmentId */>` — so a repeated CreateEnvironmentEC2 with the same token returns the same id instead of creating a second environment (the op is `@idempotent`).

Out of scope for this first slice (stay 501, filed separately later): CreateEnvironmentMembership, UpdateEnvironmentMembership, DeleteEnvironmentMembership, DescribeEnvironmentMemberships, TagResource, UntagResource, ListTagsForResource.

## Evidence

OPERATIONS.md for this crate lists all 13 operations with their input/output members and errors: CreateEnvironmentEC2 (required `name`/`instanceType`/`imageId`, output `environmentId` pattern `^[a-zA-Z0-9]{8,32}$`), DescribeEnvironments (required `environmentIds` len 1..25, output `environments` list<Environment>), DescribeEnvironmentStatus (required output `status` + `message`), ListEnvironments (`@paginated` nextToken/maxResults 0..25, output `environmentIds`), UpdateEnvironment (`environmentId` required, optional `name`/`description`/`managedCredentialsAction`, empty output), DeleteEnvironment (`environmentId` required, empty output) — all declaring `BadRequestException (400)` and `NotFoundException (400)`. The Smithy model's `Environment` shape requires `type`, `arn`, `ownerArn` and carries `id`, `name`, `description`, `connectionType`, `lifecycle`, `managedCredentialsStatus`; `EnvironmentMember` requires `permissions`, `userId`, `userArn`, `environmentId`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 13 handler files (create_environment_ec2.rs, describe_environments.rs, describe_environment_status.rs, list_environments.rs, update_environment.rs, delete_environment.rs, plus the 7 membership/tagging files), proving every operation currently answers 501 and no state exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Cloud9 client at it (awsJson1_1, target prefix `AWSCloud9WorkspaceManagementService`).
2. CreateEnvironmentEC2 with name="demo-env", instanceType="t2.micro", imageId="amazonlinux-2-x86_64" — expect 200 with an `environmentId` matching ^[a-zA-Z0-9]{8,32}$ (today: HTTP 501 NotImplemented).
3. DescribeEnvironments with environmentIds=[<that id>] — expect one Environment whose `name` is "demo-env", `type` is "ec2", `arn` ends in `:environment:<id>`, and `ownerArn` is set.
4. DescribeEnvironmentStatus with that environmentId — expect `status` = "ready" (or "creating" then "ready") and a non-empty `message`.
5. ListEnvironments with maxResults=1 after creating two environments — expect one id plus a `nextToken`; call again with that token and expect the second id and no token.
6. UpdateEnvironment with environmentId=<id>, name="demo-env-renamed" — expect 200 empty body, then DescribeEnvironments must report the new name.
7. DeleteEnvironment with that environmentId — expect 200 empty body; then DescribeEnvironments / DescribeEnvironmentStatus on the same id must return NotFoundException (HTTP 400), and ListEnvironments must no longer include it.
8. CreateEnvironmentEC2 with name="" (or instanceType="bogus") — expect BadRequestException (HTTP 400), not 501 and not a success.
