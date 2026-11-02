Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling-plans.asp-absent-plan-reads-are-empty-but-writes-are-rejected
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: auto-scaling-plans|GetScalingPlanResourceForecastData|ValidationException
Issue-Commit: 24dcbcf55b45bb9d0573a38e9c06262a825c087d

# REOPENS: GetScalingPlanResourceForecastData still refuses a self-consistent non-autoscaling resource triple with 400 ValidationException instead of answering 200 with an empty Datapoints list

> _Note: this issue was identified against the code base at commit `24dcbcf55b45bb9d0573a38e9c06262a825c087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetScalingPlanResourceForecastData

## Description
REOPENS 20261025T010903Z-autoscaling-plans-read-envelopes-aws-answers-200-with.md: its fix left the call refused.

That resolved issue's Cause 2 named exactly this code path — "the GetScalingPlanResourceForecastData handler's request-only validation chain (the forecast-window checks and the namespace/scalable-dimension/resource-id check, and the order they run in) must refuse exactly the envelopes AWS refuses and fall through to the empty-Datapoints 200 answer for the rest". The window side of that chain now behaves; the namespace/scalable-dimension side still refuses a request AWS answers 200 to. This issue has 1 cause.

Cause 1 of 1: a forecast read whose ServiceNamespace/ScalableDimension pair is self-consistent but is not the autoscaling/Auto-Scaling-group pair is refused outright.

AWS: Smithy models every coordinate member of GetScalingPlanResourceForecastData as a plain @required enum/string — ServiceNamespace is the enum {autoscaling, ecs, ec2, rds, dynamodb} and ScalableDimension is the enum {autoscaling:autoScalingGroup:DesiredCapacity, ecs:service:DesiredCount, ec2:spot-fleet-request:TargetCapacity, rds:cluster:ReadReplicaCount, dynamodb:table:ReadCapacityUnits, dynamodb:table:WriteCapacityUnits, dynamodb:index:ReadCapacityUnits, dynamodb:index:WriteCapacityUnits}; ResourceId is XmlString with only the printable-character pattern; the operation binds exactly ValidationException (client, httpError 400) and InternalServiceException (500), and its output member Datapoints is a @required list. The member prose ("The only valid value is autoscaling" / "The only valid value is autoscaling:autoScalingGroup:DesiredCapacity") describes which resources AWS actually FORECASTS for, and the emulator has read it as an accept/refuse boundary on the request. The accept/refuse boundary of this operation is recorded behavior only, and the recording disagrees: a forecast read carrying ServiceNamespace='dynamodb', ResourceId='table/<table>', ScalableDimension='dynamodb:table:ReadCapacityUnits', ForecastDataType='LoadForecast' over a one-day window is answered HTTP 200 with Datapoints = [] — "this plan has no forecast for that resource", exactly as for a plan that holds no instruction for the named resource. Hypothesis: AWS validates only that the triple is internally consistent (the dimension's namespace prefix matches ServiceNamespace, the ResourceId carries that dimension's resource-type prefix plus a unique identifier) and reserves the refusal "Unsupported service namespace, resource type or scalable dimension" for an inconsistent triple; it does not refuse a consistent non-ASG triple.

Emulator: HTTP 400, Error.Code 'ValidationException', Error.Message 'Unsupported service namespace, resource type or scalable dimension' (that whole sentence is fixed text — it interpolates nothing from the request). The crate hard-codes the single accepted pair as FORECAST_SERVICE_NAMESPACE = "autoscaling" and FORECAST_SCALABLE_DIMENSION = "autoscaling:autoScalingGroup:DesiredCapacity" and refuses anything else before the plan is ever looked up, so no dynamodb-, ecs-, ec2- or rds-dimension forecast read can ever reach the 200 path.

Why: the empty Datapoints list is the normal answer while a forecast has not been computed, so callers poll on it and treat it as success; ValidationException is a non-retryable caller error that no SDK retry policy retries. A client (or a convergence loop) that reads the forecast of a DynamoDB-backed scaling plan concludes its own legal request is malformed, and because this refusal is raised before anything else in the check, every later claim in the trace — the absent-plan read/write split (DescribeScalingPlans/DescribeScalingPlanResources 200-empty vs DeleteScalingPlan/UpdateScalingPlan ObjectNotFoundException 'No scaling plan found for scaling plan name: <name>, version: <version>'), the live-plan positive control, and DescribeScalingPlans refusing to mix ScalingPlanNames with ApplicationSources — is left unverified.

Fix: the GetScalingPlanResourceForecastData handler's request-only coordinate validation. The two single-value constants must stop being an admission gate for the request; the gate must be triple consistency (ScalableDimension's namespace prefix equals ServiceNamespace, ResourceId shaped for that dimension's resource type), with every consistent triple falling through to the normal lookup, which already answers 200 with an empty Datapoints list when no stored instruction names the resource.

Traces: auto-scaling-plans.asp-absent-plan-reads-are-empty-but-writes-are-rejected. The gate lives in services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/ops/get_scaling_plan_resource_forecast_data.rs:250-261 (the two constants) and is applied in that file's step-1 validation chain; the open maintainability issue 20261029T062738Z-auto-scaling-plans-getscalingplanresourceforecastdata-maintainability--r2.md and the open issue 20261027T211928Z-auto-scaling-plans-getscalingplanresourceforecastdata-error-response.md discuss the same triple check but neither owns this replay key (that one concerns the wording used for an OFF-ENUM value, not the refusal of an in-enum consistent triple).

## Evidence
Suite: simparity-behavioral-aws-v1. auto-scaling-plans 2/10 passed, 8 failed; 4 of the failures are OWNED by 20261022T175157Z / 20261022T093447Z and skipped.

Check auto-scaling-plans.asp-absent-plan-reads-are-empty-but-writes-are-rejected, verdict error:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GetScalingPlanResourceForecastData operation: Unsupported service namespace, resource type or scalable dimension

Request issued (check source, lines 162-166): get_scaling_plan_resource_forecast_data(ScalingPlanName='<a name never created>', ScalingPlanVersion=1, ServiceNamespace='dynamodb', ResourceId='table/<table>', ScalableDimension='dynamodb:table:ReadCapacityUnits', ForecastDataType='LoadForecast', StartTime=<top of the current hour>, EndTime=<+1 day>).

Expected (recorded AWS behavior, asserted at check line 167): HTTP 200 with Datapoints == [].
Actual: HTTP 400, Error.Code 'ValidationException', Error.Message 'Unsupported service namespace, resource type or scalable dimension'; the @required Datapoints member is absent because the body is an error envelope.

The same shape was already filed and resolved from the wire corpus: 20261025T010903Z (Issue-Key autoscaling-plans|GetScalingPlanResourceForecastData|2xx->4xx|$.Datapoints and |status) — the refusal survives its fix on the namespace/dimension branch of the chain.

Model evidence (services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/smithy-model/...json): GetScalingPlanResourceForecastData errors = ValidationException(client, 400), InternalServiceException(server, 500) — no not-found error; output Datapoints @required list; ServiceNamespace enum includes 'dynamodb'; ScalableDimension enum includes 'dynamodb:table:ReadCapacityUnits'.

Not asserted as a requirement: the generated plan/table names and the wall-clock StartTime/EndTime of the recording.

## Steps to Reproduce
1. Boot the emulator from this checkout and point the behavioral harness at it (service token 'auto-scaling-plans').
2. Call GetScalingPlanResourceForecastData with ScalingPlanName set to a name that was never created, ScalingPlanVersion=1, ServiceNamespace='dynamodb', ResourceId='table/<any-table-name>', ScalableDimension='dynamodb:table:ReadCapacityUnits', ForecastDataType='LoadForecast', StartTime=the top of the current hour, EndTime=StartTime+1 day.
3. Observe HTTP 400 ValidationException 'Unsupported service namespace, resource type or scalable dimension'; expected HTTP 200 with Datapoints = [].
4. Repeat against a plan that DOES exist and does carry a dynamodb:table:ReadCapacityUnits instruction for that ResourceId — still refused, so no DynamoDB-backed plan can have its forecast read at all.
5. Re-replay auto-scaling-plans.asp-absent-plan-reads-are-empty-but-writes-are-rejected; once the read is served, the rest of that check (absent-plan reads 200-empty, DeleteScalingPlan/UpdateScalingPlan ObjectNotFoundException 'No scaling plan found for scaling plan name: <name>, version: <version>', and DescribeScalingPlans refusing ScalingPlanNames together with ApplicationSources) is exercised for the first time and must also pass.
