Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the greengrass emulator

_Filed: 2026-10-21T21:09:00Z_
_Operation: service-com-amazonaws-greengrass-greengrass | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 92 operations in `services/service-com-amazonaws-greengrass-greengrass/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, and `src/state.rs` holds no fields at all (`ComAmazonawsGreengrassGreengrassState { /* TODO */ }`). Nothing about AWS IoT Greengrass V1 works today.

This issue scopes the FIRST buildable slice only: the Greengrass group lifecycle plus one deployment, implemented statefully so a realistic client script runs end to end. Operations outside this slice stay stubbed (501) and will be filed as individual gap issues later.

### Happy path to implement (in order)

1. **CreateGroup** — `POST /greengrass/groups`. Input: `Name` (required), optional `InitialVersion` (GroupVersion), `tags`, `AmznClientToken` header. Must mint a fresh group `Id` (UUID), an `Arn` of the form `arn:aws:greengrass:<region>:<account>:/greengrass/groups/<Id>`, set `CreationTimestamp`/`LastUpdatedTimestamp` (ISO-8601), persist the group, and — when `InitialVersion` is supplied — create version 1 from it and return `LatestVersion`/`LatestVersionArn` pointing at that stored version. With no `InitialVersion`, `LatestVersion`/`LatestVersionArn` must be absent, not fabricated.
2. **CreateGroupVersion** — `POST /greengrass/groups/{GroupId}/versions`. Input: `GroupId` (label, required) plus the component version ARNs (`CoreDefinitionVersionArn`, `FunctionDefinitionVersionArn`, `SubscriptionDefinitionVersionArn`, `DeviceDefinitionVersionArn`, `LoggerDefinitionVersionArn`, `ResourceDefinitionVersionArn`, `ConnectorDefinitionVersionArn`). Must append a new immutable version record (new version UUID, `CreationTimestamp`, version `Arn` = `<group arn>/versions/<Version>`), store the ARN set verbatim, and advance the parent group's `LatestVersion`/`LatestVersionArn`/`LastUpdatedTimestamp`. Output: `Arn`, `CreationTimestamp`, `Id` (the group id), `Version`. Unknown `GroupId` must be `BadRequestException` (HTTP 400), not 200 and not 501.
3. **GetGroup** — `GET /greengrass/groups/{GroupId}`. Must return the stored record: `Id`, `Arn`, `Name`, `CreationTimestamp`, `LastUpdatedTimestamp`, `LatestVersion`, `LatestVersionArn`, `tags` — exactly the values CreateGroup/CreateGroupVersion persisted, never an echo of the current request. Unknown id -> `BadRequestException` 400.
4. **GetGroupVersion** — `GET /greengrass/groups/{GroupId}/versions/{GroupVersionId}`. Must return `Definition` (GroupVersion) reconstructed from the stored version, plus `Arn`, `Id`, `Version`, `CreationTimestamp`. The component ARNs read back must equal those passed to CreateGroupVersion. Unknown group or version -> 400.
5. **ListGroups** — `GET /greengrass/groups`. Input: `MaxResults`, `NextToken` (both query strings). Must enumerate every persisted group as `GroupInformation` entries (`Id`, `Arn`, `Name`, `CreationTimestamp`, `LastUpdatedTimestamp`, `LatestVersion`, `LatestVersionArn`), honour `MaxResults` as a page size, and return an opaque `NextToken` that resumes exactly where the page ended (absent on the last page). Deleted groups must not appear.
6. **CreateDeployment** + **GetDeploymentStatus** — `POST /greengrass/groups/{GroupId}/deployments` with `DeploymentType` (required enum: NewDeployment | Redeployment | ResetDeployment | ForceResetDeployment) and `GroupVersionId`; returns `DeploymentId` + `DeploymentArn`. The deployment must be persisted against the group so `GET /greengrass/groups/{GroupId}/deployments/{DeploymentId}/status` returns the stored `DeploymentType`, `UpdatedAt` and a `DeploymentStatus` (e.g. `Building` -> `Success`) for that exact id. `NewDeployment` with a `GroupVersionId` that is not a stored version of the group -> `BadRequestException` 400.
7. **DeleteGroup** — `DELETE /greengrass/groups/{GroupId}`. Empty body on success; must remove the group and its versions from state so a subsequent GetGroup is `BadRequestException` 400 and ListGroups no longer lists it. Deleting an unknown id -> 400.

Every one of these must compute its response from persisted state. No canned responses, no echoing the input back, no minting an id that is never stored.

## State Model

In `ComAmazonawsGreengrassGreengrassState` (reached only via `crate::state::service_state(&*self.engine)`):

- `groups: HashMap<String /* GroupId */, Group>` where
  `Group { id: String, arn: String, name: String, creation_timestamp: String, last_updated_timestamp: String, latest_version: Option<String>, latest_version_arn: Option<String>, tags: HashMap<String,String>, versions: Vec<GroupVersionRecord>, deployments: Vec<DeploymentRecord>, role_arn: Option<String> }`
- `GroupVersionRecord { version: String, arn: String, creation_timestamp: String, core_definition_version_arn: Option<String>, function_definition_version_arn: Option<String>, subscription_definition_version_arn: Option<String>, device_definition_version_arn: Option<String>, logger_definition_version_arn: Option<String>, resource_definition_version_arn: Option<String>, connector_definition_version_arn: Option<String> }` (append-only; order defines version history for ListGroupVersions later)
- `DeploymentRecord { deployment_id: String, deployment_arn: String, deployment_type: DeploymentType, group_version_id: Option<String>, created_at: String, updated_at: String, status: String, error_message: Option<String>, error_details: Vec<ErrorDetail> }`
- `client_tokens: HashMap<String /* AmznClientToken */, String /* created resource id */>` so a retried CreateGroup/CreateGroupVersion is idempotent rather than creating duplicates.

Ids are UUIDs minted at create time and stored; ARNs are derived from the stored id plus the request's region/account so GetGroup and ListGroups return identical strings.

## Done when

A client script runs CreateGroup -> CreateGroupVersion -> GetGroup -> GetGroupVersion -> ListGroups -> CreateDeployment -> GetDeploymentStatus -> DeleteGroup -> GetGroup(404-equivalent 400) and every response body is consistent with what was written, with no 501 along the way.

## Evidence

`services/service-com-amazonaws-greengrass-greengrass/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-greengrass-greengrass.json`) declares the restJson1 routes and members used above: CreateGroup `POST /greengrass/groups` with required `Name`, optional `InitialVersion`/`tags` and output `Arn`/`Id`/`CreationTimestamp`/`LastUpdatedTimestamp`/`LatestVersion`/`LatestVersionArn`/`Name` (lines 103-108); CreateGroupVersion `POST /greengrass/groups/{GroupId}/versions` with the seven component version ARNs, output `Arn`/`CreationTimestamp`/`Id`/`Version` (lines 117-122); GetGroup `GET /greengrass/groups/{GroupId}` (lines 327-331); GetGroupVersion `GET /greengrass/groups/{GroupId}/versions/{GroupVersionId}` with output `Definition` (GroupVersion) (lines 348-353); ListGroups `GET /greengrass/groups` with `MaxResults`/`NextToken` and output `Groups` (list<GroupInformation>)/`NextToken` (lines 495-500); CreateDeployment `POST /greengrass/groups/{GroupId}/deployments` with required `DeploymentType` enum, output `DeploymentArn`/`DeploymentId` (lines 68-73); GetDeploymentStatus `GET /greengrass/groups/{GroupId}/deployments/{DeploymentId}/status` with output `DeploymentStatus`/`DeploymentType`/`ErrorDetails`/`ErrorMessage`/`UpdatedAt` (lines 292-297); DeleteGroup `DELETE /greengrass/groups/{GroupId}` with no output members (lines 201-206). `BadRequestException (HTTP 400)` is the modeled client error on each of these. Proof of the scaffold state: `src/ops/create_group.rs`, `create_group_version.rs` etc. all carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` line 8 is still `// TODO: add service-specific state fields`, so no handler can persist anything. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST /greengrass/groups with body {"Name":"demo-group"} -> expect 200 with a minted Id/Arn/CreationTimestamp; actual: HTTP 501 NotImplemented
2. POST /greengrass/groups/{GroupId}/versions with {"CoreDefinitionVersionArn":"arn:aws:greengrass:us-east-1:123456789012:/greengrass/definition/cores/abc/versions/1"} -> expect 200 with a new Version and the group's LatestVersion advanced
3. GET /greengrass/groups/{GroupId} -> expect Name=demo-group and LatestVersion equal to the version just created
4. GET /greengrass/groups/{GroupId}/versions/{Version} -> expect Definition.CoreDefinitionVersionArn to equal the ARN sent in step 2
5. GET /greengrass/groups?MaxResults=1 -> expect the group listed in Groups and a NextToken only when more groups exist
6. POST /greengrass/groups/{GroupId}/deployments with {"DeploymentType":"NewDeployment","GroupVersionId":"<Version>"} -> expect 200 with DeploymentId + DeploymentArn
7. GET /greengrass/groups/{GroupId}/deployments/{DeploymentId}/status -> expect DeploymentType=NewDeployment and a DeploymentStatus for that stored deployment
8. DELETE /greengrass/groups/{GroupId} -> expect 200, then GET /greengrass/groups/{GroupId} -> expect BadRequestException (HTTP 400) and the group absent from ListGroups
