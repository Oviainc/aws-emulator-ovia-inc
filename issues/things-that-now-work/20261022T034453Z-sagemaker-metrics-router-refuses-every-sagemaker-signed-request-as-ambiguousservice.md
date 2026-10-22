Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker-metrics.batch-get-metrics-malformed-resource-arn-rejected sagemaker-metrics.batch-get-metrics-mismatched-axis-period-rejected sagemaker-metrics.batch-get-metrics-unknown-metric-name-empty-not-error sagemaker-metrics.batch-metrics-empty-range-no-data-complete sagemaker-metrics.batch-metrics-iteration-number-ordering sagemaker-metrics.batch-metrics-max-batch-size-boundary sagemaker-metrics.batch-metrics-multi-query-index-alignment sagemaker-metrics.batch-metrics-round-trip-basic sagemaker-metrics.batch-metrics-stat-count-aggregation sagemaker-metrics.batch-metrics-stat-last-latest-timestamp sagemaker-metrics.batch-metrics-stat-max-aggregation sagemaker-metrics.batch-metrics-stat-min-aggregation sagemaker-metrics.batch-metrics-step-optional-timestamp-axis sagemaker-metrics.batch-metrics-timestamp-axis-multi-point-ordering sagemaker-metrics.batch-metrics-training-loop-e2e sagemaker-metrics.batch-metrics-trial-component-isolation sagemaker-metrics.batch-put-metrics-concurrent-same-point-conflict sagemaker-metrics.batch-put-metrics-invalid-value-rejected sagemaker-metrics.batch-put-metrics-missing-iam-permission-rejected
Issue-Shape-Id: com.amazonaws.sagemakermetrics#SageMakerMetricsService
Issue-Key: sagemaker|CreateTrialComponent|AmbiguousService
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# Router refuses every sagemaker-signed request as AmbiguousService (9-way 'sagemaker' signing-name collision), making SageMaker Metrics entirely unreachable

