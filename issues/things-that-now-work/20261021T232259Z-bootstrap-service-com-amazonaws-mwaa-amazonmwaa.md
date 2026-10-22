Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 96234e472a4dbed875261b6f3ae0a9ed15fa6e7c

# Bootstrap the MWAA (Amazon Managed Workflows for Apache Airflow) emulator

_Filed: 2026-10-21T23:22:59Z_
_Operation: service-com-amazonaws-mwaa-amazonmwaa | Class: missing_

> _Note: this issue was identified against the code base at commit `96234e472a4dbed875261b6f3ae0a9ed15fa6e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/` is an unmodified scaffold returning HTTP 501 `NotImplemented` and holds no state, so no MWAA client workflow works at all. Implement a SMALL first slice: the environment lifecycle of the service's primary (and only) resource, `Environment`, served over `restJson1` (sdkId `MWAA`, sigv4 signing name `airflow`).

### Happy path to implement (in order)

1. **CreateEnvironment** — `PUT /environments/{Name}` → 200. Required inputs: `Name` (httpLabel, 1..80, `^[a-zA-Z][0-9a-zA-Z-_]*$`), `ExecutionRoleArn`, `SourceBucketArn`, `DagS3Path`, `NetworkConfiguration` (SubnetIds + SecurityGroupIds). Optional inputs that MUST be persisted verbatim for later reads: `EnvironmentClass`, `AirflowVersion`, `MaxWorkers`, `MinWorkers`, `Schedulers`, `MinWebservers`, `MaxWebservers`, `WebserverAccessMode`, `WeeklyMaintenanceWindowStart`, `AirflowConfigurationOptions`, `LoggingConfiguration`, `KmsKey`, `Plugins*`/`Requirements*`/`StartupScript*` S3 path+version pairs, `EndpointManagement`, `Tags`. Output: `Arn`, which must be a generated, stored ARN matching `^arn:aws:airflow:<region>:<account>:environment/<Name>$`. Store the environment keyed by `Name`; set `Status = AVAILABLE` (synthesise `CreatedAt`, `WebserverUrl`, `ServiceRoleArn`, `CeleryExecutorQueue`, VPC endpoint service names from the stored name/region). Re-creating an existing name must not silently clobber state into an inconsistent record (the op is `@idempotent`).
2. **GetEnvironment** — `GET /environments/{Name}` → 200 with `Environment`. Must return the FULL persisted record (every field written by Create/Update, including `Tags`, `LoggingConfiguration` with per-logger `CloudWatchLogGroupArn`/`Enabled`/`LogLevel`, and `LastUpdate` after an update). Unknown name → `ResourceNotFoundException` (404), not 501 and not an empty 200.
3. **ListEnvironments** — `GET /environments` → 200 with required `Environments` (list of NAMES, not ARNs) plus `NextToken`. Must list exactly the names currently stored, in a stable (sorted) order, honour `MaxResults` and round-trip `NextToken` for paging; a deleted environment must disappear from the list.
4. **UpdateEnvironment** — `PATCH /environments/{Name}` → 200 with the stored `Arn`. Must merge only the members present in the request onto the persisted record (absent members unchanged), and record `LastUpdate{Status: SUCCESS, CreatedAt, Source}`. Unknown name → `ResourceNotFoundException` (404). A subsequent GetEnvironment must reflect the new values.
5. **DeleteEnvironment** — `DELETE /environments/{Name}` → 200, empty body. Must remove the environment (and its tags) from state so GetEnvironment returns `ResourceNotFoundException` (404) and ListEnvironments no longer contains the name. Unknown name → `ResourceNotFoundException` (404).
6. **ListTagsForResource / TagResource / UntagResource** — `GET|POST|DELETE /tags/{ResourceArn}`. Must resolve `ResourceArn` back to the stored environment (404 `ResourceNotFoundException` when it resolves to nothing) and read/write the SAME tag map that CreateEnvironment seeded and GetEnvironment returns. `UntagResource` removes the keys listed in the `tagKeys` query parameter.

Every one of these must compute its response from persisted state — no canned responses, no echoing the input back, no returning a random ARN that is never stored. `CreateEnvironment` followed by `GetEnvironment` must return exactly what was created.

### Out of scope for this first slice (stay 501, filed later as gap issues)
`CreateCliToken`, `CreateWebLoginToken`, `InvokeRestApi`, `PublishMetrics` (deprecated).

## State Model

- Service state (behind the existing `crate::state::service_state(...)` lock):
  - `environments: BTreeMap<String /*Name*/, EnvironmentRecord>` — BTreeMap so ListEnvironments has deterministic order and paging tokens are stable.
  - `arn_index: HashMap<String /*Arn*/, String /*Name*/>` — so the `/tags/{ResourceArn}` operations resolve an ARN to an environment.
