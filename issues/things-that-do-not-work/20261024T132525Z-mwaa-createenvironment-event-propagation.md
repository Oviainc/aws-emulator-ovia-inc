Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Enabled Apache Airflow log types never create their CloudWatch Logs log groups

_Filed: 2026-10-24T13:25:25Z_
_Operation: CreateEnvironment | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (AWS): when a create (or update) enables an Apache Airflow log type, Amazon MWAA creates the log group that log type publishes to in the caller's account — `airflow-<EnvironmentName>-<LogType>` — and reports its ARN as `LoggingConfiguration.<Module>.CloudWatchLogGroupArn`. A client that enables TaskLogs and then calls CloudWatch Logs `DescribeLogGroups` (or `GetLogEvents` on the group named by the reported ARN) finds the group.

Actual (emulator): `CreateEnvironment` stores the logging settings and `GetEnvironment` synthesises a `CloudWatchLogGroupArn` string for every ENABLED log type (src/state.rs `ModuleLoggingRecord::to_modelled` -> `log_group_arn`), but nothing is ever created in the CloudWatch Logs emulator (services/service-com-amazonaws-cloudwatchlogs-logs-20140328, which is a separate service crate in this repo). A `DescribeLogGroups` / `DescribeLogStreams` call for the reported ARN answers as if the group does not exist, so the ARN MWAA reports points at nothing: any workflow that enables logging and then reads the log group diverges from AWS.

The same omission applies to an `UpdateEnvironment` that switches a log type on (it reuses the same record and the same ARN builder), and secondarily to `EndpointManagement: SERVICE`, where MWAA creates the environment's VPC interface endpoints in the customer VPC (observable through EC2 `DescribeVpcEndpoints`) while the emulator only synthesises `WebserverVpcEndpointService` / `DatabaseVpcEndpointService` name strings.

## Evidence

Spec: the `ModuleLoggingConfiguration.CloudWatchLogGroupArn` member is modeled and documented as "The Amazon Resource Name (ARN) for the CloudWatch Logs group where the Apache Airflow log type (e.g. DagProcessingLogs) is published", and the emulator's own comment states the rule it is relying on: "MWAA publishes a log group for a log type it was asked to publish" (src/state.rs lines 215-227). Well-established MWAA behavior: enabling an Airflow log type provisions the `airflow-<env>-<type>` CloudWatch Logs group in the customer account.
Handler: src/ops/create_environment.rs lines 361-367 only build a `LoggingConfigurationRecord` in this service's own store; src/state.rs `log_group_arn` (lines 760-763) formats an ARN with `engine::emulator_arn("logs", ...)` and nothing in the crate ever calls the CloudWatch Logs service state (grep for `logs` in src/ shows only ARN formatting).
Tests: tests/all/environment_lifecycle.rs `assert_logging` only checks that the ARN STRING is reported for enabled types; no test asserts the log group exists in CloudWatch Logs. A sibling precedent is already filed for the serverless variant (20261024T071757Z-mwaa-serverless-startworkflowrun-event-propagation), so this MWAA-side gap is net-new.

## Steps to Reproduce

1. Boot the emulator with both MWAA and CloudWatch Logs mounted.
2. CreateEnvironment(Name='logs-env', ExecutionRoleArn='arn:aws:iam::000000000000:role/mwaa-exec', SourceBucketArn='arn:aws:s3:::my-airflow-bucket', DagS3Path='dags', NetworkConfiguration={'SubnetIds': ['subnet-a','subnet-b'], 'SecurityGroupIds': ['sg-1']}, LoggingConfiguration={'TaskLogs': {'Enabled': True, 'LogLevel': 'INFO'}}).
3. GetEnvironment(Name='logs-env') and read LoggingConfiguration.TaskLogs.CloudWatchLogGroupArn — it names log group 'airflow-logs-env-Task'.
4. Call CloudWatch Logs DescribeLogGroups(logGroupNamePrefix='airflow-logs-env') (or GetLogEvents on that group).
5. Expect the group MWAA created to be listed; observe an empty result — the reported ARN is backed by nothing.
6. Repeat after UpdateEnvironment(Name='logs-env', LoggingConfiguration={'SchedulerLogs': {'Enabled': True, 'LogLevel': 'INFO'}}): the new ARN is reported but no group appears.
