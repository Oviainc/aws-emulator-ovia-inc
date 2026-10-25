Issue-Source: trace-sweep-divergence
Issue-Trace: autoscalingplans-absent-plan-read-and-write-envelopes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscalingplans-absent-plan-read-and-write-envelopes autoscalingplans-forecast-time-window-validation-envelopes
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: autoscaling-plans|DescribeScalingPlanResources|2xx->4xx|$.ScalingPlanResources
Issue-Key: autoscaling-plans|DescribeScalingPlanResources|2xx->4xx|status
Issue-Key: autoscaling-plans|GetScalingPlanResourceForecastData|2xx->4xx|$.Datapoints
Issue-Key: autoscaling-plans|GetScalingPlanResourceForecastData|2xx->4xx|status
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# autoscaling-plans read envelopes AWS answers 200 with an empty required collection are refused 400 ValidationException

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingPlanResources

## Description
Two autoscaling-plans read paths share one fault: a request the recording shows AWS answering with HTTP 200 and an EMPTY collection is instead turned into an HTTP 400 ValidationException, so the response carries no body member at all. In both cases the emulator treats "nothing to return" as "bad request". This bundle has 2 causes; BOTH must be closed before submitting. Both regress behavior a previously resolved issue had closed (20261022T112048Z-autoscaling-plans-wrong-status-createscalingplan-skips-instruction-validation.md Cause 2, and 20261023T071058Z-autoscaling-plans-getscalingplanresourceforecastdata-refuses-a-forecast-window-aws.md), and they are distinct from open issue 20261022T112048Z-autoscaling-plans-describe-envelopes-drop-status-and-policy.md, which concerns under-populated 200->200 describe envelopes and does not own either replay key.

Cause 1 of 2: DescribeScalingPlanResources for a scaling plan that does not exist answers 400 instead of 200 with an empty ScalingPlanResources list. Fixture autoscalingplans-absent-plan-read-and-write-envelopes, step task#2.
AWS: DescribeScalingPlanResources models NO not-found error. Smithy: DescribeScalingPlanResources input ScalingPlanName (@required, @length 1..128, pattern ^[\p{Print}&&[^|:/]]+$) and ScalingPlanVersion (@required, long); errors = ConcurrentUpdateException (server, httpError 500), InternalServiceException (server, 500), InvalidNextTokenException (client, 400), ValidationException (client, 400) — ObjectNotFoundException is modelled on the mutating operations, deliberately not on this read. Output member ScalingPlanResources is a non-required list. The accept/refuse boundary for an absent plan is therefore recorded behavior only, and the recording for this step is HTTP 200 whose ScalingPlanResources is an empty list.
Emulator: HTTP 400 for the same request, and because the body is an error envelope the ScalingPlanResources member is absent entirely (replay detail verbatim: "status: expected 200, got 400" / "$.ScalingPlanResources: missing (expected [], got None)").
Why: polling a plan's resources is the normal way a client waits for a freshly created plan to materialize or for a deleted one to drain. Because no not-found error is modelled for this operation, SDK clients have no typed branch for it, so the 400 surfaces as an unexpected, unretryable caller error instead of "no resources yet" and create/teardown convergence loops abort.
Fix: the DescribeScalingPlanResources handler's lookup path — a miss on (ScalingPlanName, ScalingPlanVersion) must fall through to the success serializer with an empty ScalingPlanResources list instead of being mapped onto a ValidationException.
Traces: autoscalingplans-absent-plan-read-and-write-envelopes#task#2 (cluster size 1).

