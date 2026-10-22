Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 85378260976efce74d968b045ae62c2fe2a03011

# Bootstrap the ssm-quicksetup emulator

_Filed: 2026-10-21T22:28:11Z_
_Operation: service-com-amazonaws-ssmquicksetup-quicksetup | Class: missing_

> _Note: this issue was identified against the code base at commit `85378260976efce74d968b045ae62c2fe2a03011`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST SMALL SLICE: the configuration-manager lifecycle, served statefully over `restJson1` (sigv4 signing name `ssm-quicksetup`).

### Happy path to implement (in order)

1. **CreateConfigurationManager** — `POST /configurationManager`. Input: `Name`, `Description`, `ConfigurationDefinitions` (required, list of `ConfigurationDefinitionInput{Type (required, pattern ^[a-zA-Z0-9_\-.:/]{3,200}$), Parameters (required map), TypeVersion, LocalDeploymentExecutionRoleName, LocalDeploymentAdministrationRoleArn}`), `Tags` (map). Must mint a manager ARN of the form `arn:aws:ssm-quicksetup:<region>:<account>:configuration-manager/<uuid>`, assign each configuration definition a server-generated `Id`, create one Configuration record per definition, persist everything, and return `ManagerArn` (required). Reject a missing/empty `ConfigurationDefinitions` or a `Type` violating the modeled pattern with `ValidationException` (400).
2. **GetConfigurationManager** — `GET /configurationManager/{ManagerArn}`. Must read the persisted record and return `ManagerArn`, `Name`, `Description`, `CreatedAt`, `LastModifiedAt`, `ConfigurationDefinitions` (each with the `Id` assigned at create and the parameters as stored), `Tags`, and `StatusSummaries`. Unknown ARN -> `ResourceNotFoundException` (404).
3. **ListConfigurationManagers** — `POST /listConfigurationManagers`. Input `MaxItems`, `StartingToken`, `Filters`. Must return `ConfigurationManagersList` of `ConfigurationManagerSummary{ManagerArn (required), Name, Description, StatusSummaries, ConfigurationDefinitionSummaries}` built from the store — one entry per created manager, none after deletion. Honor `MaxItems` as a page bound and emit/accept `NextToken` so paging through N managers visits each exactly once.
4. **UpdateConfigurationManager** — `PUT /configurationManager/{ManagerArn}`. Input `Name`, `Description`. Must mutate the stored record (and bump `LastModifiedAt`) so a subsequent GetConfigurationManager returns the new values; empty output body, 200. Unknown ARN -> `ResourceNotFoundException` (404).
5. **DeleteConfigurationManager** — `DELETE /configurationManager/{ManagerArn}`. Must remove the manager and its child configurations from the store; empty output body, 200. Afterwards GetConfigurationManager -> 404 and the manager is absent from ListConfigurationManagers. Idempotent per `@idempotent`: deleting an already-deleted ARN must not panic (404 for an ARN never created).
6. **ListConfigurations** — `POST /listConfigurations` with `ManagerArn` / `ConfigurationDefinitionId` filters. Must return `ConfigurationsList` of the configuration records created in step 1 (`Id`, `ManagerArn`, `ConfigurationDefinitionId`, `Type`, `TypeVersion`, `Account`, `Region`, `CreatedAt`, `StatusSummaries`), filtered by the supplied `ManagerArn`; unknown `ManagerArn` -> `ResourceNotFoundException` (404).

NO canned, echoed or hardcoded responses: each of the six must compute its response from persisted state. A create followed by a get/list must return exactly what was created, including the server-assigned definition `Id`s and timestamps. A random ARN returned without storing the manager is a failure of this issue.

## State Model

In `src/state.rs` (`ComAmazonawsSsmquicksetupQuicksetupState`, reached via `crate::state::service_state(&*self.engine)`):

