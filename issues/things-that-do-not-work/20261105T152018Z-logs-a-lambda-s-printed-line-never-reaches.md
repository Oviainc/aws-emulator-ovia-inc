Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.logs-lambda-emits-to-log-group
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.logs-lambda-emits-to-log-group|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 8d52f52fda0ec71b00c9bf8f5ac2d8cb4a7a8ef6

# logs: a Lambda's printed line never reaches its /aws/lambda/<fn> log group, so FilterLogEvents never returns it (1 cause)

> _Note: this issue was identified against the code base at commit `8d52f52fda0ec71b00c9bf8f5ac2d8cb4a7a8ef6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
FilterLogEvents

## Description
Cause 1 of 1: a successful Lambda invocation writes nothing into the function's own CloudWatch Logs group — the group `/aws/lambda/<function-name>` is never created and FilterLogEvents never returns the line the handler printed.

AWS: recorded behavior only for the delivery itself (Lambda's lazy log-group creation and stdout capture are documented Lambda/CloudWatch Logs behaviour, not modelled in Smithy). A function whose execution role grants logs:CreateLogGroup, logs:CreateLogStream and logs:PutLogEvents on `arn:aws:logs:*:*:*` has its log group `/aws/lambda/<function-name>` created on first invocation; everything the handler writes to stdout is delivered there as log events, so FilterLogEvents(logGroupName="/aws/lambda/<fn>") returns an event whose `message` contains the printed text. Before delivery the group does not exist and FilterLogEvents answers ResourceNotFoundException (the check tolerates that while polling). Status/error codes: Invoke answers 200 with StatusCode 200; the divergence is the permanently empty log group.

Emulator: Invoke answers StatusCode 200, and the FilterLogEvents poll (a 180s budget, 5s interval) never returns any event containing the printed marker; the suite kills the check with `Error: Check timed out after 60.0s`.

Why: reading a function's own logs is how clients debug, assert and alarm on Lambda behaviour. With the group never created and no events delivered, FilterLogEvents/GetLogEvents on `/aws/lambda/<fn>` is useless, and everything layered on that group — retention policies, metric filters, subscription filters, Insights queries — has nothing to read.

Fix: the Lambda invocation path's CloudWatch Logs delivery component — it must create `/aws/lambda/<function-name>` on first invocation, create the invocation's log stream, and PutLogEvents the handler's stdout into it so the logs-side reads resolve.

Related but NOT covering this: open issue 20261105T111951Z-lambda-logs-an-invocation-writes-nothing-to-its.md records the same defect under the `lambda__logs` heading; this check is listed by the suite under the `logs` heading and is not attributed to that issue.

Trace: logs.logs-lambda-emits-to-log-group.

## Evidence
Expected: after Invoke (InvocationType=RequestResponse, StatusCode 200) of a python3.12 handler that prints a unique marker, FilterLogEvents(logGroupName="/aws/lambda/<fn>") returns at least one event whose `message` contains that marker (the group having been created by the first invocation).

Actual: FilterLogEvents keeps returning no matching event (the group is never created), and the check ends with `Error: Check timed out after 60.0s`.

## Steps to Reproduce
1. CreateRole trusting lambda.amazonaws.com; PutRolePolicy allowing logs:CreateLogGroup, logs:CreateLogStream and logs:PutLogEvents on arn:aws:logs:*:*:*.
2. CreateFunction (Runtime=python3.12, Handler=index.lambda_handler) whose handler does `print("<marker>")`; wait for State==Active.
3. Invoke(FunctionName=<fn>, InvocationType=RequestResponse, Payload={}) -> StatusCode 200.
4. Poll FilterLogEvents(logGroupName="/aws/lambda/<fn>") for an event whose message contains <marker> -> never returns one; the log group is never created.
