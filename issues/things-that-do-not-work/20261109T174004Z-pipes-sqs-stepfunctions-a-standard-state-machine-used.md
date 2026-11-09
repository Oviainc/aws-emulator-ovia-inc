Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes__sqs__stepfunctions.pipes-enrichment-standard-state-machine-rejected-express-accepted
Issue-Key: pipes__sqs__stepfunctions.pipes-enrichment-standard-state-machine-rejected-express-accepted|_poll_until#31|TimeoutError|raise TimeoutError(f"{description} not met within {timeout_s:.0f}s. Last result: {last!r}")
Issue-Commit: 6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4

# pipes__sqs__stepfunctions: a STANDARD state machine used as Enrichment produces no ERROR-level EnrichmentInvocationFailed execution log record

> _Note: this issue was identified against the code base at commit `6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Cause 1 of 1: the enrichment failure a STANDARD state machine causes is never surfaced in the pipe's execution log.
AWS: Pipes enriches via StartSyncExecution, which only a STANDARD-type state machine cannot serve. CreatePipe accepts a STANDARD state machine as Enrichment and the pipe reaches RUNNING, but every record then fails enrichment and never reaches the target, while the identical wiring against an EXPRESS twin delivers. With LogConfiguration {CloudwatchLogsLogDestination.LogGroupArn, Level=ERROR, IncludeExecutionData=[ALL]}, the pipe writes one ERROR-level execution log record per failed record into the configured log group with messageType "EnrichmentInvocationFailed" and an `error` object carrying awsService "states", httpStatusCode 400, exceptionType "BadRequest", resourceArn = the STANDARD state machine's ARN, a Step Functions requestId, and message of the fixed form "Enrichment invocation failed with error from States: This operation is not supported by this type of state machine (Service: Sfn, Status Code: 400, Request ID: <requestId>) (SDK Attempt Count: 1)" — only the requestId (and the resourceArn) are per-run values; the rest of that text is fixed for this error class.
Emulator: FilterLogEvents on the configured log group finds no record at all for the sent marker within 150 s (last result []). The EXPRESS control leg of the same check delivered, so the pipe is live — the STANDARD enrichment's refusal is simply never detected or logged: no EnrichmentInvocationFailed record, hence no error attribution to `states`, no 400/BadRequest classification and no state-machine ARN.
Why: this is the only signal a client has that its enrichment is failing. With it absent, a pipe that silently discards every record looks indistinguishable from an idle one: ERROR-level logging is configured precisely so operators can alarm on it, and a client debugging "records never arrive" gets no attribution to Step Functions or to the state-machine type mistake.
Fix: the Pipes runtime's enrichment stage must classify a Step Functions StartSyncExecution refusal against a STANDARD state machine as an enrichment failure, and the LogConfiguration log-writer must emit the EnrichmentInvocationFailed record (with the error object's awsService/httpStatusCode/exceptionType/resourceArn/requestId/message) to the CloudWatch Logs destination. Open issues 20261107T175408Z-pipes-sqs-a-pipe-s-logconfiguration-level-trace.md and 20261104T050114Z-pipes-sqs-pipes-a-pipe-s-logconfiguration-is-stored-and.md describe TRACE-level execution logging never being written and do not carry this ERROR-level enrichment-failure replay key.
Traces: pipes__sqs__stepfunctions.pipes-enrichment-standard-state-machine-rejected-express-accepted.

## Evidence
TimeoutError: an ERROR-level execution log record carrying 'sim2real-xsvc-standard-mark-<id>' in '/aws/vendedlogs/pipes/<log group>' not met within 45s. Last result: []. Expected the first matching event's message to parse to messageType='EnrichmentInvocationFailed' with error.awsService='states', error.httpStatusCode=400, error.exceptionType='BadRequest', error.resourceArn=<STANDARD state machine ARN>, a non-empty error.requestId and error.message 'Enrichment invocation failed with error from States: This operation is not supported by this type of state machine (Service: Sfn, Status Code: 400, Request ID: <requestId>) (SDK Attempt Count: 1)'. The EXPRESS control pipe in the same check delivered its record to the target.

## Steps to Reproduce
1. Create two SQS source queues, one SQS target, a STANDARD and an EXPRESS state machine, and a CloudWatch Logs group /aws/vendedlogs/pipes/<name>; create a pipe role granting sqs access, states:StartExecution/StartSyncExecution and logs:CreateLogStream/PutLogEvents.
2. CreatePipe with Enrichment=<EXPRESS ARN> (control) — send a message and confirm delivery to the target.
3. CreatePipe with Enrichment=<STANDARD ARN> and LogConfiguration {CloudwatchLogsLogDestination: {LogGroupArn}, Level: ERROR, IncludeExecutionData: [ALL]}; confirm DescribePipe echoes the enrichment and CurrentState=RUNNING.
4. SendMessage with a marker to the STANDARD pipe's source queue, then FilterLogEvents(logGroupName=<group>, filterPattern='"<marker>"') — no event ever appears.
