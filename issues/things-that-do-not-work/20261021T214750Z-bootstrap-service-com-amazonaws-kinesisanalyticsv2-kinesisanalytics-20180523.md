Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the Kinesis Analytics V2 (Managed Service for Apache Flink) emulator

_Filed: 2026-10-21T21:47:50Z_
_Operation: service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523 | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 33 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service. Implement a FIRST, SMALL, stateful slice: the application lifecycle happy path. Operations in scope, in order:

1. **CreateApplication** — input `ApplicationName` (required, 1..128, `^[a-zA-Z0-9_.-]+$`), `RuntimeEnvironment` (required enum, e.g. `FLINK-1_20`, `SQL-1_0`), `ServiceExecutionRole` (required, `^arn:`), optional `ApplicationDescription`, `ApplicationConfiguration`, `CloudWatchLoggingOptions`, `Tags`, `ApplicationMode`. Must persist a new application and return `ApplicationDetail` (required) with a synthesized `ApplicationARN` (`arn:aws:kinesisanalytics:<region>:<account>:application/<name>`), `ApplicationStatus` = `READY`, `ApplicationVersionId` = 1, `CreateTimestamp`, and the stored `RuntimeEnvironment`/`ServiceExecutionRole`/`ApplicationDescription`/`ApplicationMode`. Creating a name that already exists must fail with `ResourceInUseException` (400); an unknown `RuntimeEnvironment` or a `ServiceExecutionRole` not matching `^arn:` must fail with `InvalidArgumentException` (400).
2. **DescribeApplication** — input `ApplicationName` (required), optional `IncludeAdditionalDetails`. Must return the PERSISTED `ApplicationDetail` for that application (same ARN, status, version id, timestamps, config description as created/updated), and `ResourceNotFoundException` (400) for an unknown name.
3. **ListApplications** — inputs `Limit` (1..50), `NextToken`. Must return one `ApplicationSummary` per stored application (`ApplicationName`, `ApplicationARN`, `ApplicationStatus`, `ApplicationVersionId`, `RuntimeEnvironment`, `ApplicationMode`) in the required `ApplicationSummaries` list, honouring `Limit` and returning/consuming an opaque `NextToken` (must match `^[a-zA-Z0-9_.-]+$`, len 1..128) when more remain.
4. **StartApplication** — input `ApplicationName` (required), optional `RunConfiguration`. Must transition the stored application `READY` -> `RUNNING` (via `STARTING` is acceptable as long as a subsequent DescribeApplication reports `RUNNING`) and return a generated `OperationId` (1..64) that is recorded. Starting an application that is already `RUNNING` must fail with `ResourceInUseException` (400); unknown name -> `ResourceNotFoundException`.
5. **StopApplication** — input `ApplicationName` (required), optional `Force`. Must transition `RUNNING` -> `READY` in the store and return an `OperationId`. Stopping an application that is not running must fail with `ResourceInUseException` (400).
6. **DeleteApplication** — inputs `ApplicationName` (required) and `CreateTimestamp` (required). Must remove the application from the store only when the supplied `CreateTimestamp` equals the stored one (mismatch -> `InvalidArgumentException`), reject deleting a `RUNNING` application with `ResourceInUseException`, and return an empty body. Afterwards DescribeApplication must return `ResourceNotFoundException` and ListApplications must not include it.

Every one of these must compute its response from the persisted state held in the shared engine (`crate::state::service_state(&*self.engine)`), as described in README.md. No canned bodies, no echoing the request back, no generating an ARN/OperationId without storing it: a create followed by a describe must return exactly what was created, and start/stop must be observable through describe/list. The wire protocol is `awsJson1_1` on `POST /` with `X-Amz-Target: KinesisAnalytics_20180523.<OperationName>` (README/OPERATIONS.md), and modeled errors must be returned as the modeled error shapes with their documented HTTP status (409 for `ConcurrentModificationException`, 400 for the rest).

## State Model

