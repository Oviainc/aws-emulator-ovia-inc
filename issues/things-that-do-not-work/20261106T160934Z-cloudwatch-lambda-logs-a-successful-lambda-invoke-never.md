Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow
Issue-Key: cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow|run#14|Timeout|resp = _run_aws(ctx, "logs", "describe-log-groups", "--log-group-name-prefix", expected_group)
Issue-Commit: f4d38684d747c74ac6ba836920c3ff6065efe14a

# cloudwatch__lambda__logs: a successful Lambda Invoke never lazily creates /aws/lambda/<function>, so DescribeLogGroups with that prefix stays empty forever

> _Note: this issue was identified against the code base at commit `f4d38684d747c74ac6ba836920c3ff6065efe14a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause (1 of 1) surviving the suite's ownership marking in this service group. The check is the only failing check in the whole replay that is neither marked ALREADY FILED nor OWNED by an open issue.

Cause 1 of 1: a Lambda function whose execution role carries AWSLambdaBasicExecutionRole is invoked successfully, but the function's own CloudWatch Logs log group `/aws/lambda/<function-name>` is never created, so `logs describe-log-groups --log-group-name-prefix /aws/lambda/<function-name>` keeps returning an empty `logGroups` list indefinitely.
Failing check ids: cloudwatch__lambda__logs.cli-describe-log-groups-after-lambda-invoke-workflow.

AWS: recorded behavior only — the lazy-creation rule is documented AWS Lambda behaviour and is not expressed in Smithy. Smithy: logs DescribeLogGroups input member `logGroupNamePrefix` (target LogGroupName, @length min 1 max 512, pattern ^[\.\-_/#A-Za-z0-9]+$), output member `logGroups` (list, paginated items=logGroups); modeled errors only InvalidParameterException (400) and ServiceUnavailableException (500) — a successful call is a 200 with a `logGroups` array. The recording shows: before the first invocation DescribeLogGroups with the prefix `/aws/lambda/<function-name>` returns an empty `logGroups` list (creation is genuinely lazy); the first invocation answers HTTP 200 with `StatusCode` 200 and, as a side effect, creates the log group whose `logGroupName` is exactly the string `/aws/lambda/<function-name>` (fixed text `/aws/lambda/` + the function name taken from the request); DescribeLogGroups with that same prefix then returns exactly one entry whose `logGroupName` equals that string. No error code or non-2xx status is involved on either side — this is a missing side effect, not a refusal.

Emulator: the pre-invoke DescribeLogGroups correctly returns an empty `logGroups` list, and `lambda invoke` correctly answers `StatusCode` 200, but the post-invoke DescribeLogGroups poll with the same prefix never returns any entry. The check therefore spins in its wait loop until the harness budget expires and the replay reports `Error: Check timed out after 60.0s`. The emulator never refuses anything here; it simply never provisions the group, so the assertion `log group /aws/lambda/<function-name> never appeared via describe-log-groups` can never be satisfied.

Why: deriving `/aws/lambda/<name>` from a function name, describing it, then reading it is the single most common way a client finds a function's logs — the AWS CLI's `aws logs tail`, every console log view and essentially all log-reading tooling do exactly this. Against this emulator that path never resolves, so a client either hangs in a poll loop or concludes the function produced no logs. Everything that must be attached to the function's own group also becomes unreachable: PutRetentionPolicy, PutMetricFilter, PutSubscriptionFilter and data-protection policies on `/aws/lambda/<name>` all fail with ResourceNotFoundException because the group is never created.

Fix: the Lambda invocation path's CloudWatch Logs provisioning component — on the first invocation of a function, Invoke must create the log group named `/aws/lambda/<function-name>` (and its per-invocation log stream) in the logs service state, before/alongside writing the invocation's records, so that DescribeLogGroups/DescribeLogStreams in the logs service observe it. It belongs in the Invoke handler's log-delivery step (and the logs-state registration it calls), not in DescribeLogGroups, which correctly reports the state it is given.

Related but NOT covering this: open issue 20261105T152018Z-cloudwatch-lambda-logs-a-lambda-invoke-never-lazily.md describes the same lazy-creation gap and even lists this check id in its Issue-Trace-Ids, but the suite does NOT attribute this failure to it (the check is unmarked in the replay report, i.e. its replay key is not carried by that issue), so the failure is still live and unowned. Open issue 20261105T111951Z-cloudwatch-lambda-logs-5-net-new-root-causes.md owns other checks in this group, including the EMF-extraction check, and does not cover this one either.

## Evidence
Expected (AWS):
1. DescribeLogGroups(logGroupNamePrefix="/aws/lambda/<fn>") before any invoke -> HTTP 200, logGroups == [].
2. Invoke(FunctionName=<fn>, Payload={}) -> HTTP 200, StatusCode == 200.
3. DescribeLogGroups(logGroupNamePrefix="/aws/lambda/<fn>") after the invoke -> HTTP 200 with exactly one entry whose logGroupName == "/aws/lambda/<fn>" (fixed text "/aws/lambda/" + the function name from the Invoke request).

Actual (emulator):
1. matches — logGroups == [].
2. matches — StatusCode == 200.
3. diverges — logGroups stays [] on every poll for the full wait window; the group is never created. The replay ends with `Error: Check timed out after 60.0s` (the check's 60s poll loop outlives the harness budget), so the assertion `log group /aws/lambda/<fn> never appeared via describe-log-groups` is the behaviour being reported.

The divergence is a missing side effect of Invoke, not a status/error-code difference: no status, error code or error message is returned by the emulator on the diverging call — DescribeLogGroups answers 200 with an empty list where AWS answers 200 with one log group.

## Steps to Reproduce
1. iam CreateRole with a trust policy allowing lambda.amazonaws.com to sts:AssumeRole; AttachRolePolicy arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole.
2. lambda CreateFunction (Runtime python3.12, Handler index.handler, a zip containing `def handler(event, context): return 'ok'`, Publish=true) with that role; poll GetFunctionConfiguration until State == "Active".
3. aws logs describe-log-groups --log-group-name-prefix /aws/lambda/<fn>  ->  logGroups == [] (correct; creation is lazy).
4. aws lambda invoke --function-name <fn> --payload '{}' /dev/null  ->  StatusCode 200.
5. Poll aws logs describe-log-groups --log-group-name-prefix /aws/lambda/<fn> every 3s.
   Expected: within seconds, one entry with logGroupName == "/aws/lambda/<fn>".
   Observed: logGroups stays empty until the check is killed by the 60s budget.
