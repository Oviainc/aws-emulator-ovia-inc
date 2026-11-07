Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.eventbridge-fanout-both-targets-denied-no-delivery
Issue-Key: events__lambda__sns.eventbridge-fanout-both-targets-denied-no-delivery|_wait_for_matching_message#24|Timeout|resp = sqs.receive_message(
Issue-Commit: 323c298260f641866db820e53cda367b812962dd

# events__lambda__sns: an asynchronous Invoke's stdout never reaches /aws/lambda/<fn>, so the EventBridge fan-out negative-delivery check never reaches a verdict (1 cause)

> _Note: this issue was identified against the code base at commit `323c298260f641866db820e53cda367b812962dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue contains 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: an asynchronous Invoke (InvocationType=Event) runs the handler but nothing the handler writes to stdout ever becomes readable in the function's own CloudWatch Logs group, so FilterLogEvents on /aws/lambda/<function-name> never returns the invocation's output and any check that uses "invoke, then look for a printed marker in the log group" as its positive control hangs until its budget expires. Failing check: events__lambda__sns.eventbridge-fanout-both-targets-denied-no-delivery.

AWS: Smithy (lambda, com-amazonaws-lambda-awsgirapiservice): Invoke POST /2015-03-31/functions/{FunctionName}/invocations, InvocationType carried in the X-Amz-Invocation-Type header; with InvocationType=Event the call is accepted (202, empty payload) and the function runs asynchronously. The Lambda runtime then writes that run's log to the function's log group /aws/lambda/<function-name> — the group and a per-execution-environment stream are created on first use when the execution role allows logs:CreateLogGroup/CreateLogStream/PutLogEvents (the check attaches the managed AWSLambdaBasicExecutionRole), and the run contributes a `START RequestId: <id> Version: <version>` line, every line the handler printed, an `END RequestId: <id>` line and a `REPORT RequestId: <id>\tDuration: … ms\tBilled Duration: … ms\tMemory Size: … MB\tMax Memory Used: … MB` line. The fixed text is the `START`/`END`/`REPORT RequestId:` prefixes and the `Duration`/`Billed Duration`/`Memory Size`/`Max Memory Used` field names; the request id, durations and sizes are per-invocation values and are not part of the requirement. A FilterLogEvents on that group whose filterPattern quotes a string the handler printed therefore returns at least one matching event within seconds. Recorded behavior: the asynchronous Invoke is accepted and the printed marker becomes readable through FilterLogEvents shortly afterwards.

Emulator: the asynchronous Invoke is accepted, but polling logs.filter_log_events(logGroupName='/aws/lambda/<fn>', filterPattern='"<marker>"') returns no events for the whole poll window — the group is either absent (ResourceNotFoundException, which the check swallows) or empty — so the check's own positive-control assertion ("positive control direct invoke was never observed in logs") can never be satisfied and the suite reports `Error: Check timed out after 60.0s` with no AWS call having failed outright.

Why: a client relying on AWS's behavior cannot observe an asynchronous invocation at all: `aws logs tail /aws/lambda/<fn>`, metric filters over handler output, log-driven alarms and every "invoke then read the log" diagnosis loop see nothing. Worse, for permission-boundary scenarios like this one the log group is the only evidence that a target was NOT invoked; with the group permanently empty, "EventBridge correctly refused to invoke the un-permitted Lambda target" is indistinguishable from "the runtime simply never records invocations", so negative-delivery conclusions drawn against this emulator are unsound.

Fix: the Lambda Invoke execution path's log emission and its asynchronous (Event) branch — services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs together with the execution state that captures the run's output — must write each run's captured stdout, framed by the START/END/REPORT lines, through to the CloudWatch Logs state store under /aws/lambda/<function-name>, creating the group and stream on first use, for asynchronous invocations as well as RequestResponse ones. The open issue 20261105T081727Z-events-lambda-sns-an-invoked-function-writes-nothing.md describes the same absence for a synchronous Invoke's REPORT line and does not carry this check's replay key; closing that one does not close this check, which exercises the InvocationType=Event path and the handler's own printed line.

Traces: events__lambda__sns.eventbridge-fanout-both-targets-denied-no-delivery (the fan-out both-targets-denied check; its first blocking convergence is the asynchronous-invoke log-marker positive control).

## Evidence
Cause 1 — events__lambda__sns.eventbridge-fanout-both-targets-denied-no-delivery: suite verdict `[mismatch] ... Error: Check timed out after 60.0s`.

The check creates a role trusting lambda.amazonaws.com with AWSLambdaBasicExecutionRole attached, creates a python3.12 function whose handler is `print("EVENT_RECEIVED::" + json.dumps(event))`, waits for State=Active, then calls lambda.invoke(FunctionName=<fn>, InvocationType='Event', Payload={"detail":{"marker":"<marker>"}}) and polls logs.filter_log_events(logGroupName='/aws/lambda/<fn>', filterPattern='"<marker>"') for up to 60 s, asserting "positive control direct invoke was never observed in logs".

Expected (AWS): at least one event matching the printed marker in /aws/lambda/<fn> within seconds of the asynchronous invocation (the run also contributes START/END/REPORT RequestId lines to the same stream).
Actual (emulator): FilterLogEvents returns no events for the entire window (group absent or empty), the positive control never converges and the check's 60 s budget expires before it ever reaches its negative-delivery assertions — `log_events == []` (lambda target not invoked) and `messages == []` (no SNS publish) are never evaluated.

## Steps to Reproduce
1. CreateRole trusting lambda.amazonaws.com; AttachRolePolicy arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole.
2. CreateFunction (Runtime=python3.12, Handler=index.handler, Timeout=15, MemorySize=128) with a handler that does `print("EVENT_RECEIVED::" + json.dumps(event))` and returns a dict; poll GetFunctionConfiguration until State=Active.
3. Invoke it with InvocationType='Event' and a payload containing a unique marker string.
4. Poll FilterLogEvents on logGroupName='/aws/lambda/<function-name>' with filterPattern='"<marker>"'.
   AWS: returns at least one event containing the printed marker within seconds. Emulator: returns no events (or the group does not exist) for the whole window.
Replay the check listed in trace_ids to confirm the fix.
