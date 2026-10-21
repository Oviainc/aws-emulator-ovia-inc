Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the AppConfig emulator

_Filed: 2026-10-21T21:18:38Z_
_Operation: service-com-amazonaws-appconfig-amazonappconfig | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 56 AppConfig operations is an unmodified scaffold: each `src/ops/*.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, and the service holds no state. Nothing works end to end.

Implement a FIRST SLICE of 6 operations that make AppConfig's core workflow — organise an app, host a configuration, deploy it to an environment — work statefully against the restJson1 wire protocol (SDK id `AppConfig`, signing name `appconfig`):

1. **CreateApplication** — `POST /applications -> 201`. Input `Name` (required, 1..64), `Description`, `Tags`. Must mint a fresh `Id` matching `^[a-z0-9]{4,7}$`, store the application, and return `{Id, Name, Description}` from the stored record (NOT an echo of a random id that is never persisted).
2. **CreateEnvironment** — `POST /applications/{ApplicationId}/environments -> 201`. Input `ApplicationId` (@httpLabel), `Name` (required), `Description`, `Monitors`, `Tags`. Must 404 `ResourceNotFoundException` when `ApplicationId` is unknown; otherwise mint an `Id`, persist under the parent application, and return `{ApplicationId, Id, Name, Description, State: READY_FOR_DEPLOYMENT, Monitors}`.
3. **CreateConfigurationProfile** — `POST /applications/{ApplicationId}/configurationprofiles -> 201`. Input `Name` + `LocationUri` required (use `hosted` for the hosted store), plus `Description`, `Validators`, `Type`, `RetrievalRoleArn`, `Tags`. 404 on unknown application; mint `Id`; return the stored fields (`ApplicationId, Id, Name, Description, LocationUri, Validators, Type`).
4. **CreateHostedConfigurationVersion** — `POST /applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}/hostedconfigurationversions -> 201`. `Content` is the @httpPayload blob, `ContentType` the required `Content-Type` header, `Description`/`VersionLabel`/`LatestVersionNumber` are headers. Must assign a monotonically increasing `VersionNumber` per (application, profile) starting at 1, store the exact bytes, and return them with the `Application-Id`, `Configuration-Profile-Id`, `Version-Number`, `Content-Type`, `VersionLabel` headers bound by the model. Unknown application/profile -> 404.
5. **StartDeployment** — `POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments -> 201`. Required `DeploymentStrategyId`, `ConfigurationProfileId`, `ConfigurationVersion`; optional `Description`, `Tags`, `KmsKeyIdentifier`. Must assign `DeploymentNumber` monotonically per (application, environment) starting at 1, resolve the strategy for `DeploymentDurationInMinutes`/`GrowthType`/`GrowthFactor`/`FinalBakeTimeInMinutes` (accept the predefined `AppConfig.*` strategy ids, e.g. `AppConfig.AllAtOnce`), persist the deployment with `State`, `StartedAt`, `PercentageComplete`, and `ConfigurationName`/`ConfigurationLocationUri` copied from the profile. 404 for unknown application/environment/profile/strategy; `ConflictException` (409) if a deployment is already in progress for that environment.
6. **GetDeployment** — `GET /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber} -> 200`. Must return the PERSISTED deployment created in step 5 — same `ConfigurationProfileId`, `ConfigurationVersion`, `DeploymentStrategyId`, duration/growth/bake values, `State` (`COMPLETE` with `PercentageComplete: 100` and `CompletedAt` set for an all-at-once strategy, or `DEPLOYING`), and a non-empty `EventLog`. 404 `ResourceNotFoundException` for an unknown deployment number.

No canned, hardcoded or echoed responses: every field above must be computed from persisted state, and every id/version/deployment number must be stored, not merely generated. Reads must observe prior writes through the shared engine store (`crate::state::service_state(&*self.engine)`), never a locally constructed engine.

## State Model

In `src/state.rs`, hang the following off the service state struct:

- `applications: HashMap<String /*app id*/, Application>` where `Application { id, name, description, tags: HashMap<String,String>, environments: HashMap<String /*env id*/, Environment>, profiles: HashMap<String /*profile id*/, ConfigurationProfile> }`
- `Environment { id, application_id, name, description, state: EnvironmentState, monitors: Vec<Monitor>, tags, deployments: HashMap<i32 /*deployment number*/, Deployment>, next_deployment_number: i32 }`
- `ConfigurationProfile { id, application_id, name, description, location_uri, retrieval_role_arn, validators: Vec<Validator>, profile_type, kms_key_identifier, tags, hosted_versions: BTreeMap<i32 /*version number*/, HostedConfigurationVersion>, next_version_number: i32 }`
- `HostedConfigurationVersion { version_number, description, content: Vec<u8>, content_type, version_label }`
- `Deployment { deployment_number, application_id, environment_id, deployment_strategy_id, configuration_profile_id, configuration_name, configuration_location_uri, configuration_version, description, deployment_duration_in_minutes, growth_type, growth_factor, final_bake_time_in_minutes, state: DeploymentState, percentage_complete, started_at, completed_at, event_log: Vec<DeploymentEvent> }`
- `deployment_strategies: HashMap<String /*id*/, DeploymentStrategy>` pre-seeded with the AWS predefined strategies (`AppConfig.AllAtOnce`, `AppConfig.Linear50PercentEvery30Seconds`, `AppConfig.Canary10Percent20Minutes`) so StartDeployment resolves them without a prior CreateDeploymentStrategy.
- counters for id minting (4-7 lowercase alphanumeric ids, unique per resource map).

The remaining 50 operations (deployment strategies CRUD, extensions, extension associations, experiments, tagging, list/update/delete variants, GetConfiguration) stay 501 for now and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-appconfig-amazonappconfig/OPERATIONS.md` lines 26-80 (CreateApplication/CreateEnvironment/CreateConfigurationProfile/CreateHostedConfigurationVersion routes, required members, errors), lines 146-224 (GetApplication/GetDeployment/GetHostedConfigurationVersion outputs), lines 321-326 (StartDeployment input/output incl. `DeploymentNumber`, `State`, `PercentageComplete`, ConflictException). Wire protocol restJson1 per OPERATIONS.md line 10. Handler proof: `src/ops/create_application.rs` lines 4 and 169 — `//! SCAFFOLDED, NOT IMPLEMENTED:` and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateApplication"))`; all 55 other files in `src/ops/` are the same generated stub, so no test asserts any behaviour today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `appconfig` client at it.
2. `aws appconfig create-application --name MyApp` -> expect 201 with a 4-7 char lowercase `Id`; today returns HTTP 501 NotImplemented.
3. `aws appconfig create-environment --application-id <app> --name prod` -> expect 201 with `State=READY_FOR_DEPLOYMENT`.
4. `aws appconfig create-configuration-profile --application-id <app> --name MyConfig --location-uri hosted` -> expect 201 with the stored `LocationUri`.
5. `aws appconfig create-hosted-configuration-version --application-id <app> --configuration-profile-id <profile> --content-type application/json --content fileb://config.json out.json` -> expect 201, `VersionNumber=1`, and `out.json` byte-identical to `config.json`.
6. `aws appconfig start-deployment --application-id <app> --environment-id <env> --deployment-strategy-id AppConfig.AllAtOnce --configuration-profile-id <profile> --configuration-version 1` -> expect 201 with `DeploymentNumber=1`.
7. `aws appconfig get-deployment --application-id <app> --environment-id <env> --deployment-number 1` -> expect the same profile id, configuration version 1, strategy values and a terminal/active `State` with a non-empty `EventLog`, all read back from stored state.
8. `aws appconfig get-deployment ... --deployment-number 99` -> expect 404 ResourceNotFoundException.
