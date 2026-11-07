Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes__sqs.pipes-trace-log-configuration-records-the-message-payload
Issue-Key: pipes__sqs.pipes-trace-log-configuration-records-the-message-payload|_poll_until#18|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: c98f49a3ab045fe250f88b9f8f7e4420d8205746

# pipes__sqs: a pipe's LogConfiguration (Level=TRACE, IncludeExecutionData=ALL) never writes execution log records to its CloudWatch Logs destination

> _Note: this issue was identified against the code base at commit `c98f49a3ab045fe250f88b9f8f7e4420d8205746`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
This issue carries 1 cause.

Cause 1 of 1: a pipe configured with a CloudWatch Logs log destination at Level=TRACE and IncludeExecutionData=ALL never emits any execution log record, so the payload of a record the pipe handled is never observable in the configured log group.
AWS: Smithy (services/service-com-amazonaws-pipes-pipes/smithy-model/com-amazonaws-pipes-pipes.json) models CreatePipe.LogConfiguration as PipeLogConfigurationParameters and DescribePipe.LogConfiguration as PipeLogConfiguration, whose members are CloudwatchLogsLogDestination, S3LogDestination, FirehoseLogDestination, Level (LogLevel) and IncludeExecutionData (a unique list). The model documents IncludeExecutionData as "Whether the execution data (specifically, the payload, awsRequest, and awsResponse fields) is included in the log messages for this pipe" and Level as "The level of logging detail to include ... applies to all log destinations for the pipe". Recorded behavior: with Level=TRACE and IncludeExecutionData=[ALL] against a CloudWatch Logs destination, the pipe writes per-execution log records into that log group as it handles each source record, and those records carry the message payload, so FilterLogEvents/GetLogEvents on the group returns an event containing the payload the pipe processed.
Emulator: the LogConfiguration is accepted on CreatePipe and echoed back by DescribePipe, but no log events are ever written to the configured log group. The poll returns the empty list for its whole window — "an execution log record carrying the payload '<marker>' in '/aws/vendedlogs/pipes/<log-group>' not met within 150.0s. Last result: []". The marker and log-group name are per-run values; the requirement is that an execution record carrying the handled payload appears in the configured group.
Why: a client that turns on pipe logging to debug or audit a pipe gets a configuration that reports itself as installed and produces nothing. There is no way to observe which records a pipe handled, what payload it saw, or where a record was dropped, and any monitoring or alarm built on the vended log group is permanently silent.
Fix: the pipes crate's pipe runtime logging component — it must, for a pipe with a CloudwatchLogsLogDestination, emit an execution log record per handled record at the configured Level and include the payload when IncludeExecutionData contains ALL. Note this cause may be the same defect as open issue 20261104T050114Z-pipes-sqs-pipes-a-pipe-s-logconfiguration-is-stored-and.md (LogConfiguration stored and echoed but never producing execution log records); this failing check is not carried by that issue, so it is filed here and the two should be reconciled.
Traces: pipes__sqs.pipes-trace-log-configuration-records-the-message-payload.

## Evidence
Expected: with LogConfiguration={CloudwatchLogsLogDestination:{LogGroupArn:<group>}, Level:"TRACE", IncludeExecutionData:["ALL"]} on a RUNNING SQS-sourced pipe, the configured CloudWatch Logs group receives execution log records as the pipe handles each source record, and at least one of them carries the message payload.
Actual: the log group receives no events at all for the whole poll window. Replay output: "AssertionError: an execution log record carrying the payload 'sim2real-xsvc-logmark-4bfe57d93919' in '/aws/vendedlogs/pipes/sim2real-xsvc-lg-d6cf93bf41ee' not met within 150.0s. Last result: []" (the payload marker and log-group name are per-run values).

## Steps to Reproduce
Create a CloudWatch Logs log group under /aws/vendedlogs/pipes/; create an SQS source queue and a target, and a pipe execution role granting the source reads, the target write and logs:CreateLogStream/logs:PutLogEvents on the group; CreatePipe with LogConfiguration={CloudwatchLogsLogDestination:{LogGroupArn:<group arn>}, Level:"TRACE", IncludeExecutionData:["ALL"]} and DesiredState=RUNNING; wait for DescribePipe CurrentState=RUNNING (DescribePipe echoes the LogConfiguration); SendMessage to the source queue with a distinctive body; poll FilterLogEvents on the log group for an event containing that body — no events are ever returned.
