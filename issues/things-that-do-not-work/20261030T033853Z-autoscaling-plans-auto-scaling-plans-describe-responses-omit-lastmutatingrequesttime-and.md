Issue-Source: trace-sweep-divergence
Issue-Trace: autoscalingplans-scaling-plan-delete-is-not-idempotent-envelope
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscalingplans-scaling-plan-delete-is-not-idempotent-envelope autoscalingplans-scaling-plan-lifecycle
Issue-Shape-Id: com.amazonaws.autoscalingplans#AnyScaleScalingPlannerFrontendService
Issue-Key: autoscaling-plans|DescribeScalingPlans|2xx->2xx|$.ScalingPlans[*].LastMutatingRequestTime
Issue-Key: autoscaling-plans|DescribeScalingPlanResources|2xx->2xx|$.ScalingPlanResources[*].ScalingPolicies[*].PolicyName
Issue-Commit: e8c5c92d9bc397e4014b473476bd2d32121bdaea

# Auto Scaling Plans describe responses omit LastMutatingRequestTime and mint non-AWS target-tracking policy names

> _Note: this issue was identified against the code base at commit `e8c5c92d9bc397e4014b473476bd2d32121bdaea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingPlans

## Description
Both successful read paths of Auto Scaling Plans return a 200 whose body is short of what AWS returns: DescribeScalingPlans omits a member AWS always emits for a plan, and DescribeScalingPlanResources reports target-tracking policy names built to the emulator's own scheme rather than the one AWS mints. This issue has 2 causes.

Cause 1 of 2: DescribeScalingPlans never emits ScalingPlans[].LastMutatingRequestTime.
AWS: recorded behavior only — the on-disk Smithy ScalingPlan structure lists ScalingPlanName, ScalingPlanVersion, ApplicationSource, ScalingInstructions, StatusCode, StatusMessage, StatusStartTime and CreationTime, and does not declare LastMutatingRequestTime; the recording shows AWS emitting it anyway, as an epoch-seconds timestamp with sub-second precision, on every ScalingPlan entry a DescribeScalingPlans 200 returns — including entries read back right after a create, after an update, and after a delete has moved the plan into a deletion status. The requirement is that the member is PRESENT and carries the time of the last mutating request against that plan; its value is a clock reading and so differs per run.
Emulator: the member is absent from every ScalingPlan entry (reported as missing against the recorded value), so a caller that reads it gets null. The rest of the entry is served.
Why: LastMutatingRequestTime is how a client distinguishes "this plan has settled since my write" from "I am looking at a pre-write snapshot" — a converging poller compares it against the time of its own Create/Update/Delete call. With the member always null, such a loop either spins forever or, more commonly, treats the absence as "never mutated" and proceeds on stale state; it also silently breaks any caller that dereferences the field.
Fix: the DescribeScalingPlans serializer plus the plan state it reads from — the stored plan needs a last-mutating-request timestamp, stamped by every mutating operation (CreateScalingPlan, UpdateScalingPlan, DeleteScalingPlan), and the ScalingPlan output shape needs to carry it.
Traces: autoscalingplans-scaling-plan-delete-is-not-idempotent-envelope#task#4, and the sibling steps clustered with it, autoscalingplans-scaling-plan-lifecycle#task#4 and #task#7 — the same member is missing after a create, after a mutation and after a delete, so it is not specific to one lifecycle phase.

Cause 2 of 2: DescribeScalingPlanResources reports target-tracking policy names in an emulator-invented format instead of AWS's.
AWS: Smithy: ScalingPlanResource.ScalingPolicies -> ScalingPolicy, whose PolicyName is @required with @length min 1 max 256 and @pattern ^\\p{Print}+$ — the model constrains only printability, so the generated NAME FORMAT is recorded behavior only. The recording shows that when a scaling plan provisions a target-tracking policy for a resource, AWS mints the policy name as the fixed literal prefix "AutoScaling-", then the scaling plan's own name, then "-", then a UUID: "AutoScaling-<scalingPlanName>-<uuid>". The fixed parts are the "AutoScaling-" prefix and the single '-' separators; the variable parts are the plan name taken from the request and the UUID minted per policy. It is a single flat token containing no '/' — consistent with ScalingPlanName itself forbidding '/', ':' and '|'.
Emulator: PolicyName is a four-segment slash-delimited path of the form "<scalingPlanName>/TargetTracking/<hex>/<hex>": no "AutoScaling-" prefix, no UUID, and '/' separators AWS does not use here.
Why: this name is the handle a client uses to reach the underlying policy through Application Auto Scaling / EC2 Auto Scaling (DescribePolicies, alarm wiring, CloudWatch alarm dimensions, IAM resource patterns). A slash-delimited value will not match a policy-name filter or an IAM/alarm pattern written for AWS's format, and because it contains '/' it can be mis-parsed as a resource path by anything that splits on the separator. Anything keyed off the "AutoScaling-" prefix to recognise a plan-managed policy fails outright.
Fix: the component that mints policy names when a scaling plan is created (the CreateScalingPlan/UpdateScalingPlan provisioning path that materialises each ScalingPlanResource's policies) and stores them for DescribeScalingPlanResources to serialize. The scheme must be "AutoScaling-<plan name>-<uuid>".
Traces: autoscalingplans-scaling-plan-lifecycle#task#5.

Not asserted as requirements: the specific epoch value of LastMutatingRequestTime in the recording, and the specific plan name and UUID inside the recorded policy name — those are per-run values that fill the templates described above, not part of them.

## Evidence
Corpus simparity-wire-aws-v1, replayed against the emulator booted from this checkout (score 77.6%, 11 of 49 steps mismatched, all in autoscaling-plans). Both causes are 200-vs-200 body divergences, so the status and error paths are not involved.

Cause 1 — fixture autoscalingplans-scaling-plan-delete-is-not-idempotent-envelope step task#4, DescribeScalingPlans, expected 200 / got 200, verdict mismatch:
  $.ScalingPlans[0].LastMutatingRequestTime: missing (expected 1793135063.862, got None)
Cluster also covers autoscalingplans-scaling-plan-lifecycle#task#4 and autoscalingplans-scaling-plan-lifecycle#task#7 (affected_count 3). The recorded number 1793135063.862 is a clock reading; what is asserted here is that the member is emitted at all and tracks the last mutating request.

Cause 2 — fixture autoscalingplans-scaling-plan-lifecycle step task#5, DescribeScalingPlanResources, expected 200 / got 200, verdict mismatch:
  $.ScalingPlanResources[0].ScalingPolicies[0].PolicyName: value_mismatch
    expected 'AutoScaling-sim2real-probe-asp-lifeplan-4b1c7e28-8971f33c-36dd-4549-8ee3-dedca7eaa0bd'
    got      'sim2real-probe-asp-lifeplan-4b1c7e28/TargetTracking/160b8d6b63231641074a34e2f6fb970a/2caadef89be48acd2dbde9e21f608a54'
Reading the two against each other: AWS = "AutoScaling-" + plan name ('sim2real-probe-asp-lifeplan-4b1c7e28') + "-" + UUID ('8971f33c-36dd-4549-8ee3-dedca7eaa0bd'). Emulator = plan name + "/TargetTracking/" + two hex digests. The plan name and the UUID/digests are per-run; the "AutoScaling-" prefix, the '-' joins and the absence of '/' are the format requirement.

Model evidence (services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/smithy-model/com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice.json): structure ScalingPlan declares no LastMutatingRequestTime member (so Cause 1 is recorded behavior only); ScalingPolicy.PolicyName @required @length(1,256) @pattern ^\\p{Print}+$ (so Cause 2's format is recorded behavior only); ScalingPlanName @pattern ^[\\p{Print}&&[^|:/]]+$ forbids '/' in the plan name itself.

Both clusters reported owned_by: [] and an empty stub_path, so no open issue carries these replay keys. The open issue 20261029T193125Z-auto-scaling-plans-reopens-getscalingplanresourceforecastdata-still-refuses-a-self-consistent-non.md is about GetScalingPlanResourceForecastData's request gate and does not touch either read path here.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an autoscaling-plans client at it.
2. Call CreateScalingPlan with a legal plan name, a tag-based or CloudFormation ApplicationSource and one target-tracking ScalingInstruction for an Auto Scaling group.
3. Cause 1: call DescribeScalingPlans for that plan name. Observe that ScalingPlans[0] has no LastMutatingRequestTime member; expected an epoch-seconds timestamp equal to the time of the CreateScalingPlan call. Repeat after an UpdateScalingPlan and after a DeleteScalingPlan — the member must be present each time and must advance to the time of the latest mutating call.
4. Cause 2: call DescribeScalingPlanResources for the same plan/version. Observe ScalingPlanResources[0].ScalingPolicies[0].PolicyName of the form '<plan name>/TargetTracking/<hex>/<hex>'; expected a single flat token 'AutoScaling-<plan name>-<uuid>' containing no '/'.
5. Re-replay autoscalingplans-scaling-plan-delete-is-not-idempotent-envelope (task#4) and autoscalingplans-scaling-plan-lifecycle (task#4, task#5, task#7); both causes must pass before this issue is closed.
