Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.lambda-ddb-accessdenied-logged-to-cloudwatch-negative cloudwatch__dynamodb__lambda.lambda-ddb-putitem-consumed-capacity-metric-cloudwatch cloudwatch__dynamodb__lambda.lambda-ddb-table-not-exist-error-logged-cloudwatch
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-accessdenied-logged-to-cloudwatch-negative|run#9|AssertionError|assert denied_resp["ok"] is False, f"expected denied invocation to report ok=False, got {denied_resp!r}"
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-putitem-consumed-capacity-metric-cloudwatch|run#7|AssertionError|assert total == api_wcu, (
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-table-not-exist-error-logged-cloudwatch|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# cloudwatch__dynamodb__lambda: an unpermitted in-function DynamoDB write succeeds, DynamoDB capacity metrics read zero, and a table-not-found logging check hangs (3 causes)

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Three root causes behind 3 failing checks.

Cause 1 of 3 (check: lambda-ddb-accessdenied-logged-to-cloudwatch-negative): IAM is not enforced for an in-function DynamoDB call — the write the role does not allow succeeds.
AWS: a handler calling PutItem/GetItem on a table its execution role has no grant for is refused with HTTP 400 and error code `AccessDeniedException`, message `User: arn:aws:sts::<account>:assumed-role/<role-name>/<function-name> is not authorized to perform: dynamodb:PutItem on resource: <table-arn> because no identity-based policy allows the dynamodb:PutItem action` (fixed text; account, role, function, action and table ARN come from the request and the role), and the handler's traceback for it lands in the function's CloudWatch log group. Smithy: DynamoDB PutItem/GetItem declare ResourceNotFoundException/ProvisionedThroughputExceededException/etc. as modelled errors; the authorization refusal is the service-common AccessDeniedException.
Emulator: the denied invocation reports success — `expected denied invocation to report ok=False, got {'ok': True}`. No denial is produced and therefore nothing is logged.
Why: least-privilege is unverifiable and silently absent — an application tested here appears to work with a role that AWS would refuse, and the refusal an operator would find in CloudWatch never exists.
Fix: the IAM authorization path for in-function AWS SDK calls (the DynamoDB data-plane handler's authorization check).

Cause 2 of 3 (check: lambda-ddb-putitem-consumed-capacity-metric-cloudwatch): the CloudWatch metric plane does not reproduce DynamoDB's own capacity accounting.
AWS: `AWS/DynamoDB` `ConsumedWriteCapacityUnits` for a table sums exactly the capacity the PutItem responses themselves reported in their `ConsumedCapacity` member — three writes reporting 1.0 each give Sum = 3.0 over the period. Smithy: PutItem's output carries `ConsumedCapacity`, which is the same accounting the metric must mirror.
Emulator: GetMetricStatistics reports `ConsumedWriteCapacityUnits` Sum = 0 for the table while the three Lambda-driven PutItems reported 3.0 write capacity units in their own responses.
Why: capacity dashboards, autoscaling and throttling alarms all read this metric; a flat zero makes provisioned-capacity tuning and cost attribution impossible and hides hot tables.
Fix: the DynamoDB data-plane handler's metric emission into the CloudWatch metric store (it must publish the same ConsumedCapacity it returns).

Cause 3 of 3 (check: lambda-ddb-table-not-exist-error-logged-cloudwatch): the scenario never reaches a verdict.
AWS: a handler calling PutItem against a table that does not exist gets HTTP 400 `ResourceNotFoundException` with message `Requested resource not found` and the traceback appears in the function's log group within seconds.
Emulator: `Error: Check timed out after 60.0s` — no outcome is observable.
Why: the error path an operator debugs from (a not-found traceback in CloudWatch Logs) cannot be seen at all.
Fix: the Lambda invoke + CloudWatch Logs delivery path for a handler error, and the DynamoDB not-found refusal it depends on.
Traces: the three failing checks in trace_ids, under `### cloudwatch__dynamodb__lambda`. An open issue, 20261105T111951Z-cloudwatch-dynamodb-lambda-the-lambda-python-runtime-has.md, covers the absent SDK and missing logs/metrics for other checks of this service; it does not carry these three replay keys.

## Evidence
Cause 1 — expected: HTTP 400 AccessDeniedException `User: arn:aws:sts::<account>:assumed-role/<role>/<function> is not authorized to perform: dynamodb:PutItem on resource: <table-arn> because no identity-based policy allows the dynamodb:PutItem action`. Actual: AssertionError: expected denied invocation to report ok=False, got {'ok': True}.

Cause 2 — expected: AWS/DynamoDB ConsumedWriteCapacityUnits Sum equals the sum of the ConsumedCapacity values the PutItem responses returned (3.0 here). Actual: AssertionError: CloudWatch reports ConsumedWriteCapacityUnits Sum=0 for the table while the 3 Lambda-driven PutItems reported 3.0 write capacity units in their own ConsumedCapacity responses.

Cause 3 — expected: ResourceNotFoundException `Requested resource not found` logged to /aws/lambda/<function> within seconds. Actual: Error: Check timed out after 60.0s.

## Steps to Reproduce
Cause 1: create a table and a Lambda whose role grants no dynamodb action; invoke a handler that calls PutItem on it. Expected AccessDeniedException; observed ok=True.
Cause 2: invoke a handler that PutItems three times with ReturnConsumedCapacity=TOTAL, then GetMetricStatistics for AWS/DynamoDB ConsumedWriteCapacityUnits, Statistics=[Sum], over the invocation window. Expected Sum = the summed ConsumedCapacity; observed 0.
Cause 3: invoke a handler that PutItems to a nonexistent table and poll the function's log group for the traceback — no verdict within 60 s.