- `managers: BTreeMap<String /* ManagerArn */, ConfigurationManagerRecord>` — insertion-ordered-by-ARN map, the single source of truth.
- `ConfigurationManagerRecord { manager_arn: String, name: Option<String>, description: Option<String>, created_at: SystemTime, last_modified_at: SystemTime, tags: BTreeMap<String,String>, definitions: Vec<ConfigurationDefinitionRecord>, status_summaries: Vec<StatusSummaryRecord> }`
- `ConfigurationDefinitionRecord { id: String /* server-assigned, e.g. "1" or uuid */, type_: String, type_version: Option<String>, parameters: BTreeMap<String,String>, local_deployment_execution_role_name: Option<String>, local_deployment_administration_role_arn: Option<String> }`
- `configurations: BTreeMap<String /* configuration Id */, ConfigurationRecord>` where `ConfigurationRecord { id, manager_arn, configuration_definition_id, type_, type_version, account, region, created_at, last_modified_at, parameters: BTreeMap<String,String>, status_summaries: Vec<StatusSummaryRecord> }` — one row per definition at create time, removed when the manager is deleted.
- `StatusSummaryRecord { status_type: StatusType /* Deployment | AsyncExecutions */, status: Status /* INITIALIZING|DEPLOYING|SUCCEEDED|... */, status_message: Option<String>, last_updated_at: SystemTime, status_details: BTreeMap<String,String> }` — seed `Deployment`/`SUCCEEDED` at create so reads return a modeled-valid summary.
- `next_definition_id: u64` — monotonic counter backing definition/configuration ID assignment.

List pagination should be a deterministic offset/ARN cursor encoded into `NextToken` over `managers` / `configurations`.

The remaining operations (GetConfiguration, GetServiceSettings, UpdateServiceSettings, UpdateConfigurationDefinition, ListQuickSetupTypes, TagResource, UntagResource, ListTagsForResource) stay stubbed at 501 for now and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-ssmquicksetup-quicksetup/OPERATIONS.md` lists all 14 operations with routes, members and errors: CreateConfigurationManager (`POST /configurationManager`, required `ConfigurationDefinitions`, output required `ManagerArn`), GetConfigurationManager (`GET /configurationManager/{ManagerArn}`, @readonly, ResourceNotFoundException 404), ListConfigurationManagers (`POST /listConfigurationManagers`, @paginated inputToken=StartingToken outputToken=NextToken items=ConfigurationManagersList), UpdateConfigurationManager (`PUT /configurationManager/{ManagerArn}`, @idempotent), DeleteConfigurationManager (`DELETE /configurationManager/{ManagerArn}`, @idempotent), ListConfigurations (`POST /listConfigurations`, @paginated/@readonly, ManagerArn + ConfigurationDefinitionId filters). Shape details confirmed via smithy_query for `ConfigurationDefinitionInput` (required Type with pattern `^[a-zA-Z0-9_\-.:/]{3,200}$`, required Parameters), `ConfigurationDefinition` (adds server-side `Id`), `ConfigurationManagerSummary` (required ManagerArn + ConfigurationDefinitionSummaries), `StatusSummary` (required StatusType + LastUpdatedAt) and the `Status`/`StatusType` enums. Every handler file under `src/ops/` is an unmodified scaffold carrying the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker described in the crate README and returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, so no state exists and no test asserts any of this behavior.

## Steps to Reproduce

1. Start the emulator and point an SSM QuickSetup client at it (restJson1, signing name `ssm-quicksetup`).
2. CreateConfigurationManager with Name="demo", Description="bootstrap", ConfigurationDefinitions=[{Type: "AWSQuickSetupType-SSMOpsCenter", Parameters: {DelegatedAccountId: "111122223333", TargetOrganizationalUnits: "ou-1", TargetRegions: "us-east-1"}}], Tags={env: dev} -> expect 200 with a real ManagerArn; today 501 NotImplemented.
3. GetConfigurationManager(ManagerArn) -> expect 200 echoing Name/Description/Tags plus CreatedAt and ConfigurationDefinitions with a server-assigned Id and the parameters exactly as submitted.
4. ListConfigurationManagers({}) -> expect the created manager present in ConfigurationManagersList with its ManagerArn and Name; with MaxItems=1 over two managers, expect a NextToken that pages to the second without repeats.
5. ListConfigurations({ManagerArn}) -> expect one ConfigurationSummary per configuration definition created in step 2, carrying the same ConfigurationDefinitionId and Type.
6. UpdateConfigurationManager(ManagerArn, Name="demo-renamed") -> 200, then GetConfigurationManager returns Name="demo-renamed" and a bumped LastModifiedAt.
7. DeleteConfigurationManager(ManagerArn) -> 200, then GetConfigurationManager(ManagerArn) -> ResourceNotFoundException 404 and ListConfigurationManagers no longer lists it.
