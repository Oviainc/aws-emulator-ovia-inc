Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker-metrics.batch-get-metrics-malformed-resource-arn-rejected
Issue-Shape-Id: com.amazonaws.sagemakermetrics#SageMakerMetricsService
Issue-Key: sagemaker-metrics.batch-get-metrics-malformed-resource-arn-rejected|run#13|AssertionError|assert message == "Invalid SageMaker ARN", f"expected message 'Invalid SageMaker ARN', got {message!r}"
Issue-Commit: 37be8999abc11e440bb26a3acee60c47f75abac5

# sagemaker-metrics: BatchGetMetrics returns the generic Smithy constraint-violation text instead of AWS's 'Invalid SageMaker ARN' for a malformed MetricQuery.ResourceArn

> _Note: this issue was identified against the code base at commit `37be8999abc11e440bb26a3acee60c47f75abac5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetMetrics

## Description
One root cause, grounded in one failing behavioral check. Note: open issue 20261022T212853Z-sagemaker-metrics-batchgetmetrics-skips-per-query-axis-period.md mentions this check in prose under 'Not filed, for the record' and explicitly declined to treat it as a defect, but it does NOT own the check (the check id appears in neither its Issue-Trace-Ids nor any Issue-Key), and the replay report does not mark this failure as filed or owned. An error message is part of the contract, so it is filed here.

Cause 1 of 1: BatchGetMetrics request-level ValidationException carries the wrong message for a malformed ResourceArn (failing check: sagemaker-metrics.batch-get-metrics-malformed-resource-arn-rejected).

AWS: Smithy (services/service-com-amazonaws-sagemakermetrics-sagemakermetricsservice/smithy-model) models BatchGetMetrics.MetricQueries as a required MetricQueryList @length min 1 max 100, and MetricQuery.ResourceArn as a required SageMakerResourceArn @length min 0 max 2048 @pattern '^arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:[a-z\\-].*/'. BatchGetMetrics binds no operation-level errors, so ValidationException is a service-common/framework error. The model states the pattern but not the message, so for the message text this is recorded behavior only: when any MetricQuery in the list has a ResourceArn that is not a well-formed SageMaker ARN, AWS fails the ENTIRE call with error code ValidationException whose message is the constant string 'Invalid SageMaker ARN'. That message is ALL fixed text: it has no variable parts, so it does not embed the offending value, the member path, the list index or the pattern. This is distinct from a per-query SEMANTIC failure (e.g. a mismatched axis/period), which AWS reports in-band as MetricQueryResult.Status=ValidationError inside an otherwise-200 response.

Emulator: the status and the error code are correct — the whole call IS rejected with code ValidationException — but the message is the generic Smithy constraint-violation text produced by the framework's pattern validator: "1 validation error detected: Value 'not-an-arn' at 'metricQueries.2.member.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:[a-z\\-].*/". Of that text, the fixed parts are '<N> validation error detected: Value ... at ... failed to satisfy constraint: Member must satisfy regular expression pattern: ...'; the variable parts it interpolates are the rejected value from the request ('not-an-arn'), the member path with the 1-based list index ('metricQueries.2.member.resourceArn') and the modeled pattern. AWS interpolates none of these for this error class — it emits only the constant 'Invalid SageMaker ARN'.

Why: the error message is a requirement of the error class, not incidental prose. A client (or a test, or a log-matching/alerting rule) that distinguishes a malformed-ARN rejection from other ValidationExceptions by its message sees a completely different string against the emulator, and a client that parses the emulator's message for a member path and offending value finds structure that real AWS never sends. Conversely, anything developed against the emulator's verbose text breaks in production, where the message is the bare constant.

Fix: BatchGetMetrics' input validation / error-serialization path for sagemaker-metrics — this operation's ResourceArn pattern failure must not fall through to the shared generic Smithy constraint-violation message builder; it must raise the service-specific ValidationException with message exactly 'Invalid SageMaker ARN' while keeping the existing correct behavior (whole-call rejection, error code ValidationException) and keeping the per-query in-band Status=ValidationError channel for semantic failures.

Traces: sagemaker-metrics.batch-get-metrics-malformed-resource-arn-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1: 15/19 passed, 4 mismatches, all in sagemaker-metrics. Three were already owned by 20261022T212853Z-sagemaker-metrics-batchgetmetrics-skips-per-query-axis-period.md and skipped; this is the fourth, unowned.

Cause 1 — sagemaker-metrics.batch-get-metrics-malformed-resource-arn-rejected:
  sagemaker.CreateTrialComponent -> trial-component ARN <tc arn>.
  BatchPutMetrics(TrialComponentName=<tc>, MetricData=[{MetricName:'ranged', Timestamp:now, Step:0, Value:4.0}]) -> no Errors.
  Positive control passed: BatchGetMetrics(MetricQueries=[{MetricName:'ranged', ResourceArn:<tc arn>, MetricStat:'Last', Period:'IterationNumber', XAxisType:'IterationNumber'}]) -> MetricQueryResults[0].Status == 'Complete', MetricValues[0] == 4.0.
  Then BatchGetMetrics(MetricQueries=[<that valid query>, dict(<valid query>, ResourceArn='not-an-arn')]).

  Expected (AWS): the whole call is refused with Error.Code == 'ValidationException' and Error.Message == 'Invalid SageMaker ARN' (constant text, no variable parts).
  Actual (emulator): the whole call IS refused and Error.Code == 'ValidationException' (both correct), but Error.Message == "1 validation error detected: Value 'not-an-arn' at 'metricQueries.2.member.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:[a-z\\-].*/".
  Assertion: AssertionError: expected message 'Invalid SageMaker ARN', got "1 validation error detected: Value 'not-an-arn' at 'metricQueries.2.member.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:[a-z\\-].*/" (check_sagemaker_metrics_batch_get_metrics_malformed_resource_arn_rejected.py line 50).

The divergence is not a per-run value: the expected message is wholly fixed text, and the emulator's differs structurally (it interpolates the request value, the member path and the modeled pattern), not merely in an id, name, number or timestamp.

Model facts used: smithy_query operation=BatchGetMetrics -> input MetricQueries: MetricQueryList, required, @length 1..100; errors: [] (ValidationException is service-common); http POST /BatchGetMetrics code 200. smithy_query shape=MetricQuery,SageMakerResourceArn -> MetricQuery.ResourceArn required, target SageMakerResourceArn, @length 0..2048, @pattern '^arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:[a-z\\-].*/'.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, or drive it directly with botocore against the emulator endpoint:

1) sagemaker.CreateTrialComponent(TrialComponentName=<unique>) and keep the returned TrialComponentArn.
2) sagemaker-metrics.BatchPutMetrics(TrialComponentName=<name>, MetricData=[{MetricName:'ranged', Timestamp:<now>, Step:0, Value:4.0}]); assert no Errors.
3) Positive control: sagemaker-metrics.BatchGetMetrics(MetricQueries=[{MetricName:'ranged', ResourceArn:<tc arn>, MetricStat:'Last', Period:'IterationNumber', XAxisType:'IterationNumber'}]) and poll until MetricQueryResults[0].Status == 'Complete' with MetricValues == [4.0]. This already passes and must keep passing.
4) Call sagemaker-metrics.BatchGetMetrics(MetricQueries=[<the valid query from step 3>, <the same query but ResourceArn='not-an-arn'>]) and catch the botocore ClientError.
5) Inspect the error: Error.Code is 'ValidationException' on both emulator and AWS (correct), but compare Error.Message. Emulator returns the generic "1 validation error detected: Value 'not-an-arn' at 'metricQueries.2.member.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ..."; AWS returns exactly 'Invalid SageMaker ARN'. Any non-ARN string in any MetricQuery of the list reproduces it, at any index, and the expected message does not change with the value or the index.
6) Regression guard: the malformed ARN must still fail the WHOLE call (not resolve to a per-query MetricQueryResult.Status), and a well-formed-but-semantically-invalid query must still be reported in-band as Status=ValidationError inside a 200.
