Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.lambda-custom-putmetricdata-missing-permission-negative cloudwatch__lambda__logs.lambda-cloudwatch-getmetricstatistics-missing-permission-negative
Issue-Key: cloudwatch__lambda__logs.lambda-custom-putmetricdata-missing-permission-negative|run#13|AssertionError|assert test_payload.get("error_code") == "AccessDenied", (
Issue-Key: cloudwatch__lambda__logs.lambda-cloudwatch-getmetricstatistics-missing-permission-negative|run#14|AssertionError|assert test_payload.get("error_code") == "AccessDenied", (
Issue-Commit: 1945ee51cfaab7e95ed50a7b8bc82cc22a5254e2

# cloudwatch__lambda__logs: 2 net-new causes — CloudWatch PutMetricData and GetMetricStatistics are both unauthorized-by-design, succeeding for a Lambda role that lacks the grant instead of refusing AccessDenied

> _Note: this issue was identified against the code base at commit `1945ee51cfaab7e95ed50a7b8bc82cc22a5254e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
Two net-new root causes, both of them a missing authorization check on a CloudWatch operation. Each has its own failing check.

Cause 1 of 2: PutMetricData is accepted for a caller with no cloudwatch:PutMetricData grant. Failing check: cloudwatch__lambda__logs.lambda-custom-putmetricdata-missing-permission-negative.
AWS: Smithy (com-amazonaws-cloudwatch-graniteserviceversion20100801) models PutMetricData as Namespace (required, @length 1..255, pattern ^[^:]) + MetricData with an empty output, and only InternalServiceFault (500/InternalServiceError), InvalidParameterCombinationException (400/InvalidParameterCombination), InvalidParameterValueException (400/InvalidParameterValue) and MissingRequiredParameterException (400/MissingParameter) as modeled errors — authorization is not modeled, so this is recorded behavior only. The check is a twin pair: the same handler source runs under a role granted cloudwatch:PutMetricData and under a role that is not, and it asserts the ungranted handler's botocore ClientError code is exactly `AccessDenied` while the granted one returns error_code None.

Cause 2 of 2: GetMetricStatistics is likewise accepted for a caller with no cloudwatch:GetMetricStatistics grant. Failing check: cloudwatch__lambda__logs.lambda-cloudwatch-getmetricstatistics-missing-permission-negative.
AWS: Smithy models GetMetricStatistics as Namespace/MetricName/StartTime/EndTime/Period (all required; Dimensions @length 0..30, Statistics @length 1..5), output Label + Datapoints, with the same four modeled error shapes and no modeled authorization error — recorded behavior only. The check asserts that an in-handler cloudwatch.get_metric_statistics(Namespace='AWS/Lambda', MetricName='Invocations', Period=3600, Statistics=['Sum']) from a role without the read grant is "refused with exactly AccessDenied", returning the error code rather than a Datapoints list.

For both causes the requirement the checks pin is the error code `AccessDenied` on the ungranted arm and success on the granted arm. Hypothesis (neither the model nor the recording states it): AWS answers these refusals HTTP 403 and words them "User: <caller ARN> is not authorized to perform: cloudwatch:PutMetricData because no identity-based policy allows the cloudwatch:PutMetricData action" (respectively cloudwatch:GetMetricStatistics) — everything but the caller ARN (the Lambda's assumed-role ARN, taken from the request's credentials) and the action name is fixed text.

Emulator: both calls succeed and report no error. The handlers return error_code None, so the checks fail with "test handler (role WITHOUT cloudwatch:PutMetricData) expected error_code 'AccessDenied', got None" and "test role lacks cloudwatch:GetMetricStatistics, so the call must be refused with exactly AccessDenied; got error_code=None". The granted arms behave correctly, so the divergence is solely the absent authorization step.

Why: a client relying on AWS's behavior cannot validate least-privilege execution roles. Code that is written to catch AccessDenied from CloudWatch and degrade (or to prove a boundary in a negative test) never takes that branch here, so a role that will be refused in AWS looks sufficient against the emulator; and a metric read that AWS would refuse instead returns real data, leaking observability a restricted principal is not entitled to.

Fix: the two operations' handlers plus the shared request-authorization path of the CloudWatch service crate (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/put_metric_data.rs and .../src/ops/get_metric_statistics.rs) — the signing principal's identity-based policy must be evaluated for cloudwatch:PutMetricData / cloudwatch:GetMetricStatistics and the request refused with the AccessDenied error class before the operation touches metric state or returns datapoints.

Traces: the two failing check ids above, one per cause. The same missing PutMetricData authorization also surfaces under the cloudwatch__dynamodb__lambda heading and is filed separately there because an issue carries one service token.

## Evidence
Expected (AWS): the ungranted arm of each twin is refused with error code `AccessDenied`; the granted arm succeeds.

Actual (emulator):
- lambda-custom-putmetricdata-missing-permission-negative: AssertionError: test handler (role WITHOUT cloudwatch:PutMetricData) expected error_code 'AccessDenied', got None — the handler's cw.put_metric_data(Namespace=<ns>, MetricData=[{MetricName:<name>, Value:1.0, Unit:'Count'}]) returned successfully.
- lambda-cloudwatch-getmetricstatistics-missing-permission-negative: AssertionError: test role lacks cloudwatch:GetMetricStatistics, so the call must be refused with exactly AccessDenied; got error_code=None — the handler's cw.get_metric_statistics(Namespace='AWS/Lambda', MetricName='Invocations', Period=3600, Statistics=['Sum']) returned a Datapoints list.

Each check carries an in-file positive control on the same handler source with the permission granted, and those arms pass, so neither failure is a metric-storage or metric-read defect.

## Steps to Reproduce
1. Create two roles Lambda can assume: one with only basic-execution logging permissions, one additionally granting cloudwatch:PutMetricData on * (repeat with cloudwatch:GetMetricStatistics for cause 2).
2. Deploy the same python3.12 handler twice, once per role. For cause 1 the handler calls cloudwatch.put_metric_data(Namespace=<unique ns>, MetricData=[{'MetricName':<name>,'Value':1.0,'Unit':'Count'}]) and returns {'error_code': exc.response['Error']['Code']} on ClientError, else {'error_code': None}. For cause 2 the handler calls cloudwatch.get_metric_statistics(Namespace='AWS/Lambda', MetricName='Invocations', StartTime=now-1h, EndTime=now, Period=3600, Statistics=['Sum']) and returns the same shape.
3. Invoke both functions and read the payloads.
4. AWS: the ungranted function returns error_code 'AccessDenied'; the granted one returns error_code None. Emulator: both return error_code None.
5. Re-replay the two check ids in trace_ids to confirm.
