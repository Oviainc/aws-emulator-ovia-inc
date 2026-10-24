Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# CreateEnvironment does not apply MWAA's service-assigned defaults, so GetEnvironment omits members AWS always reports

_Filed: 2026-10-24T00:24:30Z_
_Operation: CreateEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A create that omits optional members stores `None` for them, and the environment record therefore reports nothing for members the real service always fills in with a service-assigned default. Every later read (`GetEnvironment`, and the environment a workflow inspects after create) is missing fields a client reads unconditionally.

Expected (real MWAA, environment created with only the required members): GetEnvironment returns at least
- `EnvironmentClass`: "mw1.small" (the default class)
- `MinWorkers`: 1 and `MaxWorkers`: 10 (the service defaults)
- `WebserverAccessMode`: "PUBLIC_ONLY"
- `EndpointManagement`: "SERVICE" (the member's own doc describes SERVICE vs CUSTOMER as the two managed modes; MWAA manages the endpoints unless the caller asks for CUSTOMER)
- `WeeklyMaintenanceWindowStart`: a window MWAA assigns (e.g. "SAT:03:30") when the caller named none
- `LoggingConfiguration`: all five module blocks (`DagProcessingLogs`, `SchedulerLogs`, `TaskLogs`, `WebserverLogs`, `WorkerLogs`) present with `Enabled: false` and the default `LogLevel: "INFO"`, no `CloudWatchLogGroupArn`.

Actual (emulator): all of the above are absent from the GetEnvironment payload. `EnvironmentClass`, `MinWorkers`, `MaxWorkers`, `WebserverAccessMode`, `EndpointManagement` and `WeeklyMaintenanceWindowStart` are stored as `Option::None` and serialized as missing; `LoggingConfiguration` is omitted entirely when the create configured no log type (`LoggingConfigurationRecord::is_empty` -> `to_modelled` returns None). Only `Schedulers`, `MinWebservers` and `MaxWebservers` get their documented defaults.

## Evidence

Spec: the Environment shape models EnvironmentClass ("Valid values: mw1.micro, mw1.small, …"), MinWorkers/MaxWorkers, WebserverAccessMode, EndpointManagement, WeeklyMaintenanceWindowStart and LoggingConfiguration as members of the environment description, and the Schedulers/MinWebservers/MaxWebservers docs show MWAA assigns defaults for unspecified sizing members — the handler already honours exactly those three. Handler: src/ops/create_environment.rs:290-341 copies `input.environment_class`, `min_workers`, `max_workers`, `webserver_access_mode`, `weekly_maintenance_window_start`, `endpoint_management` straight through as Options with no default, and builds `LoggingConfigurationRecord::default()` (all five log types None) when the request names no LoggingConfiguration; src/state.rs `to_environment` then omits each absent member. Test coverage asserts the DIVERGENT behavior: tests/all/environment_lifecycle.rs:473-489 expects `Value::Null` for EnvironmentClass / WebserverAccessMode / EndpointManagement / WeeklyMaintenanceWindowStart / MinWorkers / MaxWorkers when the create omitted them, and :530-539 requires `LoggingConfiguration` to be absent when no log type was configured.

## Steps to Reproduce

1. Boot the emulator and point a botocore mwaa client at it.
2. mwaa.create_environment(Name='DefaultsEnv', ExecutionRoleArn='arn:aws:iam::123456789012:role/mwaa-exec', SourceBucketArn='arn:aws:s3:::my-airflow-bucket-unique-name', DagS3Path='dags', NetworkConfiguration={'SubnetIds': ['subnet-00000000000000001','subnet-00000000000000002'], 'SecurityGroupIds': ['sg-00000000000000001']}) -> 200.
3. mwaa.get_environment(Name='DefaultsEnv') and inspect the Environment payload.
4. Expected: EnvironmentClass='mw1.small', MinWorkers=1, MaxWorkers=10, WebserverAccessMode='PUBLIC_ONLY', EndpointManagement='SERVICE', a WeeklyMaintenanceWindowStart value, and a LoggingConfiguration carrying all five module blocks with Enabled=false / LogLevel='INFO'.
5. Observed: every one of those keys is absent from the response.
