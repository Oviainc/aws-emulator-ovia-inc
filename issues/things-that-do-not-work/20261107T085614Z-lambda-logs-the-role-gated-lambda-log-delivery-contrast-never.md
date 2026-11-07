Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group
Issue-Key: lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: ad889ea564c74ff66a8d5d7080e6fcd3e62d9684

# The role-gated Lambda log-delivery contrast never settles: an invoke's LogType=Tail / log-group delivery pair reaches no verdict

> _Note: this issue was identified against the code base at commit `ad889ea564c74ff66a8d5d7080e6fcd3e62d9684`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: the scenario that contrasts a function whose execution role grants no CloudWatch Logs permission with a twin that carries AWSLambdaBasicExecutionRole never reaches a verdict. Both functions are created and waited to Active, both are invoked synchronously with LogType=Tail, and the check then waits for the twin's printed line to appear in `/aws/lambda/<twin>` before asserting that no `/aws/lambda/<bare>` group was created. The replay is cut at the 60s budget, so neither half of the contrast is observable.

AWS (Smithy: service-com-amazonaws-lambda-awsgirapiservice Invoke; service-com-amazonaws-cloudwatchlogs-logs-20140328 FilterLogEvents / DescribeLogGroups):
  - A synchronous Invoke of a working handler answers StatusCode 200 with no FunctionError, a RequestId in the response metadata that is a UUID, and — with LogType=Tail — a base64 `LogResult` holding the invocation's log tail. That tail contains the fixed-form lines `START RequestId: <that request id> Version: $LATEST`, the handler's own printed line verbatim, and a `REPORT RequestId: <that request id>\t…` line. The request id is a per-run value and only its JOIN with the START/REPORT lines is the requirement, never a literal id.
  - The tail is returned regardless of the execution role's CloudWatch Logs permissions, because it travels in the Invoke response rather than through logs:PutLogEvents. Separately, a function whose role carries AWSLambdaBasicExecutionRole has `/aws/lambda/<function>` created lazily on first invoke and its printed line delivered there (FilterLogEvents returns the event with message `out-<marker>\n`), while a function whose role has only the lambda.amazonaws.com trust document and no logs grant has NO `/aws/lambda/<function>` group created at all — DescribeLogGroups with that prefix returns it in neither page.

Emulator: the check is cut at 60s with no verdict. Its own budgets are far larger (180s to reach Active, 150s to observe the twin's event), so the cut means the invoke and/or the log delivery never settles promptly, and the role-gated asymmetry the check grades produces nothing to compare.

Why: a client cannot rely on the Tail log an Invoke returns (the START/print/REPORT envelope that every local debugging and CI log assertion reads), and cannot rely on the execution role gating log-group creation: the security property "no logs grant => no log group is ever created for this function" is unverifiable, and so is its positive control. A least-privilege review that asserts the ungranted function creates no group would pass vacuously because the granted one creates none either.

Related open issues, neither of which carries this replay key: 20261105T111951Z-lambda-logs-an-invocation-writes-nothing-to-its.md (an invocation writes nothing to its log group) and 20261107T035448Z-lambda-logs-3-net-new-causes-loggingconfig-loggroup.md (LoggingConfig.LogGroup).

Fix: the Lambda Invoke handler's execution path — the LogResult/Tail envelope it builds (START/REPORT lines joined to the response's request id) and the lazy creation of `/aws/lambda/<function>` plus delivery of the handler's stdout into it — together with the authorization gate that withholds that creation and delivery when the execution role grants no logs:CreateLogGroup/CreateLogStream/PutLogEvents.

Traces: lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group.

## Evidence
Check lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group: `Error: Check timed out after 60.0s`.

The check's sequence and what AWS answers:
1. iam:CreateRole twice with the trust document {Effect: Allow, Principal: {Service: lambda.amazonaws.com}, Action: sts:AssumeRole}; attach arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole to one of them only.
2. lambda:CreateFunction twice (Runtime python3.12, Handler index.handler, MemorySize 128, Timeout 30) with a handler that prints `out-<marker>` and returns {"ok": true}; poll GetFunctionConfiguration until State == "Active" and LastUpdateStatus in (None, "Successful").
3. lambda:Invoke each with LogType=Tail. Expected per AWS: StatusCode 200, no FunctionError, payload {"ok": true}, a UUID RequestId, and a non-empty LogResult whose decoded lines include `START RequestId: <that id> Version: $LATEST`, the literal `out-<marker>` line, and a line starting `REPORT RequestId: <that id>\t`.
4. Poll FilterLogEvents(logGroupName="/aws/lambda/<granted fn>") for an event with message `out-<marker>\n` (expected to arrive).
5. Poll DescribeLogGroups(logGroupNamePrefix="/aws/lambda/<ungranted fn>") and assert the group is NOT created.

Actual (emulator): the run is cut at 60s before step 4 or 5 produces a comparison. The check's own waits are 180s (Active) and 150s (the twin's event), so the budget cut reflects the emulator not settling, not the check running out of its own patience.

## Steps to Reproduce
Replay lambda__logs.log-cwl-role-without-logs-permission-keeps-the-tail-and-creates-no-group.

1. iam:CreateRole with only the lambda.amazonaws.com sts:AssumeRole trust document (no managed or inline policies) — call it the bare role. Create a second identical role and attach arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole — the basic role.
2. lambda:CreateFunction on each role: Runtime python3.12, Handler index.handler, a zip whose index.py prints "out-" + event["marker"] and returns {"ok": True}. Wait for State == Active.
3. lambda:Invoke each with LogType=Tail and Payload {"marker": "<m>"}. Expect StatusCode 200, no FunctionError, a UUID RequestId, and a LogResult whose decoded text holds the START line for that request id, the "out-<m>" line, and the REPORT line for that request id.
4. logs:FilterLogEvents(logGroupName="/aws/lambda/<basic fn>") — expect an event with message "out-<m>\n".
5. logs:DescribeLogGroups(logGroupNamePrefix="/aws/lambda/<bare fn>") — expect the group to be absent across all pages.
