Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker-metrics.batch-get-metrics-mismatched-axis-period-rejected sagemaker-metrics.batch-put-metrics-invalid-value-rejected sagemaker-metrics.batch-put-metrics-missing-iam-permission-rejected
Issue-Shape-Id: com.amazonaws.sagemakermetrics#SageMakerMetricsService
Issue-Key: sagemaker-metrics.batch-get-metrics-mismatched-axis-period-rejected|run#11|AssertionError|assert results[1]["Status"] == "ValidationError", (
Issue-Key: sagemaker-metrics.batch-put-metrics-invalid-value-rejected|run#6|AssertionError|assert errors.get(1) == "INTERNAL_ERROR", f"expected INTERNAL_ERROR at index 1, errors={errors!r}"
Issue-Key: sagemaker-metrics.batch-put-metrics-missing-iam-permission-rejected|_error#15|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# sagemaker-metrics: BatchGetMetrics skips per-query axis/period validation, BatchPutMetrics mis-codes non-finite values, and the data plane performs no IAM authorization

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetMetrics,BatchPutMetrics

## Description
Three distinct root causes in the sagemaker-metrics data plane, all grounded in failing behavioral checks. Each must be closed before submitting.

Cause 1 of 3: BatchGetMetrics does not validate the per-query XAxisType/Period combination (failing check: sagemaker-metrics.batch-get-metrics-mismatched-axis-period-rejected).
AWS: Smithy (services/service-com-amazonaws-sagemakermetrics-sagemakermetricsservice/smithy-model) models MetricQuery.XAxisType as enum {IterationNumber, Timestamp}, MetricQuery.Period as enum {OneMinute, FiveMinute, OneHour, IterationNumber}, and MetricQueryResult.Status as enum MetricQueryResultStatus {Complete, Truncated, InternalError, ValidationError}. The model does not state which pairings are legal, so for the pairing rule this is recorded behavior only: a query pairing XAxisType=Timestamp with the iteration-based Period=IterationNumber is a semantically invalid combination that AWS resolves, inside an otherwise-200 BatchGetMetrics response, to that query's MetricQueryResult.Status=ValidationError, while a sibling query in the SAME request using the valid IterationNumber axis/period pairing for the same metric resolves to Status=Complete with its values. Per-query semantic failure is thus reported in-band, not as a request-level error.
Emulator: both queries resolve to Status=Complete; the invalid-pairing query is answered as if it were well formed (observed Status 'Complete' where ValidationError is required).
Why: a client that inspects MetricQueryResult.Status to detect a bad query — the only channel AWS offers for it, since the call still returns 200 — sees success for a query AWS rejects, and will plot or act on values that real AWS never returns for that axis/period pairing. Invalid dashboard/query configurations pass silently against the emulator and then fail in production.
Fix: BatchGetMetrics' handler/per-query evaluation path — validate each MetricQuery's XAxisType against its Period independently and emit Status=ValidationError (with the optional Message member) for that element while the rest of the result list is still computed normally.
Traces: sagemaker-metrics.batch-get-metrics-mismatched-axis-period-rejected.

Cause 2 of 3: BatchPutMetrics returns the wrong per-item error code for a non-finite (NaN) Value (failing check: sagemaker-metrics.batch-put-metrics-invalid-value-rejected).
AWS: Smithy models RawMetricData.Value as a required Double and BatchPutMetricsError.Code as enum PutMetricsErrorCode {METRIC_LIMIT_EXCEEDED, INTERNAL_ERROR, VALIDATION_ERROR, CONFLICT_ERROR}, with MetricIndex identifying the offending item. The model does not say which code a non-finite value draws, so for the code selection this is recorded behavior only: an item whose Value is NaN is rejected per-item with Code=INTERNAL_ERROR at its MetricIndex inside a 200 response, while a sibling valid item in the same batch still ingests successfully and the rejected point never becomes queryable.
Emulator: the NaN item is rejected but with Code=VALIDATION_ERROR at MetricIndex 1 (observed Errors: {1: 'VALIDATION_ERROR'}).
Why: an error code is part of the contract. Clients branch on PutMetricsErrorCode — INTERNAL_ERROR is typically retried, VALIDATION_ERROR is typically treated as a permanent client fault and dropped — so the emulator inverts the retry decision for this class of item and client retry/dead-letter logic cannot be exercised faithfully.
Fix: BatchPutMetrics' handler / per-item validation path — classify non-finite Value (NaN, +/-Inf) as INTERNAL_ERROR when building BatchPutMetricsError, keeping MetricIndex and the partial-success behavior for sibling items. Note the check also asserts, after this code is corrected, that the valid sibling item is persisted and the rejected point is not queryable; those assertions were never reached in this run.
Traces: sagemaker-metrics.batch-put-metrics-invalid-value-rejected.

