Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-resiliency-policy-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-resiliency-policy-lifecycle
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.policy.AZ.rpoInSecs
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.policy.AZ.rtoInSecs
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.policy.Hardware.rpoInSecs
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.policy.Hardware.rtoInSecs
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.policy.Software.rpoInSecs
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.policy.Software.rtoInSecs
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.policyDescription
Issue-Key: resiliencehub|DescribeResiliencyPolicy|2xx->2xx|$.policy.tier
Issue-Related: 20261029T195027Z-resiliencehub-success-responses-omit-modeled-output-members-tags.md
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# resiliencehub UpdateResiliencyPolicy does not persist the updated policy fields

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateResiliencyPolicy

## Description
Cause 1 of 1: an UpdateResiliencyPolicy write is silently lost — a DescribeResiliencyPolicy that follows it still serves the pre-update policy.

AWS: UpdateResiliencyPolicy takes the required `policyArn` plus optional `policyName`, `policyDescription`, `dataLocationConstraint`, `tier` (ResiliencyPolicyTier) and `policy` (DisruptionPolicy, a map of DisruptionType -> FailurePolicy with rtoInSecs/rpoInSecs), and returns the required updated `policy` (ResiliencyPolicy). Each member supplied in the request REPLACES the stored value; members omitted are left untouched. A subsequent DescribeResiliencyPolicy on the same policyArn therefore reports the submitted tier, description and per-disruption-type RTO/RPO values, together with the service-populated `dataLocationConstraint`, `estimatedCostTier` and `tags`.

Emulator: DescribeResiliencyPolicy after the update answers HTTP 200 with the policy's ORIGINAL state throughout — the pre-update `tier`, the pre-update `policyDescription`, and the pre-update rtoInSecs/rpoInSecs for every disruption type (each exactly the creation-time value, an order of magnitude away from the submitted one in the recording). `dataLocationConstraint`, `estimatedCostTier` and `tags` are additionally absent, which is the serializer omission filed as Cause 5 of the 'resiliencehub success responses omit modeled output members' issue; the update loss reported here is a separate state defect and is what makes this cluster net-new.

Why: an update that returns success and changes nothing is the worst failure mode for a control-plane write. A client that retiers a policy (NonCritical -> Critical) and tightens its RTO/RPO believes the stricter objectives are in force, while every later assessment is computed against the old, laxer policy and reports compliance the client has not actually achieved. Read-after-write reconciliation also loops forever: the client re-submits the same change and the service keeps reporting the old values with a 200.

Fix: the UpdateResiliencyPolicy handler and this service's policy state — apply every supplied member onto the stored policy record (replacing the whole DisruptionPolicy map entry set for the disruption types supplied), leaving omitted members unchanged, and recompute estimatedCostTier from the new RTO/RPO values so both the update response and later Describe reads reflect the write.

Traces: resiliencehub-resiliency-policy-lifecycle#task#5 (DescribeResiliencyPolicy following the UpdateResiliencyPolicy at the preceding step of the same fixture).

## Evidence
DescribeResiliencyPolicy, resiliencehub-resiliency-policy-lifecycle#task#5 — expected HTTP 200, got HTTP 200, body still pre-update:
  $.policy.tier: value_mismatch (expected the submitted tier 'Critical', got the creation-time 'NonCritical')
  $.policy.policyDescription: value_mismatch (expected the submitted description, recorded 'sim2real-probe wire fixture policy, retiered'; got the creation-time 'sim2real-probe wire fixture policy')
  $.policy.policy.Software.rpoInSecs: value_mismatch (expected 30, got 300)
  $.policy.policy.Software.rtoInSecs: value_mismatch (expected 60, got 600)
  $.policy.policy.Hardware.rpoInSecs: value_mismatch (expected 60, got 600)
  $.policy.policy.Hardware.rtoInSecs: value_mismatch (expected 120, got 1200)
  $.policy.policy.AZ.rpoInSecs: value_mismatch (expected 90, got 900)
  $.policy.policy.AZ.rtoInSecs: value_mismatch (expected 180, got 1800)
  $.policy.dataLocationConstraint: missing (expected 'AnyLocation', got None)
  $.policy.estimatedCostTier: missing (expected 'L3', got None)
  $.policy.tags: missing (expected {}, got None)
Every 'got' value is exactly the value the policy was CREATED with, so no member of the update was applied.

Smithy citations: UpdateResiliencyPolicy input policyArn (required Arn), policyName (EntityName), policyDescription (EntityDescription, length 0-500), dataLocationConstraint (DataLocationConstraint enum), tier (ResiliencyPolicyTier enum), policy (DisruptionPolicy map: DisruptionType [Software, Hardware, AZ, Region] -> FailurePolicy); output policy (required ResiliencyPolicy) with members dataLocationConstraint, estimatedCostTier (EstimatedCostTier [L1-L4]), tags. The specific second numbers and description text above are the request's values for this run; the requirement is that whatever the update supplies is what a later read returns.

## Steps to Reproduce
1. CreateResiliencyPolicy(policyName=<p>, policyDescription=<desc1>, tier="NonCritical", policy={Software:{rtoInSecs:600,rpoInSecs:300}, Hardware:{rtoInSecs:1200,rpoInSecs:600}, AZ:{rtoInSecs:1800,rpoInSecs:900}}) -> policyArn.
2. UpdateResiliencyPolicy(policyArn=<policyArn>, policyDescription=<desc2>, tier="Critical", policy={Software:{rtoInSecs:60,rpoInSecs:30}, Hardware:{rtoInSecs:120,rpoInSecs:60}, AZ:{rtoInSecs:180,rpoInSecs:90}}) -> HTTP 200.
3. DescribeResiliencyPolicy(policyArn=<policyArn>). Observe tier still 'NonCritical', policyDescription still <desc1>, and every rtoInSecs/rpoInSecs still the step-1 values; dataLocationConstraint, estimatedCostTier and tags are absent as well.
Full replay: replay_wire_corpus() and re-check resiliencehub-resiliency-policy-lifecycle#task#5.
