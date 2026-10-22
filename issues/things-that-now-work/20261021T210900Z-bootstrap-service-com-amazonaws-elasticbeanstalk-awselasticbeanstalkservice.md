Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Elastic Beanstalk emulator

_Filed: 2026-10-21T21:09:00Z_
_Operation: service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 46 handlers under `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a FIRST, SMALL stateful slice: the application/environment lifecycle, over the `awsQuery` protocol (sdkId `Elastic Beanstalk`, signing name `elasticbeanstalk`).

### Happy path to implement (in order)

1. **CreateApplication** — in: `ApplicationName` (required, len 1..100), `Description` (len 0..200), `ResourceLifecycleConfig`, `Tags`. Out: `Application` (ApplicationDescription) built from what was just stored: `ApplicationName`, `ApplicationArn` (e.g. `arn:aws:elasticbeanstalk:<region>:<account>:application/<name>`), `Description`, `DateCreated`/`DateUpdated` (server clock), empty `Versions`/`ConfigurationTemplates`, `ResourceLifecycleConfig`.
2. **CreateApplicationVersion** — in: `ApplicationName` + `VersionLabel` (both required), `Description`, `SourceBundle` (S3Location), `AutoCreateApplication`, `Process`, `Tags`. Out: `ApplicationVersion` (ApplicationVersionDescription) with `ApplicationName`, `VersionLabel`, `ApplicationVersionArn`, `SourceBundle` echoed from storage, `DateCreated`/`DateUpdated`, `Status` = `Unprocessed`. The new label must also appear in the owning application's `Versions` list on the next DescribeApplications.
3. **CreateEnvironment** — in: `ApplicationName` (required), `EnvironmentName` (len 4..40), `VersionLabel`, `SolutionStackName`/`PlatformArn`, `CNAMEPrefix`, `Description`, `Tier`, `OptionSettings`, `Tags`. Out: the full EnvironmentDescription fields: generated `EnvironmentId` (`e-<8 lowercase alnum>`), `EnvironmentArn`, `EnvironmentName`, `ApplicationName`, `VersionLabel`, `SolutionStackName`, `CNAME` (`<cname-prefix-or-envname>.<region>.elasticbeanstalk.com`), `EndpointURL`, `DateCreated`/`DateUpdated`, `Status` = `Launching`, `Health` = `Grey`, `HealthStatus` = `Pending`, `Tier`, `AbortableOperationInProgress`.
4. **DescribeEnvironments** — in: `ApplicationName`, `VersionLabel`, `EnvironmentIds`, `EnvironmentNames`, `IncludeDeleted`, `MaxRecords` (1..1000), `NextToken`. Out: `Environments` listing the environments actually in the store, filtered by the supplied name/id/application filters, with terminated environments excluded unless `IncludeDeleted` is true. Must return exactly what CreateEnvironment persisted (same `EnvironmentId`, `CNAME`, `VersionLabel`).
5. **DescribeApplications** — in: `ApplicationNames`. Out: `Applications` listing the stored ApplicationDescriptions (all of them when the filter is absent), each with its accumulated `Versions`.
6. **TerminateEnvironment** — in: `EnvironmentId` or `EnvironmentName`, `TerminateResources`, `ForceTerminate`. Out: the environment's description with `Status` = `Terminating` (and the record transitioning to `Terminated` so a later DescribeEnvironments omits it unless `IncludeDeleted=true`).
7. **DeleteApplication** — in: `ApplicationName` (required), `TerminateEnvByForce`. Empty output; the application, its versions and (when `TerminateEnvByForce`) its environments must be gone from subsequent describes.

### Rules

- NEVER STUBS: every one of these must compute its response from persisted state. No canned bodies, no echoing the input without storing it, no generating an `EnvironmentId` that is never written. A create followed by a describe must return what was created.
- Resolve environments by either `EnvironmentId` or `EnvironmentName` wherever both are modeled.
- Minimal error semantics on the happy path: referencing an unknown application/environment must fail rather than return an empty 200 (Elastic Beanstalk uses `InvalidParameterValue` in the awsQuery error envelope); `CreateApplication` / `CreateApplicationVersion` / `CreateEnvironment` with a name that already exists must not silently overwrite.
- Validate the modeled length constraints (`ApplicationName` 1..100, `EnvironmentName` 4..40, `Description` 0..200, `VersionLabel` 1..100) instead of accepting anything.

## State Model

Stored in the shared per-service state (`src/state.rs`, reached via `crate::state::service_state(&*self.engine)`):

- `applications: BTreeMap<String /*ApplicationName*/, Application>` where `Application { name, arn, description, date_created, date_updated, resource_lifecycle_config, configuration_templates: Vec<String>, tags: Vec<Tag> }`
- `application_versions: BTreeMap<(String /*ApplicationName*/, String /*VersionLabel*/), ApplicationVersion>` where `ApplicationVersion { application_name, version_label, arn, description, source_bundle: Option<S3Location>, status: "Unprocessed", date_created, date_updated, tags }` — the application's `Versions` list is derived from this map (newest first).
- `environments: BTreeMap<String /*EnvironmentId*/, Environment>` where `Environment { environment_id, environment_name, arn, application_name, version_label, solution_stack_name, platform_arn, template_name, description, cname, endpoint_url, tier, option_settings: Vec<ConfigurationOptionSetting>, status: EnvironmentStatus, health, health_status, abortable_operation_in_progress, date_created, date_updated, tags }`, plus a `environment_name_index: BTreeMap<String, String>` (name -> id) for name-based lookups.
- `next_id_counter: u64` used only to make `EnvironmentId` values unique; IDs are always written into `environments` before being returned.

All other operations (platform versions, configuration templates/options, health and instance health, managed actions, events, tagging, SwapEnvironmentCNAMEs, UpdateEnvironment, Rebuild/Restart/RequestEnvironmentInfo, ComposeEnvironments, CheckDNSAvailability, CreateStorageLocation, ListAvailableSolutionStacks, …) stay 501 for now and will be filed as individual gap issues.

Done when a realistic client script (AWS CLI/SDK) runs the 7 steps above end to end with correct, stateful response bodies — not merely 2xx.

## Evidence

`services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice/OPERATIONS.md` lists all 47 modeled operations with their input/output members: CreateApplication (l.56-60), CreateApplicationVersion (l.62-66), CreateEnvironment (l.74-78), DescribeApplications (l.128-132), DescribeEnvironments (l.177-181), TerminateEnvironment (l.264-268), DeleteApplication (l.92-96). Output structure members confirmed from the Smithy model shapes `ApplicationDescription` (ApplicationArn/ApplicationName/Description/DateCreated/DateUpdated/Versions/ConfigurationTemplates/ResourceLifecycleConfig), `ApplicationVersionDescription` (ApplicationVersionArn/VersionLabel/SourceBundle/Status/DateCreated...) and `EnvironmentDescription` (EnvironmentId/EnvironmentArn/CNAME/EndpointURL/Status/Health/HealthStatus/Tier...). `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 46 handler files (create_application.rs, create_application_version.rs, create_environment.rs, describe_applications.rs, describe_environments.rs, terminate_environment.rs, delete_application.rs included), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS client at it: `aws --endpoint-url http://localhost:<port> elasticbeanstalk ...`
2. `create-application --application-name my-app --description "demo"` — expect 200 with Application.ApplicationName=my-app and an ApplicationArn; today: HTTP 501 NotImplemented
3. `create-application-version --application-name my-app --version-label v1 --source-bundle S3Bucket=b,S3Key=app.zip` — expect ApplicationVersion with VersionLabel=v1, Status=Unprocessed
4. `describe-applications` — expect my-app present with Versions containing v1
5. `create-environment --application-name my-app --environment-name my-env-1 --version-label v1 --solution-stack-name "64bit Amazon Linux 2 v5.8.0 running Node.js 18"` — expect a generated EnvironmentId (e-xxxxxxxx), a CNAME, Status=Launching, Health=Grey
6. `describe-environments --application-name my-app` — expect exactly that environment with the same EnvironmentId/CNAME/VersionLabel that CreateEnvironment returned
7. `terminate-environment --environment-name my-env-1` — expect Status=Terminating, then `describe-environments` no longer lists it (and `--include-deleted` does)
8. `delete-application --application-name my-app --terminate-env-by-force` then `describe-applications` — expect the application gone
