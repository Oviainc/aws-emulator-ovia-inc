Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the Lambda Core (lambdacoreapiservice) emulator

_Filed: 2026-10-21T22:28:24Z_
_Operation: service-com-amazonaws-lambdacore-lambdacoreapiservice | Class: missing_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 6 operations in this crate is an unmodified scaffold: each `src/ops/*.rs` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds only `// TODO: add service-specific state fields`. No client workflow can run.

This bootstrap issue scopes the FIRST, SMALL slice: the create -> read -> list -> update -> delete lifecycle of the service's primary (and only) resource, the **network connector**. Protocol is `restJson1`, sigv4 signing name `lambda`, URI prefix `/2026-04-04/network-connectors`.

### Happy path to implement, in order

1. **CreateNetworkConnector** — `POST /2026-04-04/network-connectors` -> **202**. Input: `Name` (required), `Configuration` (required, union `NetworkConnectorConfiguration` with member `VpcEgressConfiguration`), optional `OperatorRole`, `ClientToken` (@idempotencyToken), `Tags`. Must allocate and persist: an `Id`, the unqualified `Arn` (`arn:aws:lambda:<region>:<account>:network-connector:<Name>`), version `1` and its qualified `VersionArn` (`<Arn>:1`), the stored `Configuration`/`OperatorRole`, and `State`. Response fields `Arn`, `Name`, `Id` are required and must be the persisted values (not echoes of an input that was dropped). Creating a second connector with the same `Name` must fail with `ResourceConflictException` (409). The op is `@idempotent`: a repeat with the same `ClientToken` must return the SAME `Arn`/`Id`/`Version`, not a second resource.

2. **GetNetworkConnector** — `GET /2026-04-04/network-connectors/{Identifier}` -> **200**. `Identifier` must resolve by name, by unqualified ARN, and by version-qualified ARN (`<Arn>:<n>`). Must return the stored `Arn`, `Name`, `Id`, `Configuration`, `OperatorRole`, `State`, plus `Version`/`VersionArn` for the resolved version and `LatestVersion`/`LatestVersionArn` for the connector, and `LastModified`. Unknown identifier -> `ResourceNotFoundException` (404).

3. **ListNetworkConnectors** — `GET /2026-04-04/network-connectors` -> **200**. Returns `NetworkConnectors: list<NetworkConnectorSummary>` (required; `Arn`, `Name`, `Id`, `Type`, `State`, `LastModified`, `LatestVersionArn`, `LatestVersion`) built from the store — every created connector appears, deleted ones do not. Honour the `State` query filter, `MaxItems` (1..100) and `Marker`/`NextMarker` paging (`NextMarker` present only when more remain).

4. **UpdateNetworkConnector** — `PUT /2026-04-04/network-connectors/{Identifier}` -> **202**. Applies `Configuration` and/or `OperatorRole` and, per the model's version semantics ("each configuration update creates a new immutable version"), appends a NEW version: `Version` = previous + 1 with `VersionArn` = `<Arn>:<Version>`, bumps `LastModified`, and sets `LastUpdateStatus`. A following GetNetworkConnector must observe the new configuration and the bumped `LatestVersion`. Unknown identifier -> `ResourceNotFoundException` (404).

5. **ListNetworkConnectorVersions** — `GET /2026-04-04/network-connectors/{Identifier}/versions` -> **200**. Returns `NetworkConnectorVersions: list<NetworkConnectorVersionSummary>` (`Arn`, `VersionArn`, `Name`, `Id`, `Version`, `State`, `LastModified`) — one entry per persisted version, so after one update the list has 2 entries. Same `State`/`MaxItems`/`Marker` handling as above; unknown identifier -> 404.

6. **DeleteNetworkConnector** — `DELETE /2026-04-04/network-connectors/{Identifier}` -> **202**. Removes the connector and all its versions from the store and echoes the deleted resource's persisted `Arn`, `Name`, `Id`, `Configuration`, `State`. A subsequent GetNetworkConnector -> `ResourceNotFoundException` (404) and the connector is absent from ListNetworkConnectors. Being `@idempotent`, a second delete of an already-absent connector must not panic (404 is acceptable).

