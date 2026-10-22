Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the kinesisanalytics emulator

_Filed: 2026-10-21T21:08:42Z_
_Operation: service-com-amazonaws-kinesisanalytics-kinesisanalytics-20150814 | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 20 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow works at all. Implement a first, small, stateful slice covering the Kinesis Analytics (v1, SQL applications) application lifecycle:

1. **CreateApplication** — input `ApplicationName` (required, 1..128, `^[a-zA-Z0-9_.-]+$`), `ApplicationDescription`, `Inputs` (list<Input>), `Outputs` (list<Output>), `CloudWatchLoggingOptions`, `ApplicationCode`, `Tags`. Must persist a new application record and return `ApplicationSummary` (required) with the stored `ApplicationName`, a generated `ApplicationARN` of the form `arn:aws:kinesisanalytics:<region>:<account>:application/<ApplicationName>`, and `ApplicationStatus` = `READY`. Creating a name that already exists must fail with `ResourceInUseException`.
2. **DescribeApplication** — input `ApplicationName`. Must return `ApplicationDetail` (required) read back from the stored record: `ApplicationName`, `ApplicationARN`, `ApplicationStatus`, `ApplicationVersionId` (starts at 1), `CreateTimestamp`, `LastUpdateTimestamp`, `ApplicationDescription`, `ApplicationCode`, and `InputDescriptions` / `OutputDescriptions` / `CloudWatchLoggingOptionDescriptions` derived from what CreateApplication stored (each description carrying the server-assigned `InputId` / `OutputId` / `CloudWatchLoggingOptionId`). Unknown name must fail with `ResourceNotFoundException`.
3. **ListApplications** — input `Limit` (1..50), `ExclusiveStartApplicationName`. Must return `ApplicationSummaries` (required) built from the store, sorted by application name, honouring `Limit` and starting strictly after `ExclusiveStartApplicationName`, and set `HasMoreApplications` (required) to true only when applications remain beyond the page.
4. **StartApplication** — input `ApplicationName`, `InputConfigurations` (required). Must transition the stored status `READY` -> `RUNNING` (via/settling on `RUNNING`) so a following DescribeApplication observes it; starting an application that is already `RUNNING` must fail with `ResourceInUseException`, and an unknown name with `ResourceNotFoundException`.
5. **StopApplication** — input `ApplicationName`. Must transition the stored status `RUNNING` -> `READY`, observable via DescribeApplication; unknown name -> `ResourceNotFoundException`.
6. **DeleteApplication** — input `ApplicationName`, `CreateTimestamp` (required). Must remove the record only when the supplied `CreateTimestamp` matches the stored one (mismatch -> `ResourceInUseException`/`ConcurrentModificationException`); after deletion DescribeApplication must return `ResourceNotFoundException` and the application must be absent from ListApplications.

Every one of these must compute its response from persisted state — no canned, hardcoded or echoed responses, and no generating an ARN/ID without storing it. `ApplicationVersionId` must be a real counter on the stored record (incremented by configuration-changing operations such as UpdateApplication/AddApplication* when those are implemented later), not a constant.

## State Model

In `src/state.rs` (`ComAmazonawsKinesisanalyticsKinesisanalytics20150814State`), reached through `crate::state::service_state(&*self.engine)`:

- `applications: BTreeMap<String /*ApplicationName*/, Application>` — BTreeMap so ListApplications pagination by name is deterministic.
- `Application { name: String, arn: String, description: Option<String>, status: ApplicationStatus /* DELETING|STARTING|STOPPING|READY|RUNNING|UPDATING */, version_id: i64 /*starts 1*/, create_timestamp: DateTime, last_update_timestamp: DateTime, application_code: Option<String>, inputs: Vec<InputRecord>, outputs: Vec<OutputRecord>, reference_data_sources: Vec<ReferenceDataSourceRecord>, cloudwatch_logging_options: Vec<CloudWatchLoggingOptionRecord>, tags: BTreeMap<String, String>, next_id: u64 /*source of InputId/OutputId/ReferenceId/CloudWatchLoggingOptionId*/ }`
- `InputRecord { input_id: String, name_prefix: String, kinesis_streams_input: Option<{resource_arn, role_arn}>, kinesis_firehose_input: Option<{resource_arn, role_arn}>, input_schema: SourceSchema, input_parallelism_count: i32, input_processing_configuration: Option<...> }`
- `OutputRecord { output_id: String, name: String, destination: KinesisStream|KinesisFirehose|LambdaDestination with {resource_arn, role_arn}, destination_schema: {record_format_type} }`
- `CloudWatchLoggingOptionRecord { cloud_watch_logging_option_id: String, log_stream_arn: String, role_arn: String }`
- `ReferenceDataSourceRecord { reference_id: String, table_name: String, s3_reference: Option<{bucket_arn, file_key, reference_role_arn}>, reference_schema: SourceSchema }`