- `EnvironmentRecord` fields (mirroring the modeled `Environment` shape): `name`, `arn`, `status: EnvironmentStatus` (`AVAILABLE` after create), `created_at: DateTime`, `webserver_url`, `execution_role_arn`, `service_role_arn`, `kms_key`, `airflow_version`, `source_bucket_arn`, `dag_s3_path`, `plugins_s3_path`, `plugins_s3_object_version`, `requirements_s3_path`, `requirements_s3_object_version`, `startup_script_s3_path`, `startup_script_s3_object_version`, `airflow_configuration_options: BTreeMap<String,String>`, `environment_class`, `min_workers`, `max_workers`, `schedulers`, `min_webservers`, `max_webservers`, `network_configuration: NetworkConfigurationRecord{subnet_ids: Vec<String>, security_group_ids: Vec<String>}`, `logging_configuration: LoggingConfigurationRecord{dag_processing_logs, scheduler_logs, task_logs, webserver_logs, worker_logs: ModuleLoggingRecord{enabled: bool, log_level: String, cloud_watch_log_group_arn: String}}`, `weekly_maintenance_window_start`, `webserver_access_mode`, `endpoint_management`, `celery_executor_queue`, `webserver_vpc_endpoint_service`, `database_vpc_endpoint_service`, `last_update: Option<LastUpdateRecord{status, created_at, source, error}>`, `tags: BTreeMap<String,String>`.

Done = a realistic integration script (create → get → list → update → get again → tag/list-tags/untag → delete → get returns 404) passes with correct, stateful response bodies, not merely 2xx.

## Evidence

`services/service-com-amazonaws-mwaa-amazonmwaa/OPERATIONS.md` enumerates all 12 operations with routes, required members, outputs and errors: CreateEnvironment `PUT /environments/{Name}` (required Name/ExecutionRoleArn/SourceBucketArn/DagS3Path/NetworkConfiguration, output Arn), GetEnvironment `GET /environments/{Name}` (output Environment, error ResourceNotFoundException 404), ListEnvironments `GET /environments` (@paginated inputToken=NextToken, outputToken=NextToken, items=Environments; required Environments list), UpdateEnvironment `PATCH /environments/{Name}` (output Arn, ResourceNotFoundException 404), DeleteEnvironment `DELETE /environments/{Name}` (no output members, ResourceNotFoundException 404), and the three `/tags/{ResourceArn}` operations with Tags map 1..50. The `Environment` shape queried from the Smithy model supplies the field list used in the State Model above (Name, Status, Arn, CreatedAt, WebserverUrl, ExecutionRoleArn, ServiceRoleArn, KmsKey, AirflowVersion, SourceBucketArn, Dag/Plugins/Requirements/StartupScript paths+versions, AirflowConfigurationOptions, EnvironmentClass, Min/MaxWorkers, NetworkConfiguration, LoggingConfiguration, LastUpdate, WeeklyMaintenanceWindowStart, Tags, WebserverAccessMode, Schedulers, Webserver/DatabaseVpcEndpointService, CeleryExecutorQueue, EndpointManagement, Min/MaxWebservers). `src/ops/create_environment.rs` is representative of the whole crate: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateEnvironment"))` — HTTP 501, zero state. No tests exist for any MWAA behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `mwaa` client at it.
2. `aws mwaa create-environment --name MyMWAAEnvironment --execution-role-arn arn:aws:iam::123456789012:role/my-execution-role --source-bucket-arn arn:aws:s3:::my-airflow-bucket-unique-name --dag-s3-path dags --network-configuration SubnetIds=subnet-aaa,subnet-bbb,SecurityGroupIds=sg-111 --environment-class mw1.small --max-workers 10 --tags Environment=Staging` — expect 200 with a stored `Arn`; today 501 NotImplemented.
3. `aws mwaa get-environment --name MyMWAAEnvironment` — expect 200 whose `Environment` echoes the persisted ExecutionRoleArn, SourceBucketArn, DagS3Path, NetworkConfiguration, EnvironmentClass, MaxWorkers, Tags, Status=AVAILABLE, CreatedAt and the same Arn.
4. `aws mwaa list-environments` — expect `Environments` to contain exactly `MyMWAAEnvironment`; repeat with `--max-results 1` and follow `NextToken` after creating a second environment.
5. `aws mwaa update-environment --name MyMWAAEnvironment --max-workers 25 --environment-class mw1.medium` — expect 200 with the same Arn, then get-environment shows MaxWorkers=25, EnvironmentClass=mw1.medium, all other fields unchanged, and a populated LastUpdate.
6. `aws mwaa tag-resource --resource-arn <Arn> --tags Team=Data` then `aws mwaa list-tags-for-resource --resource-arn <Arn>` — expect both `Environment=Staging` and `Team=Data`; `aws mwaa untag-resource --resource-arn <Arn> --tag-keys Team` removes only `Team`.
7. `aws mwaa delete-environment --name MyMWAAEnvironment` — expect 200; then `get-environment` returns ResourceNotFoundException (404) and `list-environments` returns an empty `Environments` list.
8. `aws mwaa get-environment --name NoSuchEnv` — expect ResourceNotFoundException (404), not 501.
