Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes__sqs.pipes-trace-log-configuration-records-the-message-payload
Issue-Key: pipes__sqs.pipes-trace-log-configuration-records-the-message-payload|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# pipes: a pipe's LogConfiguration is stored and echoed but never produces execution log records — a Level=TRACE, IncludeExecutionData=ALL CloudWatch Logs destination stays empty

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
This bundle has 1 cause; it must be closed before submitting.

Cause 1 of 1: a pipe created with LogConfiguration={CloudwatchLogsLogDestination:{LogGroupArn}, Level:"TRACE", IncludeExecutionData:["ALL"]} accepts and echoes that configuration, and (now that the data plane exists) moves the record from its SQS source to its SQS target, but it never writes a single execution log record to the configured CloudWatch Logs log group, so no record carrying the source payload, no executionId, no resourceArn and no logLevel=TRACE line ever appears. Failing check: pipes__sqs.pipes-trace-log-configuration-records-the-message-payload.

AWS: Smithy (com.amazonaws.pipes): CreatePipe/UpdatePipe take LogConfiguration -> PipeLogConfigurationParameters with a REQUIRED Level member targeting LogLevel (values OFF | ERROR | INFO | TRACE), an IncludeExecutionData list of IncludeExecutionDataOption (value ALL) and CloudwatchLogsLogDestinationParameters.LogGroupArn (required, @length 1..1600, pattern ^arn:aws...:logs:<region>:<account>:log-group:<name>(:\*)?$); DescribePipe reports it back as PipeLogConfiguration. IncludeExecutionData is documented as "Specify ALL to include the execution data (specifically, the payload, awsRequest, and awsResponse fields) in the log messages for this pipe" and Level as "The level of logging detail to include. This applies to all log destinations for the pipe." Those members exist precisely because EventBridge Pipes EMITS execution log records to the configured destination as it runs a pipe: the pipe's own execution log, one JSON record per execution stage, written into log streams of the named group. Record field names beyond that (executionId, resourceArn, logLevel, payload) are recorded behavior only — the replayed check reads them back and the model names payload/awsRequest/awsResponse as the execution-data fields — so the requirement is: a Level=TRACE pipe writes, for each execution, JSON records into the configured log group in which resourceArn is the executing pipe's ARN, executionId is one id shared by all records of that execution, logLevel is the record's level with at least one TRACE record present, and (because IncludeExecutionData contains ALL) the source record's payload appears in the record's own `payload` field — never a record for an execution that did not happen. No HTTP status or error code is involved: the divergence is an absent side effect, and the check fails as a wait that never completes ("Check timed out after 60.0s").
Emulator: nothing in the pipes crate ever calls CloudWatch Logs. services/service-com-amazonaws-pipes-pipes/src/ops/create_pipe.rs:329 stores the configuration (log_configuration_of, lines 361-389 — a pure write-shape -> read-shape conversion so DescribePipe can echo it) and nothing else consumes it; the data plane's own contract enumerates every call it makes and logs is not one of them — services/service-com-amazonaws-pipes-pipes/src/state/data_plane.rs:94-114 says "the pipe reaches the sqs, dynamodbstreams, lambda and states surfaces of this same emulator and nothing else" and names only CloudWatch METRICS (src/state/metrics.rs) as what a pass publishes. Consequently filter_log_events on the configured group returns no event matching the delivered payload, the check's poll for "an execution log record carrying the payload ..." never succeeds, and the check hangs to the harness cap.
Why: a client that turns pipe logging on gets a configuration that reads back perfectly and a log group that stays permanently empty. Pipe logging is the only per-execution observability EventBridge Pipes offers, so an operator cannot see which records entered a pipe, which were dropped by FilterCriteria, what the enrichment was handed or returned, or why a target stage failed; log-based assertions, log-subscription alarms and log-driven debugging of a pipe all silently observe nothing while the pipe reports healthy. It also makes a misconfigured destination (a group the role cannot write) indistinguishable from a correctly configured one.
Fix: an execution-logging stage in the pipes data plane — services/service-com-amazonaws-pipes-pipes/src/state/data_plane.rs alongside the existing metrics publication (src/state/metrics.rs), reading the stored PipeLogConfiguration from the pipe record (written in src/ops/create_pipe.rs and the UpdatePipe handler): per execution, mint one execution id, emit the stage records the configured Level admits (TRACE emitting the finest-grained records, ERROR only failures, OFF nothing) to the configured destination via the emulator's cloudwatchlogs surface (CreateLogStream/PutLogEvents, made as the pipe's execution role like every other data-plane call), and include the source record in each record's payload field only when IncludeExecutionData contains ALL.
Traces: pipes__sqs.pipes-trace-log-configuration-records-the-message-payload.