Cause 2 of 2: GetScalingPlanResourceForecastData refuses forecast requests inside AWS's accepted envelope with 400 instead of answering 200 with an empty Datapoints list — now across four steps of the envelope-probe fixture. Fixture autoscalingplans-forecast-time-window-validation-envelopes, step task#1 (representative).
AWS: Smithy: GetScalingPlanResourceForecastData input members ScalingPlanName, ScalingPlanVersion, ServiceNamespace, ResourceId, ScalableDimension, ForecastDataType, StartTime, EndTime are all @required; errors = ValidationException (client, httpError 400) and InternalServiceException (server, 500) only; output member Datapoints is a @required list. The window limits are prose-only (seven-day maximum span, StartTime at most 56 days in the past) and the model states no further accept/refuse boundary, so which envelopes are accepted is recorded behavior only. The recording for these steps is HTTP 200 with Datapoints = [] — a forecast that exists but has no computed data points, not a validation failure. Other steps of the same fixture are recorded as 400 and the emulator matches them, which shows the corpus is probing the boundary and these four steps sit on the accept side of it.
Emulator: HTTP 400 for all four, with the @required Datapoints member absent from the error body (replay detail verbatim: "status: expected 200, got 400" / "$.Datapoints: missing (expected [], got None)").
Why: an empty Datapoints list means "no predicted values yet" and is the normal answer while a predictive-scaling forecast has not been computed, so callers poll on it and treat it as success. A 400 ValidationException is an unretryable caller error that SDK retry policies will not retry, so forecast pollers and dashboards report a configuration failure for a legal request, and a client that validates its own window against the emulator concludes a legal window is illegal.
Fix: the GetScalingPlanResourceForecastData handler's request-only validation chain (the forecast-window checks and the namespace/scalable-dimension/resource-id check, and the order they run in) must refuse exactly the envelopes AWS refuses and fall through to the empty-Datapoints 200 answer for the rest; in particular a refusal must not be decided by the serving process's wall clock where the recorded accept/refuse boundary is a property of the request alone. The 200 path already returns the empty required list correctly.
Traces: autoscalingplans-forecast-time-window-validation-envelopes#task#1 (representative), plus the same cluster's task#5, task#6 and task#7 — affected_count 4.

## Evidence
Replay summary: trace_set simparity-wire-ga-prod-v1, emulator_up true (health 200, data listener accepts), bootstrap_signal false, 3 fixtures replayed, 22 steps total, 17 passed, 5 mismatched, 0 errored, 0 blocked, score 77.3%, divergence_count 2. Both clusters report stub_path "" and owned_by [] — neither is blocked by an unimplemented operation nor owned by an open issue, so both are net-new.

Cause 1 — fixture autoscalingplans-absent-plan-read-and-write-envelopes, step task#2, operation DescribeScalingPlanResources, verdict mismatch. Expected (recorded AWS response): HTTP 200 with $.ScalingPlanResources = []. Actual (emulator): HTTP 400 with $.ScalingPlanResources absent. Replay detail verbatim: "status: expected 200, got 400\n$.ScalingPlanResources: missing (expected [], got None)". Divergence key: service=autoscaling-plans|DescribeScalingPlanResources|status=200->400|keys=$.ScalingPlanResources,status. affected_count 1 (autoscalingplans-absent-plan-read-and-write-envelopes#task#2).

Cause 2 — fixture autoscalingplans-forecast-time-window-validation-envelopes, step task#1, operation GetScalingPlanResourceForecastData, verdict mismatch. Expected: HTTP 200 with $.Datapoints = []. Actual: HTTP 400 with $.Datapoints absent. Replay detail verbatim: "status: expected 200, got 400\n$.Datapoints: missing (expected [], got None)". Divergence key: service=autoscaling-plans|GetScalingPlanResourceForecastData|status=200->400|keys=$.Datapoints,status. affected_count 4: autoscalingplans-forecast-time-window-validation-envelopes#task#1, #task#5, #task#6, #task#7.

Model evidence (Smithy, com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService): DescribeScalingPlanResources errors = ConcurrentUpdateException(500), InternalServiceException(500), InvalidNextTokenException(400), ValidationException(400) — no not-found error; output ScalingPlanResources is a non-required list. GetScalingPlanResourceForecastData errors = ValidationException(400), InternalServiceException(500); output Datapoints is @required.

Not asserted as requirements: the ValidationException $.Message wording on the steps where status and error code already match, and any generated id or timestamp in the recorded bodies.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout (/__admin/health answers 200 and the data listener accepts).
2. Replay the simparity-wire-ga-prod-v1 wire corpus against it.
3. Cause 1: replay fixture autoscalingplans-absent-plan-read-and-write-envelopes and stop at step task#2 — DescribeScalingPlanResources for a ScalingPlanName that was never created, with its ScalingPlanVersion. Observe HTTP 400 with no ScalingPlanResources member; expected HTTP 200 with ScalingPlanResources = [].
4. Cause 2: replay fixture autoscalingplans-forecast-time-window-validation-envelopes. Its recorded-400 steps still pass. Stop at step task#1 (and likewise task#5, task#6, task#7), whose recorded response is HTTP 200 with Datapoints: [], and issue that step's GetScalingPlanResourceForecastData request. Observe HTTP 400 ValidationException with no Datapoints member; expected HTTP 200 with Datapoints = [].
5. Both causes are closed only when all five steps (task#2 of the absent-plan fixture and task#1/#5/#6/#7 of the forecast fixture) replay clean.
