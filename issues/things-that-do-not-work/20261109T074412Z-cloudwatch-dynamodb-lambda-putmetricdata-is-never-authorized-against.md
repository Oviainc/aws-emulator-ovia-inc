Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.ddb-stream-lambda-missing-cloudwatch-permission-negative cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative cloudwatch__dynamodb__lambda.lambda-put-metric-missing-permission-negative cloudwatch__dynamodb__lambda.skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data
Issue-Key: cloudwatch__dynamodb__lambda.ddb-stream-lambda-missing-cloudwatch-permission-negative|run#24|AssertionError|assert len(bad_points) == 0, "custom metric must never land without the permission"
Issue-Key: cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative|run#16|AssertionError|assert starved_points == [], f"expected no datapoints for {starved_metric}, got {starved_points}"
Issue-Key: cloudwatch__dynamodb__lambda.lambda-put-metric-missing-permission-negative|run#12|AssertionError|assert neg_payload["ok"] is False, f"expected the denied call to fail, got payload {neg_payload}"
Issue-Key: cloudwatch__dynamodb__lambda.skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data|run#7|AssertionError|assert len(warn) == 1, f"the denied publish was not reported: {inv['lines']!r}"
Issue-Commit: 70f3a3a7604d630d6ba1835b99fbb61e8ce58784

# cloudwatch__dynamodb__lambda: PutMetricData is never authorized against the caller's identity — a Lambda execution role with no cloudwatch:PutMetricData grant still lands the datapoint

> _Note: this issue was identified against the code base at commit `70f3a3a7604d630d6ba1835b99fbb61e8ce58784`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
Cause 1 of 1: CloudWatch PutMetricData performs no identity-based authorization. Four permission-boundary checks in this suite build an IAM role that deliberately omits `cloudwatch:PutMetricData`, invoke a Lambda under that role, and assert the publish is refused. In every one of them the emulator accepts the call and stores the datapoint, so the boundary does not exist.

AWS: PutMetricData is bound to the awsQuery CloudWatch service (Smithy: `PutMetricData`, input `PutMetricDataInput` with `Namespace` @required @length(1,255) @pattern(^[^:]) and `MetricData`; modelled errors InternalServiceFault 500/InternalServiceError, InvalidParameterCombinationException 400/InvalidParameterCombination, InvalidParameterValueException 400/InvalidParameterValue, MissingRequiredParameterException 400/MissingParameter). The model declares no per-operation authorization error, because an authorization refusal is the service-common one: AWS evaluates the caller's identity-based policy BEFORE the write and, when no statement allows the action, answers HTTP 403 with the awsQuery error envelope Type `Sender`, Code `AccessDenied` and the message AWS words for a denied action — fixed text `User: `, ` is not authorized to perform: `, ` because no identity-based policy allows the `, ` action`, with the variable parts taken from the request's signing identity and the action: the caller principal ARN (for a Lambda execution role, the `arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name>` the invocation assumed) and the action name `cloudwatch:PutMetricData`. Nothing is stored: no metric, no datapoint, and no series for GetMetricStatistics / GetMetricData to return. The suite's recorded behavior agrees — each check's in-handler control path catches a `ClientError` whose `Error.Code` is `AccessDenied` and whose message names `cloudwatch:PutMetricData`, and each check pairs the denied role with an otherwise identical PERMITTED role whose publish does land, so the only difference under test is the grant.