Related but NOT covering this: 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md (its Cause 5 is CloudWatch METRICS emission, a different destination and a different record shape, and its Cause 1 — no data plane at all — is no longer the symptom here since 74/87 pipes checks now pass). No open issue names pipe log configuration or CloudWatch Logs emission.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): Total 85/140 passed (60.7%); Mismatched 42, Errored 13, Skipped 0, Unloadable 0. No bootstrap signal — the emulator was serving: pipes 74/87 passed, pipes__sqs 6/9, and in this very check CreatePipe, DescribePipe, SQS CreateQueue/SendMessage/ReceiveMessage, IAM CreateRole and logs CreateLogGroup/DescribeLogGroups all answered 200.

Failing check: [mismatch] pipes__sqs.pipes-trace-log-configuration-records-the-message-payload
  Error: Check timed out after 60.0s

What the check does and where it stops:
  1. creates source and target SQS queues and a log group /aws/vendedlogs/pipes/<name>; DescribeLogGroups answers it (so the logs surface itself works).
  2. creates a pipes execution role granting sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source, sqs:SendMessage on the target, and logs:CreateLogStream + logs:PutLogEvents on the group ARN and <group ARN>:*.
  3. CreatePipe(..., SourceParameters={SqsQueueParameters:{BatchSize:1}}, LogConfiguration={CloudwatchLogsLogDestination:{LogGroupArn:<group arn>}, Level:"TRACE", IncludeExecutionData:["ALL"]}, DesiredState=RUNNING) and waits for CurrentState=RUNNING — accepted.
  4. SendMessage a JSON body carrying a unique marker; the record is received at the target.
  5. polls logs.filter_log_events(logGroupName=<group>, filterPattern='"<marker>"').
  Expected: at least one event whose message is a JSON execution record with marker inside its `payload` field, `resourceArn` containing the pipe, and an `executionId`; then filter_log_events on that executionId returns records of that execution only, including at least one with logLevel == "TRACE", every one naming the pipe in resourceArn; and a marker never sent through the pipe matches no event.
  Actual: filter_log_events returns {"events": []} for the duration of the poll — the log group is empty — so the wait described as "an execution log record carrying the payload '<marker>' in '<group>'" never completes and the check is cut at the harness cap.

Code corroboration (no CloudWatch Logs call path exists):
  services/service-com-amazonaws-pipes-pipes/src/ops/create_pipe.rs:329 — log_configuration: input.log_configuration.as_ref().map(log_configuration_of) (store-and-echo only).
  services/service-com-amazonaws-pipes-pipes/src/ops/create_pipe.rs:361-389 — log_configuration_of maps the write shape to the read shape; nothing consumes the stored value.
  services/service-com-amazonaws-pipes-pipes/src/state/data_plane.rs:94-97 — "the pipe reaches the sqs, dynamodbstreams, lambda and states surfaces of this same emulator and nothing else".
  services/service-com-amazonaws-pipes-pipes/src/state/data_plane.rs:111-114 — the only publication a pass makes is the pipe's CloudWatch METRICS (super::metrics).

Generated pipe/queue/role/log-group names, the marker nonce, ARNs, account id, region, hosts, ports, timestamps, execution ids and request ids in this check differ per run and are examples only. The requirement is the behavior: a RUNNING pipe whose LogConfiguration names a CloudWatch Logs destination at Level=TRACE with IncludeExecutionData=[ALL] writes per-execution JSON records into that group, carrying that execution's id, the executing pipe's ARN, the record's log level (with TRACE records present) and the source record in the record's payload field, and writes no record for an execution that never happened.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite); the check pipes__sqs.pipes-trace-log-configuration-records-the-message-payload fails with "Check timed out after 60.0s".
2. By hand: create two SQS queues (source with VisibilityTimeout=60, and a target) and note their ARNs.
3. logs CreateLogGroup logGroupName=/aws/vendedlogs/pipes/<name>; DescribeLogGroups logGroupNamePrefix=<that name> and take the group ARN with any trailing ':*' stripped.
4. iam CreateRole trusted by pipes.amazonaws.com with an inline policy allowing sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source, sqs:SendMessage on the target, and logs:CreateLogStream + logs:PutLogEvents on <group arn> and <group arn>:*.
5. pipes CreatePipe(Name=<p>, RoleArn=<role>, Source=<source arn>, SourceParameters={SqsQueueParameters:{BatchSize:1}}, Target=<target arn>, DesiredState=RUNNING, LogConfiguration={CloudwatchLogsLogDestination:{LogGroupArn:<group arn>}, Level:"TRACE", IncludeExecutionData:["ALL"]}); poll DescribePipe until CurrentState=RUNNING (and observe LogConfiguration echoed back verbatim).
6. sqs SendMessage to the source with a body containing a unique marker; ReceiveMessage on the target and confirm the record was delivered.
7. logs FilterLogEvents(logGroupName=<group>, filterPattern='"<marker>"') and logs DescribeLogStreams(logGroupName=<group>). Observe no streams and no events: the pipe produced no execution log record at all. Expected: at least one JSON record whose payload field contains the marker, whose resourceArn names the pipe and which shares an executionId with the execution's other records, at least one of them logLevel=TRACE.
8. After fixing, re-replay pipes__sqs.pipes-trace-log-configuration-records-the-message-payload.