Every one of these must compute its response from persisted state held in the shared engine (`crate::state::service_state(&*self.engine)`), reached through the lock guard. No canned responses, no echoing an input the handler never stored, no random IDs that are not written to the store. Each handler's `SCAFFOLDED, NOT IMPLEMENTED` marker line must be deleted when implemented.

## State Model

In `ComAmazonawsLambdacoreLambdacoreapiserviceState`:

- `connectors: BTreeMap<String /* name */, NetworkConnector>` — ordered so list paging is stable.
- `by_arn: BTreeMap<String /* unqualified Arn */, String /* name */>` (or resolve by scanning) so `{Identifier}` accepts name, ARN or qualified ARN.
- `idempotency: HashMap<String /* ClientToken */, String /* name */>` so a replayed CreateNetworkConnector returns the original resource.
- `NetworkConnector { name: String, id: String, arn: String, type_: NetworkConnectorType /* VPC_EGRESS */, operator_role: Option<String>, state: NetworkConnectorState, state_reason: Option<String>, state_reason_code: Option<NetworkConnectorStateReasonCode>, last_update_status: Option<NetworkConnectorLastUpdateStatus>, last_update_status_reason: Option<String>, last_modified: Timestamp, tags: BTreeMap<String,String>, latest_version: i64, versions: BTreeMap<i64, ConnectorVersion> }`
- `ConnectorVersion { version: i64, version_arn: String, configuration: NetworkConnectorConfiguration, operator_role: Option<String>, state: NetworkConnectorState, last_modified: Timestamp }`

The remaining model-only concerns (asynchronous PENDING -> ACTIVE transitions, `DELETE_FAILED`/`FAILED` state-reason codes, tag APIs, limit exceeded) stay out of this first slice and are filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-lambdacore-lambdacoreapiservice/OPERATIONS.md` lists all 6 operations with their routes, required inputs/outputs and errors (restJson1, sigv4 name `lambda`, prefix `/2026-04-04/network-connectors`). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all six handler files (create_network_connector.rs, get_network_connector.rs, update_network_connector.rs, delete_network_connector.rs, list_network_connectors.rs, list_network_connector_versions.rs), each returning HTTP 501. `src/state.rs` contains no fields (`// TODO: add service-specific state fields`), so no state is held. Version semantics are grounded in the model: `NetworkConnectorSummary.LatestVersion` doc — "Each configuration update creates a new immutable version; this attribute exposes the most recent one" — and `NetworkConnectorVersionSummary` doc. Required output members `Arn`/`Name`/`Id` and the paginated traits (`inputToken=Marker`, `outputToken=NextMarker`, `MaxItems` range 1..100) come from the Smithy model via smithy_query. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/`curl` client at the Lambda Core endpoint.
2. POST /2026-04-04/network-connectors with {"Name":"my-connector","Configuration":{"VpcEgressConfiguration":{...}},"ClientToken":"tok-1"} — expect 202 with Arn, Name=my-connector, Id, Version=1, VersionArn=<Arn>:1. Today: HTTP 501 NotImplemented.
3. Repeat the identical POST with ClientToken "tok-1" — expect the same Arn/Id/Version (idempotent replay), not a second connector; a different-ClientToken POST with the same Name — expect 409 ResourceConflictException.
4. GET /2026-04-04/network-connectors/my-connector — expect 200 with the Configuration and OperatorRole exactly as created, LatestVersion=1.
5. GET /2026-04-04/network-connectors — expect NetworkConnectors to contain one summary for my-connector with its Arn/Id/State/LatestVersion; check MaxItems=1 with several connectors returns NextMarker and the follow-up Marker call returns the rest without duplicates.
6. PUT /2026-04-04/network-connectors/my-connector with a changed Configuration — expect 202 with Version=2 and VersionArn=<Arn>:2; GET again and expect the new Configuration and LatestVersion=2.
7. GET /2026-04-04/network-connectors/my-connector/versions — expect 2 entries (Version 1 and 2) with distinct VersionArns.
8. DELETE /2026-04-04/network-connectors/my-connector — expect 202 echoing the persisted Arn/Name/Id; then GET the same identifier — expect 404 ResourceNotFoundException and an empty ListNetworkConnectors.