All other operations (AddApplication*/DeleteApplication* sub-resource ops, UpdateApplication, DiscoverInputSchema, TagResource/UntagResource/ListTagsForResource) stay stubbed at 501 for now and will be filed as individual gap issues; the `tags` and sub-resource vectors above exist so that slice can be added without reshaping the store.

## Evidence

`services/service-com-amazonaws-kinesisanalytics-kinesisanalytics-20150814/OPERATIONS.md` lists all 20 operations with their input/output members and errors (CreateApplication -> required `ApplicationSummary`; DescribeApplication -> required `ApplicationDetail`; ListApplications -> required `ApplicationSummaries` + `HasMoreApplications` with `Limit`/`ExclusiveStartApplicationName`; StartApplication requires `InputConfigurations`; DeleteApplication requires `CreateTimestamp`). `smithy_query shape=ApplicationDetail,ApplicationSummary,ApplicationStatus` confirms `ApplicationARN`, `ApplicationStatus` and `ApplicationVersionId` (range 1..999999999) are @required output fields and that the status enum is DELETING|STARTING|STOPPING|READY|RUNNING|UPDATING. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 20 handler files, including `create_application.rs`, `describe_application.rs`, `list_applications.rs`, `start_application.rs`, `stop_application.rs`, `delete_application.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501). No tests exercise these ops (only the generated `tests/state_roundtrip.rs`). Wire protocol per README/OPERATIONS.md: awsJson1_1, `POST /` with `X-Amz-Target: KinesisAnalytics_20150814.<Operation>`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI kinesisanalytics client at it.
2. CreateApplication with ApplicationName="demo-app", ApplicationCode="SELECT * FROM SOURCE_SQL_STREAM_001;" and one Kinesis streams Input -> expect 200 with ApplicationSummary{ApplicationName="demo-app", ApplicationARN=arn:aws:kinesisanalytics:...:application/demo-app, ApplicationStatus=READY}.
3. CreateApplication again with the same name -> expect ResourceInUseException, not a second record.
4. DescribeApplication ApplicationName="demo-app" -> expect 200 with ApplicationDetail echoing the stored description/code, ApplicationVersionId=1, a CreateTimestamp, and an InputDescription carrying a server-assigned InputId.
5. ListApplications -> expect ApplicationSummaries containing "demo-app" and HasMoreApplications=false; ListApplications with Limit=1 and two applications created -> expect one summary and HasMoreApplications=true, then pass ExclusiveStartApplicationName=<first name> to get the second.
6. StartApplication ApplicationName="demo-app" with InputConfigurations=[{Id=<InputId from describe>, InputStartingPositionConfiguration={InputStartingPosition=NOW}}] -> expect 200, then DescribeApplication shows ApplicationStatus=RUNNING.
7. StartApplication again while RUNNING -> expect ResourceInUseException.
8. StopApplication ApplicationName="demo-app" -> expect 200, then DescribeApplication shows ApplicationStatus=READY.
9. DeleteApplication ApplicationName="demo-app" with the CreateTimestamp returned by DescribeApplication -> expect 200; DescribeApplication then returns ResourceNotFoundException and ListApplications no longer lists it.
10. DescribeApplication/StartApplication/StopApplication on a never-created name -> expect ResourceNotFoundException (HTTP 400 awsJson1_1 error shape), not 501.
