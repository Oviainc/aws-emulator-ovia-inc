Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__stepfunctions.deep-sfn-3-express-async-history-rejected-logs-carry-events logs__stepfunctions.xsvc-sfn-3-express-async-all-level-log-events-arrive-in-order-per-execution logs__stepfunctions.xsvc-sfn-3-express-error-level-logs-only-the-run-through-a-fail-state logs__stepfunctions.xsvc-sfn-3-express-execution-history-api-rejected-while-logs-carry-it logs__stepfunctions.xsvc-sfn-3-express-include-execution-data-false-logs-same-events-without-payload
Issue-Key: logs__stepfunctions.deep-sfn-3-express-async-history-rejected-logs-carry-events|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: logs__stepfunctions.xsvc-sfn-3-express-async-all-level-log-events-arrive-in-order-per-execution|_warm_up#5|Timeout|time.sleep(_POLL_S)
Issue-Key: logs__stepfunctions.xsvc-sfn-3-express-error-level-logs-only-the-run-through-a-fail-state|_warm_up#5|Timeout|time.sleep(_POLL_S)
Issue-Key: logs__stepfunctions.xsvc-sfn-3-express-execution-history-api-rejected-while-logs-carry-it|_warm_up#5|Timeout|time.sleep(_POLL_S)
Issue-Key: logs__stepfunctions.xsvc-sfn-3-express-include-execution-data-false-logs-same-events-without-payload|_warm_up#5|Timeout|time.sleep(_POLL_S)
Issue-Commit: c083bea380b29cc5238d586af0774e0d5864cbe3

# logs__stepfunctions: EXPRESS executions deliver no vended CloudWatch Logs events, so the only record of an EXPRESS run is empty and all five express-logging checks hang to the 60 s cap

> _Note: this issue was identified against the code base at commit `c083bea380b29cc5238d586af0774e0d5864cbe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: an EXPRESS state machine created with a loggingConfiguration naming a CloudWatch Logs vended log group never writes any log event (or log stream) to that group, so an asynchronously started EXPRESS execution leaves no observable record anywhere.

AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions): CreateStateMachine.type targets StateMachineType (STANDARD|EXPRESS) and CreateStateMachine.loggingConfiguration targets LoggingConfiguration{level: LogLevel [ALL, ERROR, FATAL, OFF], includeExecutionData, destinations ("Limited to size 1. Required, if your log level is not set to OFF")}; LogDestination.cloudWatchLogsLogGroup.logGroupArn "must end with :*". GetExecutionHistory models InvalidArn (400) among its errors, and for EXPRESS executions AWS refuses the history API — so CloudWatch Logs is the only record of an EXPRESS run. Beyond the model this is recorded behavior only: the recordings show that an async StartExecution on an EXPRESS machine returns only executionArn (containing ":express:") and startDate — no status, no output — and that the configured log group then receives one JSON event per history event, each carrying `type`, `id` and `execution_arn`, with the ids forming the contiguous sequence 1..N of that execution's history in history order (ExecutionStarted -> TaskStateEntered/state transitions -> ExecutionSucceeded, or ... FailStateEntered -> ExecutionFailed). level=ERROR delivers only the failure-tier events of a run that goes through a Fail state; includeExecutionData=false delivers the same event types with the input/output payload omitted. Delivery goes live shortly after the logging configuration is written, which is why each check first drives warm-up executions and waits for one to land.

Emulator: CreateStateMachine accepts type=EXPRESS together with the loggingConfiguration without error, but the named log group never receives a log stream or any log event for any EXPRESS execution. All five checks block in their warm-up/await-delivery barrier and are killed by the harness with the identical verdict `Check timed out after 60.0s`. Because the barrier precedes the graded assertions, the per-level selection, the ordering/id contiguity, the includeExecutionData redaction and the "GetExecutionHistory on an EXPRESS execution ARN is refused 400 InvalidArn" assertions were never exercised and remain ungraded.

Why: a client running EXPRESS workflows has no way to see what happened. DescribeExecution/GetExecutionHistory do not serve EXPRESS runs, so with vended logging inert there is no success/failure record, no input/output, and no error/cause at all — failures are invisible, and any consumer built on those log events (metric filters, subscription filters, Logs Insights) reports zero.

Fix: the Step Functions execution-logging path for EXPRESS workflows — the component that turns a state machine's stored loggingConfiguration into CloudWatch Logs delivery (log-stream creation plus a per-history-event PutLogEvents emitter, with the level filter and includeExecutionData redaction), in the sfn crate's execution engine/state; the CreateStateMachine validator already accepts the configuration.

Traces: the five failing checks listed under `### logs__stepfunctions` in trace_ids, each with detail `Check timed out after 60.0s`. This is the same defect as the one filed for the `stepfunctions` heading in this sweep (the standard-workflow log-delivery issue), observed here on EXPRESS machines where logs are the only record. No open issue covers Step Functions vended log delivery.

## Evidence
Expected (AWS, per the recordings): sfn.StartExecution on an EXPRESS machine with loggingConfiguration {level:ALL, includeExecutionData:true, destinations:[{cloudWatchLogsLogGroup:{logGroupArn:"<group>:*"}}]} returns {executionArn containing ":express:", startDate} only; the log group then holds that execution's events, e.g. {"type":"ExecutionStarted","id":1,"execution_arn":"<arn>",...}, {"type":"TaskStateEntered","id":2,...}, {"type":"ExecutionSucceeded",...} with ids 1..N and no gaps or duplicates; sfn.GetExecutionHistory on that same execution ARN is refused 400 InvalidArn.

Actual (emulator): the log group receives no log stream and no log events for any EXPRESS execution, so each check's delivery barrier never resolves and the harness reports `Check timed out after 60.0s` for all five checks (deep-sfn-3-express-async-history-rejected-logs-carry-events, xsvc-sfn-3-express-async-all-level-log-events-arrive-in-order-per-execution, xsvc-sfn-3-express-error-level-logs-only-the-run-through-a-fail-state, xsvc-sfn-3-express-execution-history-api-rejected-while-logs-carry-it, xsvc-sfn-3-express-include-execution-data-false-logs-same-events-without-payload).

## Steps to Reproduce
1. Create an IAM role trusted by states.amazonaws.com carrying the documented log-delivery actions (logs:CreateLogDelivery, logs:GetLogDelivery, logs:UpdateLogDelivery, logs:DeleteLogDelivery, logs:ListLogDeliveries, logs:PutResourcePolicy, logs:DescribeResourcePolicies, logs:DescribeLogGroups, logs:CreateLogStream, logs:PutLogEvents on "*").
2. logs.CreateLogGroup for a group named /aws/vendedlogs/states/<unique>.
3. sfn.CreateStateMachine with type="EXPRESS", a one-state Pass definition, the role above, and loggingConfiguration={level:"ALL", includeExecutionData:true, destinations:[{cloudWatchLogsLogGroup:{logGroupArn:"arn:aws:logs:<region>:<account>:log-group:<group>:*"}}]}.
4. sfn.StartExecution (async) with any JSON input; confirm the response carries only executionArn/startDate.
5. Poll logs.FilterLogEvents / logs.DescribeLogStreams on the group. Expected: ExecutionStarted .. ExecutionSucceeded events naming that execution ARN, ids 1..N. Observed: no streams, no events, indefinitely.
6. Also call sfn.GetExecutionHistory on that EXPRESS execution ARN — expected 400 InvalidArn (this assertion is currently never reached because step 5 never completes).