In `src/state.rs`, on the service state struct:

- `applications: BTreeMap<String /* ApplicationName */, Application>` — BTreeMap so ListApplications has a stable, token-resumable order.
- `Application { name: String, arn: String, description: Option<String>, runtime_environment: String /* RuntimeEnvironment enum value */, service_execution_role: String, application_mode: Option<String>, status: String /* ApplicationStatus: READY | STARTING | RUNNING | STOPPING | DELETING */, version_id: i64 /* starts at 1 */, create_timestamp: DateTime, last_update_timestamp: Option<DateTime>, configuration: Option<ApplicationConfigurationDescription>, cloud_watch_logging_options: Vec<CloudWatchLoggingOptionDescription>, conditional_token: Option<String>, tags: BTreeMap<String, String>, operations: Vec<AppOperation> }`
- `AppOperation { operation_id: String, operation: String /* "StartApplication" | "StopApplication" | ... */, status: String /* IN_PROGRESS | SUCCESSFUL | FAILED | CANCELLED */, start_time: DateTime, end_time: Option<DateTime> }` — populated by Start/Stop so the returned `OperationId` is backed by stored state (this is also what ListApplicationOperations/DescribeApplicationOperation will later read).

Done = a client script that runs CreateApplication -> DescribeApplication -> ListApplications -> StartApplication -> StopApplication -> DeleteApplication succeeds end to end with correct, stateful response bodies (not merely 2xx).

Everything outside this slice (AddApplication*/DeleteApplication* sub-resources, UpdateApplication, RollbackApplication, snapshots, versions, operations listing, tagging, DiscoverInputSchema, CreateApplicationPresignedUrl, UpdateApplicationMaintenanceConfiguration) stays 501 for now and will be filed as individual gap issues later.

## Evidence

All 33 handler files under `services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker on line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501 with zero persisted state (grep of the marker matches every file, including create_application.rs, describe_application.rs, list_applications.rs, start_application.rs, stop_application.rs, delete_application.rs). Field names, requiredness, constraints and error/status mappings above are taken from the crate's generated `OPERATIONS.md` (CreateApplication, DescribeApplication, ListApplications, StartApplication, StopApplication, DeleteApplication sections) and from the Smithy model shapes `ApplicationDetail` and `ApplicationSummary` (both require `ApplicationARN`, `ApplicationName`, `ApplicationStatus`, `ApplicationVersionId`, `RuntimeEnvironment`). Wire protocol (`awsJson1_1`, `X-Amz-Target: KinesisAnalytics_20180523.*`) from OPERATIONS.md 'Wire protocol'. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI v2 kinesisanalyticsv2 client at it.
2. CreateApplication with ApplicationName=my-app, RuntimeEnvironment=FLINK-1_20, ServiceExecutionRole=arn:aws:iam::123456789012:role/svc — expect 200 with ApplicationDetail{ApplicationARN ending in application/my-app, ApplicationStatus=READY, ApplicationVersionId=1, CreateTimestamp set}; today: HTTP 501 NotImplemented.
3. DescribeApplication with ApplicationName=my-app — expect the same ARN/status/version/runtime that were just created.
4. ListApplications — expect ApplicationSummaries to contain my-app with status READY and version 1.
5. StartApplication with ApplicationName=my-app — expect 200 with an OperationId; then DescribeApplication must report ApplicationStatus=RUNNING.
6. StopApplication with ApplicationName=my-app — expect 200 with an OperationId; then DescribeApplication must report ApplicationStatus=READY.
7. DeleteApplication with ApplicationName=my-app and CreateTimestamp taken from the describe response — expect 200; a following DescribeApplication must return ResourceNotFoundException (HTTP 400) and ListApplications must no longer list my-app.
8. Negative checks: CreateApplication for an existing name -> ResourceInUseException; DescribeApplication for an unknown name -> ResourceNotFoundException; DeleteApplication with a wrong CreateTimestamp -> InvalidArgumentException.
