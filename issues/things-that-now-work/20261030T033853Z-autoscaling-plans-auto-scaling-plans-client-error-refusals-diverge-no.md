Issue-Source: trace-sweep-divergence
Issue-Trace: autoscalingplans-create-scaling-plan-structural-validation-envelopes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscalingplans-create-scaling-plan-structural-validation-envelopes autoscalingplans-pagination-and-limit-validation-envelopes
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: autoscaling-plans|CreateScalingPlan|4xx->2xx|$.__type
Issue-Key: autoscaling-plans|CreateScalingPlan|4xx->2xx|$.message
Issue-Key: autoscaling-plans|CreateScalingPlan|4xx->2xx|status
Issue-Key: autoscaling-plans|CreateScalingPlan|4xx->4xx|$.Message
Issue-Key: autoscaling-plans|DescribeScalingPlans|4xx->4xx|$.Message
Issue-Key: autoscaling-plans|DescribeScalingPlanResources|4xx->4xx|$.Message
Issue-Commit: e8c5c92d9bc397e4014b473476bd2d32121bdaea

# Auto Scaling Plans client-error refusals diverge: no scalingPlanName pattern check and emulator-authored validation messages

> _Note: this issue was identified against the code base at commit `e8c5c92d9bc397e4014b473476bd2d32121bdaea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateScalingPlan

## Description
The Auto Scaling Plans crate's client-error path diverges from AWS in three independent ways: one input constraint is not enforced at all (the call succeeds where AWS refuses it), and two refusal messages are emulator-authored prose rather than AWS's wording. This issue has 4 causes.

Cause 1 of 4: CreateScalingPlan does not enforce the ScalingPlanName pattern, so a name containing a forbidden character is accepted with 200 instead of being refused.
AWS: Smithy: CreateScalingPlan.ScalingPlanName -> ScalingPlanName, @required, @length min 1 max 128, @pattern "^[\\p{Print}&&[^|:/]]+$" (the three characters '|', ':' and '/' are excluded from the otherwise-printable set); CreateScalingPlan binds ValidationException (client, httpError 400). A request whose ScalingPlanName violates that pattern is refused HTTP 400 with the Smithy constraint-violation envelope, whose keys are "__type" and lowercase "message" (not the "Message" key the hand-raised service errors in this API use). The message is AWS's standard constraint template: the fixed text is "<n> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; the variable parts are the error count, the offending value echoed from the request, the lowerCamel member name as the service names it ('scalingPlanName'), and the pattern body as the model states it ([\\p{Print}&&[^|:/]]+). The recorded refusal is __type 'ValidationException' and message "1 validation error detected: Value 'bad|name' at 'scalingPlanName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\p{Print}&&[^|:/]]+" — 'bad|name' there is the request's own value filling the template, not part of it.
Emulator: HTTP 200 with a normal CreateScalingPlanResponse; no __type and no message member are emitted at all, and the plan is created under the illegal name.
Why: '|', ':' and '/' are excluded because the service uses them as delimiters in the identifiers it derives from a plan name (policy names, resource keys, ARNs). A client that AWS would have stopped at the API boundary instead gets a plan whose name poisons every derived identifier, and a caller testing its own input validation against the emulator sees malformed names pass. Because the pattern is also unenforced, the error is latent rather than immediate.
Fix: CreateScalingPlan's request validation — the ScalingPlanName member needs its modeled @pattern (and @length) enforced before the plan is stored, and the resulting refusal must be serialized in the constraint-violation envelope ("__type"/"message"), which is a different envelope from the "Message"-keyed service errors this crate already emits.
Traces: autoscalingplans-create-scaling-plan-structural-validation-envelopes#task#1.

Cause 2 of 4: CreateScalingPlan's empty-ScalingInstructions refusal uses emulator wording instead of AWS's.
AWS: Smithy: CreateScalingPlan.ScalingInstructions -> ScalingInstructions, @required list of ScalingInstruction, with no @length on the list, so the "at least one instruction" rule is recorded behavior only; the operation binds ValidationException (client, 400). AWS refuses an empty list with HTTP 400, ValidationException, and the message "Must provide at least 1 scaling instruction" — that whole sentence is fixed text and interpolates nothing from the request.
Emulator: HTTP 400 ValidationException (status and code correct) with Message 'ScalingInstructions must specify at least one scaling instruction'.
Why: the status and code are right, so SDK-level error typing works, but the message text is part of the contract: clients, log scrapers and assertion suites that match on AWS's sentence see no match, and anything surfacing the message to an operator shows wording AWS never emits.
Fix: the CreateScalingPlan handler's ScalingInstructions validation — replace the authored sentence with AWS's fixed text.
Traces: autoscalingplans-create-scaling-plan-structural-validation-envelopes#task#2 (and the two sibling steps clustered with it, task#3 and task#4).

Cause 3 of 4: DescribeScalingPlans' invalid-NextToken refusal uses emulator wording instead of AWS's.
AWS: Smithy: DescribeScalingPlans.NextToken -> NextToken; the operation binds InvalidNextTokenException (client, httpError 400, doc "The token provided is not valid") alongside ValidationException. For a NextToken the service cannot decode, AWS answers HTTP 400 with the message "The NextToken value is not valid" — fixed text in full; it does not echo the rejected token.
Emulator: HTTP 400 (status and envelope key correct) with Message 'The NextToken provided is not valid'.
Why: same class of breakage as Cause 2 — the only thing distinguishing a bad continuation token from the other 400s this operation can raise, for any caller that reads the message, is text the emulator words differently.
Fix: the DescribeScalingPlans handler's pagination-token validation (the message constant it raises its invalid-token refusal with).
Traces: autoscalingplans-pagination-and-limit-validation-envelopes#task#1 (and the sibling step clustered with it, task#3).

Cause 4 of 4: DescribeScalingPlanResources' invalid-NextToken refusal has the same wrong wording, from its own handler.
AWS: Smithy: DescribeScalingPlanResources.NextToken -> NextToken; the operation binds InvalidNextTokenException (client, httpError 400) and ValidationException (client, 400). The invalid-token refusal is HTTP 400 with the same fixed message "The NextToken value is not valid".
Emulator: HTTP 400 with Message 'The NextToken provided is not valid'.
Why: as Cause 3. It is listed separately because it is a second copy of the wrong sentence on a different operation's code path, so fixing only DescribeScalingPlans leaves this one diverging.
Fix: the DescribeScalingPlanResources handler's pagination-token validation; both operations should share one message source so the two cannot drift apart again.
Traces: autoscalingplans-pagination-and-limit-validation-envelopes#task#2.

Not asserted as requirements anywhere above: the generated plan names in the fixtures, and any request id or clock value in the recorded envelopes.

## Evidence
Corpus simparity-wire-aws-v1, replayed against the emulator booted from this checkout (score 77.6%, 11 of 49 steps mismatched, all in autoscaling-plans).

Cause 1 — fixture autoscalingplans-create-scaling-plan-structural-validation-envelopes step task#1, CreateScalingPlan, verdict mismatch:
  status: expected 400, got 200
  $.__type: missing (expected 'ValidationException', got None)
  $.message: missing (expected "1 validation error detected: Value 'bad|name' at 'scalingPlanName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\p{Print}&&[^|:/]]+", got None)
Fixed text of that message: "<n> validation error detected: Value '<v>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Variable parts: the count (1), the request's own ScalingPlanName value ('bad|name'), the member name as the service spells it ('scalingPlanName'), and the model's pattern body.

Cause 2 — same fixture, step task#2 (cluster also covers task#3, task#4), CreateScalingPlan, status 400 vs 400:
  $.Message: value_mismatch (expected 'Must provide at least 1 scaling instruction', got 'ScalingInstructions must specify at least one scaling instruction')

Cause 3 — fixture autoscalingplans-pagination-and-limit-validation-envelopes step task#1 (cluster also covers task#3), DescribeScalingPlans, status 400 vs 400:
  $.Message: value_mismatch (expected 'The NextToken value is not valid', got 'The NextToken provided is not valid')

Cause 4 — same fixture, step task#2, DescribeScalingPlanResources, status 400 vs 400:
  $.Message: value_mismatch (expected 'The NextToken value is not valid', got 'The NextToken provided is not valid')

Model evidence (services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/smithy-model/com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice.json): CreateScalingPlan.ScalingPlanName @required @length(1,128) @pattern ^[\\p{Print}&&[^|:/]]+$; CreateScalingPlan.ScalingInstructions @required list, no @length; CreateScalingPlan errors = ConcurrentUpdateException(500), InternalServiceException(500), LimitExceededException(400), ValidationException(400); DescribeScalingPlans and DescribeScalingPlanResources both bind InvalidNextTokenException(client, 400, "The token provided is not valid") and ValidationException(400).

No open issue owns these replay keys (every cluster reported owned_by: [] and an empty stub_path). The open issue 20261029T193125Z-auto-scaling-plans-reopens-getscalingplanresourceforecastdata-still-refuses-a-self-consistent-non.md concerns GetScalingPlanResourceForecastData's coordinate gate, a different operation and a different code path; the open issue 20261029T211352Z-autoscaling-auto-scaling-client-error-refusals-diverge-emulator-authored.md is the EC2 Auto Scaling service, not autoscaling-plans.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an autoscaling-plans client at it.
2. Cause 1: call CreateScalingPlan with a ScalingPlanName containing a '|' (e.g. 'bad|name'), a valid ApplicationSource and one scaling instruction. Observe HTTP 200 and a created plan; expected HTTP 400 with body {"__type":"ValidationException","message":"1 validation error detected: Value 'bad|name' at 'scalingPlanName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\p{Print}&&[^|:/]]+"}. Repeat with ':' and '/' — both must also be refused.
3. Cause 2: call CreateScalingPlan with a legal name and ScalingInstructions=[]. Observe HTTP 400 ValidationException with Message 'ScalingInstructions must specify at least one scaling instruction'; expected Message 'Must provide at least 1 scaling instruction'.
4. Cause 3: call DescribeScalingPlans with NextToken set to an undecodable string. Observe Message 'The NextToken provided is not valid'; expected 'The NextToken value is not valid'.
5. Cause 4: call DescribeScalingPlanResources with a valid ScalingPlanName/ScalingPlanVersion and the same undecodable NextToken. Observe the same wrong sentence; expected 'The NextToken value is not valid'.
6. Re-replay autoscalingplans-create-scaling-plan-structural-validation-envelopes (task#1-#4) and autoscalingplans-pagination-and-limit-validation-envelopes (task#1-#3); all four causes must pass before this issue is closed.
