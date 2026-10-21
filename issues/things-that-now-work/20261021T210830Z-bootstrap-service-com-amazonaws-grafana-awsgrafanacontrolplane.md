Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the grafana (Amazon Managed Grafana) emulator

_Filed: 2026-10-21T21:08:30Z_
_Operation: service-com-amazonaws-grafana-awsgrafanacontrolplane | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 25 operations in `services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/` is an untouched scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Scaffolded::not_implemented(..)` (HTTP 501). The service holds no state, so no client workflow works.

Implement a FIRST, SMALL slice: the workspace lifecycle, the primary resource of the AWS Grafana control plane (restJson1, sdkId `grafana`, sigv4 name `grafana`). Scope is exactly these 5 operations, each computing its response from persisted state — no canned, hardcoded, or echoed-input responses, and no random-ID-without-storage:

1. **CreateWorkspace** — `POST /workspaces` -> HTTP 202. Input: `accountAccessType` (CURRENT_ACCOUNT|ORGANIZATION, required), `permissionType` (CUSTOMER_MANAGED|SERVICE_MANAGED, required), `authenticationProviders` (list, required), plus optional `clientToken` (@idempotencyToken), `workspaceName`, `workspaceDescription`, `workspaceRoleArn`, `organizationRoleName`, `stackSetName`, `workspaceDataSources`, `workspaceNotificationDestinations`, `workspaceOrganizationalUnits`, `tags`, `vpcConfiguration`, `networkAccessControl`, `configuration`, `grafanaVersion`, `ipAddressType`, `kmsKeyId`. Must mint an id matching the modeled pattern `^g-[0-9a-f]{10}$`, persist the full record, and return `workspace` (WorkspaceDescription) whose required members are all populated: `id`, `created`, `modified`, `status`, `endpoint` (length 1..2048, e.g. `https://g-xxxxxxxxxx.grafana-workspace.<region>.amazonaws.com`), `grafanaVersion`, `dataSources`, `authentication` (AuthenticationSummary carrying the requested providers). Honour `clientToken`: a repeated create with the same token returns the same workspace rather than a second one.
2. **DescribeWorkspace** — `GET /workspaces/{workspaceId}` -> 200, `workspace` (WorkspaceDescription, required). Must return exactly what CreateWorkspace stored (name, description, accountAccessType, permissionType, authenticationProviders, tags, grafanaVersion, endpoint, created/modified timestamps). Unknown/未-created id -> `ResourceNotFoundException` (404); an id that violates `^g-[0-9a-f]{10}$` -> `ValidationException` (400).
3. **ListWorkspaces** — `GET /workspaces` -> 200, `workspaces` (list<WorkspaceSummary>, required) + `nextToken`. Must enumerate the persisted workspaces, projecting each stored record to WorkspaceSummary (required: `id`, `created`, `modified`, `status`, `endpoint`, `grafanaVersion`, `authentication`). Respect `maxResults`/`nextToken` paginating over a stable order (the operation is `@paginated(items=workspaces)`).
4. **UpdateWorkspace** — `PUT /workspaces/{workspaceId}` -> 202, returns `workspace` (required). Must mutate only the members present in the request (`workspaceName`, `workspaceDescription`, `accountAccessType`, `permissionType`, `workspaceRoleArn`, `workspaceDataSources`, `workspaceNotificationDestinations`, `vpcConfiguration`/`removeVpcConfiguration`, `networkAccessControl`/`removeNetworkAccessConfiguration`, `ipAddressType`), bump `modified`, and the change must be visible to a following DescribeWorkspace. Unknown id -> 404.
5. **DeleteWorkspace** — `DELETE /workspaces/{workspaceId}` -> 202, returns the deleted `workspace` (required). Must remove the record from state; a subsequent DescribeWorkspace/DeleteWorkspace on that id -> `ResourceNotFoundException` (404), and ListWorkspaces must no longer include it.

## State Model

A service-level store guarded by the usual state lock, e.g.:

