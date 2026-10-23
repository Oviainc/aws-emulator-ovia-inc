Issue-Source: trace-sweep-divergence
Issue-Trace: autoscalingplans-forecast-time-window-validation-envelopes
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscalingplans-forecast-time-window-validation-envelopes
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: autoscaling-plans|GetScalingPlanResourceForecastData|2xx->4xx|$.Datapoints
Issue-Key: autoscaling-plans|GetScalingPlanResourceForecastData|2xx->4xx|status
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# autoscaling-plans GetScalingPlanResourceForecastData refuses a forecast window AWS accepts

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetScalingPlanResourceForecastData

## Description
The emulator's pre-lookup refusal chain for GetScalingPlanResourceForecastData is stricter than AWS at one of the forecast-window/resource envelopes the corpus probes: a request the recording shows AWS accepting is turned into a 400 ValidationException instead of a successful, empty forecast. This issue has 1 cause.

Cause 1 of 1: a forecast request inside AWS's accepted envelope is refused with HTTP 400 ValidationException where AWS answers HTTP 200 with an empty Datapoints list.
AWS: GetScalingPlanResourceForecastData models every input member as required and declares exactly two errors, ValidationException (client, HTTP 400) and InternalServiceException (server, HTTP 500); its output declares Datapoints as a REQUIRED list (Smithy: operation GetScalingPlanResourceForecastData; shape ValidationException doc "An exception was thrown for a validation issue. Review the parameters provided."). The model documents the window limits in prose only - EndTime is "the exclusive end time" with "maximum time duration between the start and end time is seven days", StartTime is "the inclusive start time" and "can be at most 56 days before the current date and time" - and documents no success/failure boundary beyond that. Which envelopes AWS accepts versus refuses is therefore recorded behavior only, and the recording for this step shows AWS answering the request with HTTP 200 and Datapoints: [] - i.e. a forecast that exists but has no computed data points, NOT a validation failure. The other steps of the same fixture confirm AWS does refuse genuinely out-of-envelope windows with 400, so this step's 200 is a deliberate accept, not an absence of validation.
Emulator: HTTP 400 for the same request, and because the response is an error envelope the required Datapoints member is absent entirely (replay: "status: expected 200, got 400" and "$.Datapoints: missing (expected [], got None)"). The refusal is raised before the plan is ever looked up, by one of the request-only checks the handler runs in order: the ForecastDataType enum check, validate_forecast_window's three refusals (EndTime at or before StartTime, span wider than seven days, StartTime more than 56 days before the process's wall-clock now), and validate_forecast_resource's namespace/scalable-dimension/resource-id check, which admits only ServiceNamespace "autoscaling" with autoscaling:autoScalingGroup:DesiredCapacity. Hypothesis: the over-strict check is either the 56-day history refusal - it is evaluated against the serving process's current clock rather than against the request alone, so an absolute StartTime that was in-envelope stays in-envelope only while the process clock is near it - or validate_forecast_resource refusing a service namespace / scalable dimension the recording shows AWS accepting on this path. The replay reports only the status and the missing member, so which of the two fired is not established by this evidence; the defect is that the request is refused at all.
Why: a client that asks for a forecast over a window AWS serves gets a hard 400 ValidationException instead of a 200 with an empty forecast. These are not interchangeable: an empty Datapoints list means "no predicted values yet" and is the normal answer while a plan's forecast has not been computed, so callers poll on it and treat it as success. A 400 is an unretryable caller error, so SDK retry policies do not retry it, dashboards and predictive-scaling pollers surface a configuration failure for a perfectly valid request, and any code that validates its own window against the emulator before calling AWS concludes the window is illegal when it is not.
Fix: the GetScalingPlanResourceForecastData handler's request-only validation - validate_forecast_window / validate_forecast_resource and the order in which the handler applies them - must refuse exactly the envelopes AWS refuses and fall through to the empty-Datapoints 200 answer for the rest; in particular a refusal must not be decided by the serving process's wall clock where the recorded accept/refuse boundary is a property of the request. The 200 path itself is already correct in returning an empty required Datapoints list.
Traces: autoscalingplans-forecast-time-window-validation-envelopes#task#6.

Related but NOT covered here: the same fixture's steps task#2, task#3, task#4 and task#8 diverge only in the ValidationException $.Message text (status and error code match at 400); no documented or request-derived rule fixes that wording, so it is not filed as a defect. The three DescribeScalingPlans / DescribeScalingPlanResources divergences in this corpus are already owned by open issue 20261022T112048Z-autoscaling-plans-describe-envelopes-drop-status-and-policy.md.

## Evidence
Cause 1 - fixture autoscalingplans-forecast-time-window-validation-envelopes, step task#6, operation GetScalingPlanResourceForecastData.
Expected (recorded AWS response): HTTP 200 with body member Datapoints = [] (empty list).
Actual (emulator): HTTP 400. Replay detail verbatim: "status: expected 200, got 400" / "$.Datapoints: missing (expected [], got None)". Divergence key: service=autoscaling-plans|GetScalingPlanResourceForecastData|status=200->400|keys=$.Datapoints,status. Cluster size 1 (affected: autoscalingplans-forecast-time-window-validation-envelopes#task#6). No stub_path and no owning issue, so this failure is net-new.
Model evidence: Smithy operation GetScalingPlanResourceForecastData - output member Datapoints is @required; bound errors are ValidationException (HTTP 400, client) and InternalServiceException (HTTP 500, server) only; input docs state the seven-day maximum span and the 56-day history limit in prose.
Contrast within the same fixture: steps task#2, task#3, task#4 and task#8 of this fixture DO return 400 in both the recording and the emulator (they differ only in message text), which shows the corpus is probing the accept/refuse boundary and that task#6 sits on the accept side of it.
Summary context: emulator_up true, bootstrap_signal false, state_reset_failures 0, 25/33 steps passed - so this is a per-request refusal, not a service that is down or a fixture that ran against leaked state.

## Steps to Reproduce
1. Boot the emulator from this checkout (health endpoint answers 200 and the data listener accepts).
2. Replay fixture autoscalingplans-forecast-time-window-validation-envelopes from trace set simparity-wire-aws-v1 against it.
3. Let the fixture's earlier window-validation steps run (task#2, task#3, task#4 all legitimately return 400 in both recording and emulator).
4. Stop at step task#6, the step whose recorded response is HTTP 200 with Datapoints: []. Issue that step's GetScalingPlanResourceForecastData request.
5. Observe the emulator answers HTTP 400 with a ValidationException instead of 200, so the required Datapoints member is absent from the response body.
6. Expected: HTTP 200 and a body carrying Datapoints as an empty list.
