Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Interconnect emulator

_Filed: 2026-10-21T21:08:33Z_
_Operation: service-com-amazonaws-interconnect-interconnect | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 13 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a first, small, stateful slice covering the Connection lifecycle on a pre-seeded Environment. Wire protocol is `awsJson1_0`: `POST /` with `X-Amz-Target: Interconnect.<OperationName>` (see README/OPERATIONS.md).

### Happy path to implement (in order)

1. **ListEnvironments** (@readonly, @paginated items=environments) — input `maxResults` (1..100), `nextToken` (1..1024), optional filters `provider` (Provider union: `cloudServiceProvider` | `lastMileProvider`) and `location`. Output `environments` (list<Environment>, required) + `nextToken`. Environments are AWS-managed — there is no CreateEnvironment operation in the model — so the service must SEED a small fixed catalog of Environments at startup and list them from that store, applying the `provider`/`location` filters and honoring `maxResults`/`nextToken` as a real page cursor. Each Environment must carry its required members: `provider`, `location`, `environmentId`, `state` (available|limited|unavailable), `bandwidths`, `type` (ProductType), plus optional `activationPageUrl`, `remoteIdentifierType`.

2. **GetEnvironment** (@readonly) — input `id` (1..64, required). Output `environment` (Environment, required) — must be the SAME record ListEnvironments returned for that id, looked up from the seeded store (not synthesized per call). An unknown id must not return a fabricated environment.