- `workspaces: HashMap<String /* workspaceId "g-xxxxxxxxxx" */, Workspace>`
- `Workspace { id: String, name: Option<String>, description: Option<String>, account_access_type: String, permission_type: String, authentication_providers: Vec<String>, saml_configuration: Option<SamlConfiguration>, status: String /* ACTIVE after create */, endpoint: String, grafana_version: String, data_sources: Vec<String>, notification_destinations: Vec<String>, organizational_units: Vec<String>, organization_role_name: Option<String>, stack_set_name: Option<String>, workspace_role_arn: Option<String>, vpc_configuration: Option<VpcConfiguration>, network_access_control: Option<NetworkAccessConfiguration>, ip_address_type: Option<String>, kms_key_id: Option<String>, configuration: String /* JSON, len 2..65536 */, license_type: Option<String>, tags: BTreeMap<String,String>, created: SystemTime, modified: SystemTime }`
- `idempotency_tokens: HashMap<String /* clientToken */, String /* workspaceId */>` so a repeated CreateWorkspace returns the first workspace.
- `workspace_id_seq` / RNG used only to mint ids that match `^g-[0-9a-f]{10}$` — the id must then be stored, never returned without storage.

Everything else (service accounts and their tokens, API keys, permissions, authentication/configuration describe+update, license association, tagging, ListVersions) stays 501 for now and will be filed as individual gap issues.

Done = a client script can create a workspace, describe it and see its own inputs back, list it, update its name/description and observe the change, delete it, and then get a 404 — all against real persisted state.

## Evidence

Spec: `services/service-com-amazonaws-grafana-awsgrafanacontrolplane/smithy-model/com-amazonaws-grafana-awsgrafanacontrolplane.json`; routes, required inputs/outputs and errors as denormalized in the crate's `OPERATIONS.md` (CreateWorkspace `POST /workspaces -> 202` @idempotent; DescribeWorkspace `GET /workspaces/{workspaceId} -> 200`; ListWorkspaces `GET /workspaces -> 200` @paginated(items=workspaces); UpdateWorkspace `PUT /workspaces/{workspaceId} -> 202`; DeleteWorkspace `DELETE /workspaces/{workspaceId} -> 202` @idempotent). Required WorkspaceDescription/WorkspaceSummary members (`id`, `created`, `modified`, `status`, `endpoint`, `grafanaVersion`, `dataSources`, `authentication`) come from the model shapes themselves. Current state: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:'` matches all 25 files in `src/ops/`, including `create_workspace.rs`, `describe_workspace.rs`, `list_workspaces.rs`, `update_workspace.rs`, `delete_workspace.rs` — each returns HTTP 501 and persists nothing. No tests assert any workspace behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI grafana client at it.
2. CreateWorkspace with accountAccessType=CURRENT_ACCOUNT, permissionType=CUSTOMER_MANAGED, authenticationProviders=["AWS_SSO"], workspaceName="demo", workspaceDescription="bootstrap demo", tags={"env":"test"} — expect 202 and a workspace whose id matches ^g-[0-9a-f]{10}$ with endpoint/created/modified/status/grafanaVersion/authentication populated; actual: HTTP 501 NotImplemented.
3. Repeat the same CreateWorkspace with the identical clientToken — expect the same workspaceId returned, not a second workspace.
4. DescribeWorkspace with the returned workspaceId — expect name="demo", description="bootstrap demo", the same authenticationProviders and tags that were created.
5. ListWorkspaces — expect the workspace in `workspaces` as a WorkspaceSummary; call again with maxResults=1 and follow nextToken to confirm pagination over a stable order.
6. UpdateWorkspace with workspaceDescription="updated" only — expect 202, then DescribeWorkspace shows description="updated", name still "demo", and a bumped `modified`.
7. DeleteWorkspace with that id — expect 202 returning the deleted workspace; then DescribeWorkspace on the same id returns ResourceNotFoundException (404) and ListWorkspaces no longer contains it.
8. DescribeWorkspace with id "not-a-workspace" — expect ValidationException (400); with a well-formed but unknown id "g-0123456789" — expect ResourceNotFoundException (404).