> _Resolution note (added when this report was moved): the cause above — the
> `AmbiguousService` refusal — is fixed. Each of the nine services that sign as
> `sagemaker` now claims only the wire surface its own model publishes (its URI path
> space, or the service its `X-Amz-Target` names for the awsJson1_1 control plane), so
> SageMaker Metrics and the SageMaker control plane are reachable and the metrics
> put/get round trip is pinned end-to-end in
> `core/router/tests/all/sagemaker_signing_name_routing.rs`._
>
> _What is NOT fixed here: the 19 trace ids above each start with the control plane's
> `CreateTrialComponent`, which now REACHES that service and answers the scaffold's
> `501 NotImplemented` — it is still one of that crate's `//! SCAFFOLDED, NOT
> IMPLEMENTED:` stubs. Those checks therefore stay red until that stub is implemented
> (its own item in the stub census, `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:'
> services/*/src/ops/`), for a different reason than the one this report is about._

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetMetrics

## Description
This issue has 1 cause. It must be closed before submitting.

Cause 1 of 1: the shared request router resolves an incoming request to an emulated service by its sigv4 signing name alone, and nine emulated SageMaker-family services share the signing name 'sagemaker'. Every request signed for 'sagemaker' is therefore refused with a synthetic 'AmbiguousService' error instead of being routed, so both SageMaker Metrics itself and the SageMaker control-plane call its checks use for setup (CreateTrialComponent) are unreachable. All 19 behavioral checks of sagemaker-metrics fail on this single refusal, at the first API call of each check.

AWS: SageMaker Metrics is a real service reachable on its own endpoint. Smithy (services/service-com-amazonaws-sagemakermetrics-sagemakermetricsservice, query=service_info): sdkId 'SageMaker Metrics', apiVersion 2022-09-30, protocol restJson1, authSchemes [sigv4], sigv4Name 'sagemaker', arnNamespace 'sagemaker', endpointPrefix 'metrics.sagemaker'. Its two bound operations are BatchPutMetrics (PUT /BatchPutMetrics, required TrialComponentName: ExperimentEntityName @length 1..120 @pattern ^[a-z0-9](-*[a-z0-9]){0,119}$; required MetricData: RawMetricDataList @length 1..10) and BatchGetMetrics (POST /BatchGetMetrics, required MetricQueries: MetricQueryList @length 1..100; output MetricQueryResults @length 1..100), each with httpResponseCode 200. The SageMaker control plane (sdkId 'SageMaker', sigv4Name 'sagemaker') answers CreateTrialComponent with 200 and a TrialComponentArn. Neither model, nor any AWS model, defines an error named 'AmbiguousService'; signing name is not how AWS distinguishes these services — the endpoint host (metrics.sagemaker.<region>.amazonaws.com vs api.sagemaker.<region>.amazonaws.com), the protocol and the URI/target are.

Emulator: every call returns the modeled-error-shaped failure 'AmbiguousService' with message "9 emulated services all claim requests for 'sagemaker': sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker; the request is rejected rather than served by an arbitrary one" (recorded message is an example of the router's refusal text; the error CODE AmbiguousService is the divergence). botocore classifies it as retryable and exhausts retries ('reached max retries: 3'). Observed on CreateTrialComponent in all 19 checks, which is the first call every check makes; because routing never resolves, BatchPutMetrics/BatchGetMetrics were never exercised at all, so no statement can be made yet about the metrics data-plane semantics the 19 checks target (axis/period validation, per-query Status=ValidationError vs request-level ValidationException, stat aggregation, ordering, batch-size boundary, trial-component isolation).

Why: a client gets a non-AWS error code for every operation, so no error branch matches and SDK retry logic retries a request that can never succeed. SageMaker Metrics and the SageMaker control plane are both 100% unusable — not just on malformed input — so every training-metrics workload fails at the first call.

Fix: the shared request router / service-registry claim resolution — disambiguate claimants by endpointPrefix and the request host/URI (and protocol + operation target), not by sigv4 signing name alone, so 'metrics.sagemaker' resolves to SageMaker Metrics and 'api.sagemaker' to the SageMaker control plane. The refusal path must also stop being reachable for a legitimately resolvable request. Once routing lands, BatchPutMetrics/BatchGetMetrics input validation and the metrics state/aggregation behavior the 19 checks assert must be re-replayed, because they have never run.

Related open issue (does NOT cover these checks): '20261021T234924Z-featurestore-runtime-sagemaker-router-refuses-every-featurestore-runtime-sagemaker-request-as' reports the same router defect from the WIRE corpus for the featurestore-runtime.sagemaker claimant. It carries replay keys only for sagemaker-featurestore-runtime fixtures and none of the sagemaker-metrics behavioral checks, so closing it will not close these 19. A fix that generalises claim resolution should close both.

Traces: all 19 failing checks listed under trace_ids, each failing at its first call (CreateTrialComponent).

## Evidence
Behavioral suite simparity-behavioral-aws-v1: sagemaker-metrics 0/19 passed (0%), 19 errored, 0 mismatched, 0 skipped, 0 unloadable; no check marked ALREADY FILED or OWNED by.

All 19 checks error identically:
  botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateTrialComponent operation (reached max retries: 3): 9 emulated services all claim requests for 'sagemaker': sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker, sagemaker; the request is rejected rather than served by an arbitrary one

Expected: CreateTrialComponent -> 200 with TrialComponentArn; then BatchPutMetrics -> 200 (Errors empty) and BatchGetMetrics -> 200 with MetricQueryResults aligned to MetricQueries. Actual: error code AmbiguousService for every call, so neither metrics operation is ever invoked.

Smithy evidence:
- SageMaker Metrics service_info: sdkId 'SageMaker Metrics', version 2022-09-30, restJson1, sigv4Name 'sagemaker', arnNamespace 'sagemaker', endpointPrefix 'metrics.sagemaker'.
- BatchGetMetrics: POST /BatchGetMetrics, code 200, input MetricQueries (required, MetricQueryList, @length 1..100), output MetricQueryResults (@length 1..100), no operation-level errors modeled.
- BatchPutMetrics: PUT /BatchPutMetrics, code 200, input TrialComponentName (required, ExperimentEntityName @length 1..120 @pattern ^[a-z0-9](-*[a-z0-9]){0,119}$) and MetricData (required, RawMetricDataList @length 1..10), output Errors.
- No AWS Smithy model defines an error named 'AmbiguousService'.

The nine colliding sigv4Name='sagemaker' claimants present in the checkout: sagemaker, sagemaker-runtime, sagemaker-runtime-http2, sagemaker-a2i-runtime, sagemaker-featurestore-runtime, sagemaker-metrics, sagemaker-edge, sagemaker-geospatial, sagemaker-training-session-runtime (count matches the router's own '9 emulated services' text).

## Steps to Reproduce
1. Clone the repo and boot the emulator (it is up: it answers with an AWS-shaped error document, bootstrap_signal false).
2. Point a botocore 'sagemaker' client at the emulator and call CreateTrialComponent(TrialComponentName='sim2real-behav-tc-xxxx').
3. Observe the call fails with error code AmbiguousService and message "9 emulated services all claim requests for 'sagemaker': ...", retried to exhaustion, instead of 200 with a TrialComponentArn.
4. Point a 'sagemaker-metrics' client (endpointPrefix metrics.sagemaker, restJson1, signing name 'sagemaker') at the emulator and call BatchPutMetrics(TrialComponentName=<name>, MetricData=[{MetricName:'ranged',Timestamp:<now>,Step:0,Value:4.0}]) and then BatchGetMetrics(MetricQueries=[{MetricName:'ranged',ResourceArn:<tc arn>,MetricStat:'Last',Period:'IterationNumber',XAxisType:'IterationNumber'}]). Observe the same AmbiguousService refusal for both, confirming the service is unreachable rather than mis-implemented.
5. Replay the behavioral suite and re-run all 19 sagemaker-metrics checks listed in trace_ids to confirm the fix — none of them has ever reached a metrics operation, so each must pass on its own semantics after routing is fixed.