Emulator: the call returns success for the ungranted role and the datapoint becomes readable.
  - `ddb-stream-lambda-missing-cloudwatch-permission-negative`: a stream-triggered function whose role has no `cloudwatch:PutMetricData` publishes `CustomInvoke` into its own namespace anyway — `AssertionError: custom metric must never land without the permission` (the check reads the bad function's namespace and finds datapoints).
  - `deploy-cloudwatch-composite-alarm-missing-metric-negative`: the CFN role is `logs-only` (logs:CreateLogGroup/CreateLogStream/PutLogEvents and nothing else), yet `ProcessedCount` has a series — `AssertionError: expected no datapoints for ProcessedCount, got [{'Timestamp': ..., 'Sum': 3.0, 'Unit': 'None'}]`. The 3.0/timestamp are one run's values; the requirement is that the list is empty.
  - `lambda-put-metric-missing-permission-negative`: the handler returns `{'ok': True}` — the SDK call raised nothing — `AssertionError: expected the denied call to fail, got payload {'ok': True}`. AWS makes this `{'ok': False, 'code': 'AccessDenied'}`.
  - `skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data`: the handler's refusal log line (`Failed to publish skipped records ...` naming `AccessDenied` and `cloudwatch:PutMetricData`) is absent because the publish succeeded — `AssertionError: the denied publish was not reported: ['Skipping record - no OldImage present (event type: INSERT)', 'Processed 0 records, skipped 1 records']`.

Why: a client relying on AWS's behavior gets the permission model inverted. Least-privilege policies cannot be validated against the emulator: a role that AWS refuses silently succeeds here, so a deployment tested green against the emulator fails closed in AWS the first time it publishes a metric. Conversely, negative/boundary tests and alarm-starvation scenarios (a metric that must stay absent so its alarm stays INSUFFICIENT_DATA) can never be reproduced — the starved metric always has data, and an alarm over it transitions when AWS leaves it in INSUFFICIENT_DATA. Error-handling code paths that branch on `AccessDenied` from PutMetricData are unreachable and therefore untestable.

Fix: the authorization gate of the PutMetricData handler in the cloudwatch service crate — the identity-based-policy evaluation of the signing principal for action `cloudwatch:PutMetricData` must run before any datapoint is appended to the metric store, and must refuse with the 403 `AccessDenied` awsQuery error described above instead of writing. The same gate is what makes the four checks' permitted controls still succeed, so it belongs in front of the write, not in the alarm/metric-stream fan-out that the write triggers.

Related but NOT covering this: `20261106T151148Z-cloudwatch-dynamodb-lambda-an-unpermitted-in-function-dynamodb.md` reports the same missing gate for an in-function DynamoDB write (a different service's action), and `20261108T205321Z-cloudwatch-dynamodb-lambda-the-in-function-aws-sdk.md` reports a PutMetricData serialization defect (a Python datetime for a modelled timestamp member), not the authorization gate.

Traces: the four failing check ids below. Handler under suspicion: services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/put_metric_data.rs (its doc header at lines 3-11 describes the write and the alarm/metric-stream fan-out it triggers, with no authorization step).

## Evidence
All four checks are [mismatch] under `### cloudwatch__dynamodb__lambda` and none is marked ALREADY FILED or OWNED.

1) cloudwatch__dynamodb__lambda.ddb-stream-lambda-missing-cloudwatch-permission-negative
   Expected: the function whose role omits cloudwatch:PutMetricData publishes nothing; its namespace has zero datapoints (the identical role WITH the grant publishes normally).
   Actual: `AssertionError: custom metric must never land without the permission` — bad_points is non-empty.

2) cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative
   Expected: `starved_points == []` for metric ProcessedCount (the CFN LambdaRole grants only logs:CreateLogGroup/CreateLogStream/PutLogEvents).
   Actual: `AssertionError: expected no datapoints for ProcessedCount, got [{'Timestamp': datetime.datetime(2026, 11, 9, 7, 22, tzinfo=tzlocal()), 'Sum': 3.0, 'Unit': 'None'}]` (the timestamp/Sum are per-run values; the requirement is the empty list).

3) cloudwatch__dynamodb__lambda.lambda-put-metric-missing-permission-negative
   Expected: the in-handler `cw.put_metric_data(...)` raises ClientError with `Error.Code == "AccessDenied"`, so the handler returns `{'ok': False, 'code': 'AccessDenied'}`; HTTP 403, awsQuery Type `Sender`, Code `AccessDenied`, message `User: <caller principal ARN> is not authorized to perform: cloudwatch:PutMetricData because no identity-based policy allows the cloudwatch:PutMetricData action`.
   Actual: `AssertionError: expected the denied call to fail, got payload {'ok': True}` — the publish returned 200.

4) cloudwatch__dynamodb__lambda.skipped-records-metric-is-not-published-when-the-role-lacks-put-metric-data
   Expected: exactly one handler log line starting `Failed to publish skipped records` that contains `AccessDenied` and `cloudwatch:PutMetricData`; after the grant is added, SkippedRecords sums to exactly 1.0 over 1.0 samples (the swallowed publish must never replay).
   Actual: `AssertionError: the denied publish was not reported: ['Skipping record - no OldImage present (event type: INSERT)', 'Processed 0 records, skipped 1 records']` — no refusal was ever raised to the handler.

Smithy (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801): PutMetricData input Namespace @required @length(min 1, max 255) @pattern(^[^:]) + MetricData; modelled errors InternalServiceFault(500/InternalServiceError), InvalidParameterCombinationException(400/InvalidParameterCombination), InvalidParameterValueException(400/InvalidParameterValue), MissingRequiredParameterException(400/MissingParameter) — no authorization error is modelled per-operation; the 403 AccessDenied refusal is the service-common one.

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com whose policy grants ONLY logs:CreateLogGroup/CreateLogStream/PutLogEvents (no cloudwatch:PutMetricData).
2. Create a Lambda function with that role whose handler calls `boto3.client('cloudwatch').put_metric_data(Namespace=<unique ns>, MetricData=[{'MetricName': 'GuardedMetric', 'Value': 1.0, 'Unit': 'Count'}])` and returns `{'ok': True}` on success or `{'ok': False, 'code': err.response['Error']['Code']}` on ClientError.
3. Invoke the function.
   Expected (AWS): payload `{'ok': False, 'code': 'AccessDenied'}` — the call is refused 403 with code AccessDenied and a message naming the caller's assumed-role ARN and `cloudwatch:PutMetricData`.
   Actual (emulator): payload `{'ok': True}`.
4. Call GetMetricStatistics / GetMetricData for that namespace and metric over the invocation window.
   Expected (AWS): no datapoints at all.
   Actual (emulator): the datapoint is present.
5. Repeat with the SAME handler under a role that additionally allows cloudwatch:PutMetricData — both the permitted and the unpermitted role behave identically in the emulator, which is the defect.
6. Variant that also reproduces it through a stream trigger: wire a DynamoDB stream event source mapping to two identical functions, one role with the grant and one without, write one item to each table, and read each function's namespace — the ungranted one must stay empty and does not.