3. **CreateConnection** (@idempotent) — input `bandwidth` (required, `^\d+[MG]bps$`), `attachPoint` (AttachPoint union: `directConnectGateway` UUID | `arn`, required), `environmentId` (required), optional `description` (`^[-a-zA-Z0-9_ ]+$`), `remoteAccount`, `tags`, `clientToken` (@idempotencyToken). Output `connection` (Connection). Must allocate and PERSIST a new Connection with all required members computed by the server: `id` matching `^(mcc|lmcc)-[a-z0-9]{8}$` (prefix derived from the environment's provider kind — `mcc` for cloudServiceProvider, `lmcc` for lastMileProvider), `arn` = `arn:aws:interconnect:<region>:<account>:connection/<id>`, `description`, `bandwidth`, `attachPoint`, `environmentId`, `provider` and `location` and `type` COPIED FROM the referenced Environment (not echoed from input — the input has no such fields), `state` = `requested` (the modeled initial state), `sharedId` (a UUID), `ownerAccount` (12-digit account), `activationKey` (1..2048), `tags`. `clientToken`/@idempotent: a repeat call with the same client token must return the previously created connection rather than allocating a second one. `bandwidth` must be validated against the environment's `bandwidths` set.

4. **GetConnection** (@readonly) — input `identifier` (`^(mcc|lmcc)-[a-z0-9]{8}$`, required). Output `connection` — must return the stored record byte-for-byte equal in content to what CreateConnection returned (same `id`, `arn`, `sharedId`, `activationKey`, `state`, `tags`). A `identifier` that was never created must not yield a hollow 200 with a zeroed Connection.

5. **ListConnections** (@readonly, @paginated items=connections) — input `maxResults`, `nextToken`, and filters `state` (ConnectionState), `environmentId`, `provider`, `attachPoint`. Output `connections` (list<ConnectionSummary>) + `nextToken`. Must enumerate the persisted connection store, project each stored Connection into a ConnectionSummary (`id`, `arn`, `description`, `bandwidth`, `attachPoint`, `environmentId`, `provider`, `location`, `type`, `state`, `sharedId`, `billingTier`), apply EVERY supplied filter, and page with a real cursor so that `maxResults=1` twice walks two distinct connections.

6. **UpdateConnection** (@idempotent) — input `identifier` (required), optional `description`, `bandwidth`, `clientToken`. Output `connection`. Must mutate the STORED record in place (only the supplied fields; absent fields unchanged) and a subsequent GetConnection must observe the new values. Unknown identifier must not create a record.

7. **DeleteConnection** (@idempotent) — input `identifier` (required), `clientToken`. Output `connection` (required). Must transition the stored record's `state` to `deleting`/`deleted` and return the final Connection; after it, GetConnection must no longer report the connection as `available`/`requested` and ListConnections without a `state` filter must reflect the terminal state (or omit it, consistently with the chosen model).

NO operation in this slice may return a canned, hardcoded, or echoed-input response. Each must read/write the shared per-service store reached through `crate::state::service_state(&*self.engine)` (never a freshly constructed engine — see README), and each must compute server-owned fields (`id`, `arn`, `sharedId`, `activationKey`, `state`, `provider`, `location`, `type`) rather than copying them from the request. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each of the seven handler files as it is implemented.

## State Model

In `src/state.rs` (`ComAmazonawsInterconnectInterconnectState`), hold:

- `environments: BTreeMap<String /* environmentId */, Environment>` — SEEDED at state construction with a handful of fixed entries; fields: `environment_id`, `provider: Provider{ cloud_service_provider | last_mile_provider }`, `location`, `state: EnvironmentState`, `bandwidths: Vec<String>` (e.g. `["1Gbps", "10Gbps"]`), `product_type: String`, `activation_page_url: Option<String>`, `remote_identifier_type: Option<RemoteAccountIdentifierType>`.
- `connections: BTreeMap<String /* connection id, mcc-xxxxxxxx */, StoredConnection>` where `StoredConnection { id, arn, description, bandwidth, attach_point: AttachPoint, environment_id, provider, location, product_type, state: ConnectionState, shared_id: String /* uuid */, billing_tier: Option<i32>, owner_account: String, activation_key: String, tags: BTreeMap<String,String>, remote_account: Option<RemoteAccountIdentifier>, created_at }`.
- `connection_order: Vec<String>` (or rely on the BTreeMap's stable key order) to give ListConnections a deterministic sequence for cursor-based pagination.
- `idempotency_tokens: HashMap<String /* clientToken */, String /* connection id */>` so repeated @idempotent CreateConnection / UpdateConnection / DeleteConnection calls replay the first outcome.
- `activation_keys: HashMap<String /* activationKey */, String /* connection id */>` — the reverse index needed later by DescribeConnectionProposal / AcceptConnectionProposal (populate it on create even though those two ops stay stubbed in this slice).
- `attach_points: BTreeMap<String /* environmentId */, Vec<AttachPointDescriptor{ type, identifier, name }>>` — seeded, for the later ListAttachPoints.

Tags are stored on `StoredConnection.tags` keyed by the connection ARN lookup, so the later TagResource / UntagResource / ListTagsForResource slice reads and writes the same map rather than a separate store.

Out of scope for this first slice (they stay 501 and will be filed as individual gap issues): DescribeConnectionProposal, AcceptConnectionProposal, ListAttachPoints, TagResource, UntagResource, ListTagsForResource.

Done = an integration test that lists environments, gets one by id, creates a connection on it, gets it back and asserts the returned `id`/`arn`/`sharedId`/`state` match the create response, lists it (with a `state` filter and with `maxResults=1`), updates its description, re-gets it to see the new description, deletes it, and confirms the terminal state — all passing with correct response bodies, not merely 2xx.

## Evidence

`services/service-com-amazonaws-interconnect-interconnect/OPERATIONS.md` lists all 13 operations with their input/output members and traits (awsJson1_0, target prefix `Interconnect.`); it is the source for every field named above. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-interconnect-interconnect/src/ops/` matches all 13 handler files (accept_connection_proposal.rs, create_connection.rs, delete_connection.rs, describe_connection_proposal.rs, get_connection.rs, get_environment.rs, list_attach_points.rs, list_connections.rs, list_environments.rs, list_tags_for_resource.rs, tag_resource.rs, untag_resource.rs, update_connection.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501. The Smithy model (`smithy_query` on Connection, ConnectionSummary, Environment, AttachPoint, Provider, ConnectionState, AttachPointDescriptor) grounds the required members and constraints: Connection requires id/arn/description/bandwidth/attachPoint/environmentId/provider/location/type/state/sharedId/ownerAccount/activationKey; ConnectionState enum = available|requested|pending|down|deleting|deleted|failed|updating; the model documents `requested` as "the initial state of a connection" and `deleting`/`deleted` as the delete transitions; ListConnections/ListEnvironments/ListAttachPoints are @paginated with inputToken/outputToken=nextToken; CreateConnection/UpdateConnection/DeleteConnection are @idempotent with an @idempotencyToken clientToken. There is no CreateEnvironment operation in the model, which is why Environments must be seeded. No tests currently assert any of this (only the generated `tests/state_roundtrip.rs` example exists).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `interconnect` at it (awsJson1_0, POST / with X-Amz-Target: Interconnect.<Op>).
2. Call ListEnvironments (no filters) — expect a non-empty `environments` list of seeded Environments, each with provider/location/environmentId/state/bandwidths/type. Today: HTTP 501 NotImplemented.
3. Call GetEnvironment with the `environmentId` of the first listed environment — expect the identical Environment record.
4. Call CreateConnection with that `environmentId`, a `bandwidth` from that environment's `bandwidths`, an `attachPoint` of `{"directConnectGateway":"<uuid>"}`, a `description`, `tags={"env":"test"}` and a `clientToken` — expect a `connection` whose `id` matches ^(mcc|lmcc)-[a-z0-9]{8}$, with a well-formed `arn`, `state`=`requested`, a UUID `sharedId`, an `activationKey`, and `provider`/`location`/`type` copied from the environment.
5. Repeat the exact CreateConnection call with the SAME clientToken — expect the SAME connection id back (idempotent), not a second connection.
6. Call GetConnection with that `identifier` — expect the same stored record (same id, arn, sharedId, activationKey, tags, state) as the create response.
7. Call ListConnections with `environmentId` set and `maxResults=1` — expect exactly one ConnectionSummary for the created connection plus a `nextToken` only when more exist; re-call with the token and expect the next distinct connection, never a repeat of the first.
8. Call ListConnections with `state="deleted"` — expect the created connection to be absent (filter actually applied).
9. Call UpdateConnection with that identifier and a new `description`, leaving `bandwidth` unset — expect the returned connection to show the new description and the UNCHANGED bandwidth; then GetConnection again and expect the new description to persist.
10. Call DeleteConnection with that identifier — expect the required `connection` in the response with `state` in {deleting, deleted}; then GetConnection/ListConnections and confirm the connection is no longer reported as requested/available.

## Resolution note

The slice above is implemented: ListEnvironments, GetEnvironment, CreateConnection, GetConnection, ListConnections, UpdateConnection and DeleteConnection all read and write the shared per-service store, and `tests/all/connection_lifecycle.rs` exercises them over the wire.

Six operations deliberately stay at HTTP 501 and are NOT yet filed as separate reports — the plan above promised individual gap issues for them, and this change files none, so they are tracked here and by the scaffold census (`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-interconnect-interconnect/src/ops/`, which still matches exactly these six files):

- `describe_connection_proposal.rs` — DescribeConnectionProposal (the `activation_keys` reverse index it needs is already populated on create).
- `accept_connection_proposal.rs` — AcceptConnectionProposal.
- `list_attach_points.rs` — ListAttachPoints (the `attach_points` seed it needs is already in state).
- `tag_resource.rs`, `untag_resource.rs`, `list_tags_for_resource.rs` — the tagging trio, which must read and write `StoredConnection.connection.tags` rather than a second store.

`Connection$billingTier` is left unset: the member is optional and AWS publishes no rule for assigning a tier, so the emulator reports nothing there rather than an invented ordinal.