Cause 3 of 3: no IAM authorization on the sagemaker-metrics data plane — BatchPutMetrics succeeds for a principal with no identity-based permission (failing check: sagemaker-metrics.batch-put-metrics-missing-iam-permission-rejected).
AWS: recorded behavior only (the sagemaker-metrics Smithy model declares authSchemes sigv4 and binds no operation-level errors; AccessDeniedException is a service-common/authorization error, not an operation member). A caller using credentials from an assumed role that has a trust policy but NO attached permission policy cannot call BatchPutMetrics: the request is rejected with error code AccessDeniedException, the attempted metric never lands (a later BatchGetMetrics for it returns Status=Complete with an empty MetricValues list), and the same call from the fully permissioned caller succeeds. The recorded message is of the form 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: sagemaker:BatchPutMetrics on resource: arn:aws:sagemaker:<region>:<account>:experiment-trial-component/<name> because no identity-based policy allows the sagemaker:BatchPutMetrics action' — that text embeds the caller identity, the region/account and the generated role, session and trial-component names, so it is an EXAMPLE; the requirement is the AccessDeniedException error code and the refusal itself.
Emulator: the call from the unauthorized role returns HTTP 200 with an empty body ({'ResponseMetadata': {'HTTPStatusCode': 200, ...}}) — no authorization decision is made at all, so the write is accepted.
Why: this is an overly-permissive divergence, the most dangerous kind. Least-privilege work — verifying that a training job's role actually needs sagemaker:BatchPutMetrics, or that a deliberately unprivileged principal is refused — passes against the emulator and fails in AWS, and negative-path tests for AccessDeniedException handling can never be written against it.
Fix: the sagemaker-metrics request pipeline's authorization step (sigv4 principal resolution + IAM policy evaluation for sagemaker:BatchPutMetrics on the trial-component resource), shared with BatchGetMetrics; on denial, return AccessDeniedException and leave state untouched.
Traces: sagemaker-metrics.batch-put-metrics-missing-iam-permission-rejected.

Not filed, for the record: sagemaker-metrics.batch-get-metrics-malformed-resource-arn-rejected also fails, but only on error-message text — the emulator correctly fails the WHOLE call with ValidationException for a malformed MetricQuery.ResourceArn (SageMakerResourceArn @pattern '^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:[a-z\-].*/'), emitting the generic constraint-violation text instead of the recorded 'Invalid SageMaker ARN'. Status and error code agree, and the rule behind AWS's constant string is not documented, so it is not treated as a defect here.

## Evidence
Replay of suite simparity-behavioral-aws-v1: 15/19 passed, 4 mismatches, all in sagemaker-metrics.

Cause 1 — sagemaker-metrics.batch-get-metrics-mismatched-axis-period-rejected:
  BatchPutMetrics(TrialComponentName=<tc>, MetricData=[{MetricName:'paired', Timestamp:now, Step:0, Value:6.0}]) -> no Errors.
  BatchGetMetrics(MetricQueries=[
    {MetricName:'paired', ResourceArn:<tc arn>, MetricStat:'Last', Period:'IterationNumber', XAxisType:'IterationNumber'},
    {same, XAxisType:'Timestamp', Start:now-120, End:now+120}])
  Expected: MetricQueryResults[0].Status == 'Complete' with MetricValues[0] == 6.0; MetricQueryResults[1].Status == 'ValidationError'.
  Actual:   AssertionError: expected Status=ValidationError for the Timestamp/IterationNumber mismatch, got 'Complete'.

Cause 2 — sagemaker-metrics.batch-put-metrics-invalid-value-rejected:
  BatchPutMetrics(MetricData=[{MetricName:'ok-metric', Value:1.0}, {MetricName:'bad-metric', Value:NaN}])
  Expected: Errors contains {MetricIndex:1, Code:'INTERNAL_ERROR'} and no entry for index 0.
  Actual:   AssertionError: expected INTERNAL_ERROR at index 1, errors={1: 'VALIDATION_ERROR'}.

Cause 3 — sagemaker-metrics.batch-put-metrics-missing-iam-permission-rejected:
  iam.CreateRole(<role with trust policy only, no permission policy>) -> sts.AssumeRole -> sagemaker-metrics client with those credentials.
  denied_client.BatchPutMetrics(TrialComponentName=<tc>, MetricData=[{MetricName:'denied-metric', Value:1.0}])
  Expected: ClientError with Error.Code == 'AccessDeniedException' (message of the 'User: ... is not authorized to perform: sagemaker:BatchPutMetrics ...' form), and no queryable 'denied-metric' afterwards.
  Actual:   AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}, 'RetryAttempts': 0}}.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, or drive it directly with botocore against the emulator endpoint:

1) Cause 1: sagemaker.CreateTrialComponent -> BatchPutMetrics one point for metric 'paired' (Step 0) -> BatchGetMetrics with two MetricQueries for that metric on the trial-component ARN: the first {MetricStat:'Last', Period:'IterationNumber', XAxisType:'IterationNumber'}, the second identical but XAxisType='Timestamp' with Start/End set. Observe MetricQueryResults[1].Status: emulator 'Complete', AWS 'ValidationError' (result[0] must stay 'Complete').

2) Cause 2: sagemaker.CreateTrialComponent -> BatchPutMetrics with two items in one batch, item 0 Value=1.0 and item 1 Value=float('nan'). Inspect the Errors list: emulator reports Code='VALIDATION_ERROR' at MetricIndex 1, AWS reports 'INTERNAL_ERROR'. Then BatchGetMetrics for 'ok-metric' (must be Complete, value 1.0) and for 'bad-metric' (must be Complete with empty MetricValues).

3) Cause 3: sagemaker.CreateTrialComponent -> iam.CreateRole with a trust policy allowing arn:aws:iam::<account>:root to sts:AssumeRole and NO attached permission policy -> sts.AssumeRole -> build a sagemaker-metrics client from the returned temporary credentials -> BatchPutMetrics for metric 'denied-metric' on that trial component. Emulator returns HTTP 200; AWS raises AccessDeniedException. Then BatchGetMetrics for 'denied-metric' must show Status=Complete with empty MetricValues, while the same write from the fully permissioned caller succeeds.
