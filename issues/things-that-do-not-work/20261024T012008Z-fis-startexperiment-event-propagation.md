Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Experiment logging is never delivered: no log records reach the configured CloudWatch Logs group or S3 bucket while an experiment runs

_Filed: 2026-10-24T01:20:07Z_
_Operation: StartExperiment | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A template may carry `logConfiguration` naming a CloudWatch Logs log group (`cloudWatchLogsConfiguration.logGroupArn`) and/or an S3 destination (`s3Configuration.bucketName`/`prefix`) plus a `logSchemaVersion`. Real FIS, for every experiment started from such a template, DELIVERS experiment log records to that destination for the duration of the run (experiment start/stop, target resolution, per-action start/end events).

The emulator stores the configuration, copies it onto the experiment and reports it back, but no log record is ever produced: nothing in the start path or in `ExperimentRecord::settle` writes to the CloudWatch Logs or S3 store. A caller that starts an experiment and then calls logs:GetLogEvents / FilterLogEvents on the configured log group (or s3:ListObjectsV2 on the configured bucket/prefix) finds nothing, so any workflow that verifies an experiment by its logs, or that trips a metric filter / subscription on them, is unobservable against the emulator. The same holds for the report artifact a configured `experimentReportConfiguration.outputs.s3Configuration` would have received.

Expected: starting an experiment whose template configures logging results in log events in the named log group (and/or objects under the named S3 bucket/prefix) that a later read of those services reports. Actual: the destinations stay empty; the configuration is a pure echo.

## Evidence

Smithy model: CreateExperimentTemplate.logConfiguration -> CreateExperimentTemplateLogConfigurationInput {cloudWatchLogsConfiguration, s3Configuration, logSchemaVersion (required)}; Experiment.logConfiguration -> ExperimentLogConfiguration, i.e. the destination is part of the experiment the service runs. Well-established FIS behavior (FIS User Guide, "Experiment logging"): FIS sends detailed experiment logs to the configured CloudWatch Logs log group or S3 bucket while the experiment runs.
Handler/state: src/ops/create_experiment_template.rs persists the configuration (state.rs `template_log_configuration`); src/ops/start_experiment.rs copies it onto the record (state.rs `from_template` lines 904-907 via `experiment_log_configuration`) and nothing else touches it — the only cross-service store START reads is the IAM one (start_experiment.rs lines 236-252), and `settle` (state.rs ~941) only moves the status/endTime. No writer for logs or S3 exists in the crate.
Tests: tests/all/experiment_lifecycle.rs only asserts the logConfiguration round-trips on create/get; no test in tests/all/ observes a log destination.

## Steps to Reproduce

1. Boot the emulator and point botocore `fis`, `logs` and `s3` clients at it.
2. logs.create_log_group(logGroupName='/aws/fis/experiments') and s3.create_bucket(Bucket='fis-logs').
3. create_experiment_template with logConfiguration={'logSchemaVersion': 2, 'cloudWatchLogsConfiguration': {'logGroupArn': '<arn of that group>:*'}, 's3Configuration': {'bucketName': 'fis-logs', 'prefix': 'experiments'}} and one action with a duration.
4. start_experiment from that template and poll get_experiment until it is terminal.
5. logs.filter_log_events(logGroupName='/aws/fis/experiments') and s3.list_objects_v2(Bucket='fis-logs', Prefix='experiments') — observe zero events and zero objects, although the experiment reports the logConfiguration it ran with.
