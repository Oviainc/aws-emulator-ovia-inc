Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.lambda-missing-logs-permission-negative
Issue-Key: cloudwatch__dynamodb__lambda.lambda-missing-logs-permission-negative|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: e4e3e5befb4453536c7145f8d5553fe1cd0770b8

# cloudwatch__dynamodb__lambda: the missing-logs-permission boundary check reaches no verdict — a permitted Lambda invocation's stdout never becomes visible in /aws/lambda/<fn>, so the check hangs past the 60 s cap

> _Note: this issue was identified against the code base at commit `e4e3e5befb4453536c7145f8d5553fe1cd0770b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
FilterLogEvents

## Description
Cause 1 of 1: a Lambda invocation whose execution role carries AWSLambdaBasicExecutionRole produces no log events that CloudWatch Logs will serve, so the check's positive control never settles and the scenario reaches no verdict at all.
AWS: recorded behavior only for the delivery latency; the documented behavior is that an invocation of a function whose execution role allows logs:CreateLogGroup/CreateLogStream/PutLogEvents has its stdout (here a single `print(f"MARKER::{...}")`) delivered, within seconds, as a log event in the log group /aws/lambda/<function-name>, retrievable by FilterLogEvents with a filterPattern matching the printed marker. The counterpart requirement the check then grades is that a function whose role carries only the lambda.amazonaws.com trust policy and no logs:* permission still invokes successfully (no FunctionError, the handler's {"ok": true} payload) but delivers NO log events — FilterLogEvents on /aws/lambda/<fn> stays empty (or the group is absent, ResourceNotFoundException). The fixed requirements are: invocation succeeds in both cases; events present for the permitted role, absent for the unpermitted one. The function names and markers are per-run values.
Emulator: the check ends `Error: Check timed out after 60.0s`. Its positive control polls FilterLogEvents on /aws/lambda/<pos_fn> for the printed marker for up to 90 s and must succeed before the negative absence window is even entered, so the emulator never made the permitted invocation's stdout visible in its log group inside the window. Nothing after that point is graded: whether an unpermitted role is correctly withheld from CloudWatch Logs is unknown, and the IAM boundary this check exists to prove is untested.
Why: a client reading its function's output from CloudWatch Logs gets nothing, so no invocation can be debugged and no log-based assertion (a marker, an error trace, a request id) can be made; and because the positive control is a precondition, the emulator also silently suppresses the verdict on the logs-permission boundary itself.
Fix: the Lambda invocation path's log delivery — the component that creates /aws/lambda/<function-name>, opens a stream per invocation and appends the handler's stdout/stderr as log events through the execution role's logs permissions, and the CloudWatch Logs FilterLogEvents read path those events must be visible through. Open issue 20261105T111951Z-cloudwatch-dynamodb-lambda-the-lambda-python-runtime-has.md reports that invocation logs and metrics never reach CloudWatch for other checks of this service and does not claim this check.
Traces: cloudwatch__dynamodb__lambda.lambda-missing-logs-permission-negative.

## Evidence
Expected: after invoking a function whose role has AWSLambdaBasicExecutionRole, logs.filter_log_events(logGroupName="/aws/lambda/<pos_fn>", filterPattern='"<marker-pos>"') returns at least one event within seconds; then the same probe against the unpermitted function's group returns [] for the whole window.
Actual: `Error: Check timed out after 60.0s`. The check's first poll (_poll_until over probe_pos, timeout_s=90.0, interval_s=5.0, label="positive control: logs should be delivered when the role has logging permission") never satisfies its predicate, so execution does not reach the negative assertion `assert neg_events == [], f"expected no log events without logs permission, found {neg_events}"`. The function names and markers in the run are per-run generated values and are not part of the requirement.

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com and attach arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole.
2. Create a python3.x Lambda with that role whose handler does `print(f"MARKER::{os.environ['MARKER']}")` and returns {"ok": True}, with env MARKER=<marker>.
3. Invoke it (InvocationType RequestResponse) and confirm the payload is {"ok": true} with no FunctionError.
4. Poll `aws logs filter-log-events --log-group-name /aws/lambda/<fn> --filter-pattern '"<marker>"'` — no event ever appears (and the log group may never exist), so the poll runs out.
5. Repeat 1–4 with a role that has the trust policy and no logs:* permission: the invocation must still succeed while the log group stays empty — this half of the check is never reached because step 4 never settles.
