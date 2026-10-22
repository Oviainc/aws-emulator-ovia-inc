Issue-Source: trace-sweep-divergence
Issue-Trace: billingconductor-pricing-plan-arn-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: billingconductor-pricing-plan-arn-validation-errors billingconductor-pricing-rule-arn-validation-errors
Issue-Shape-Id: com.amazonaws.billingconductor#AWSBillingConductor
Issue-Key: billingconductor|UpdatePricingPlan|4xx->4xx|$.Message
Issue-Key: billingconductor|DeletePricingPlan|4xx->4xx|$.Message
Issue-Key: billingconductor|AssociatePricingRules|4xx->4xx|$.Message
Issue-Key: billingconductor|DisassociatePricingRules|4xx->4xx|$.Message
Issue-Key: billingconductor|ListPricingRulesAssociatedToPricingPlan|4xx->4xx|$.Message
Issue-Key: billingconductor|UpdatePricingRule|4xx->4xx|$.Message
Issue-Key: billingconductor|DeletePricingRule|4xx->4xx|$.Message
Issue-Key: billingconductor|ListPricingPlansAssociatedWithPricingRule|4xx->4xx|$.Message
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# ValidationException body omits required Message for pricing plan and pricing rule ARN operations

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePricingPlan

## Description
This bundle carries 8 causes, all the same fault in a different pricing operation: a modelled @pattern violation on a pricing-plan or pricing-rule ARN member is correctly answered with HTTP 400 ValidationException, but the error body is serialized WITHOUT the @required `Message` member. Every cause below must be closed before submitting. Same root fault as the billing-group/resource-association bundle filed alongside this one ("ValidationException body omits required Message for billing group and resource association operations") — that issue does NOT cover these steps, so they are filed here and must be re-replayed separately.

Shared AWS behavior — Smithy: com.amazonaws.billingconductor#ValidationException is @error("client"), httpError 400, members {Message: String @required, Reason, Fields}. The required Message for a constraint violation is constructed from the request: `<N> validation error(s) detected: Value '<supplied value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <the member's @pattern with anchors stripped>`, multiple violations joined by `; ` and counted in the prefix, list members wrapped as `Member must satisfy constraint: [Member must satisfy regular expression pattern: ...]`. That rule is the requirement; the recorded strings quoted below are examples of it. Relevant patterns: PricingPlanArn ^(arn:aws(-cn)?:billingconductor::(aws|[0-9]{12}):pricingplan/)?(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})$ and PricingRuleArn ^(arn:aws(-cn)?:billingconductor::[0-9]{12}:pricingrule/)?[a-zA-Z0-9]{10}$.

Shared Emulator behavior — from the replay output every step answered 400 with `$.Message: missing (... got None)`; no Message member is emitted.

Shared Why — clients read the violated member and constraint out of the exception message. With a required member absent, an SDK deserializes it as null and the caller gets a ValidationException that identifies neither the bad ARN member nor its pattern; for AssociatePricingRules/DisassociatePricingRules it also cannot tell that BOTH the plan ARN and a rule ARN in the list were rejected. Strict deserializers may fault on the missing required member instead of raising the modelled error.

Shared Fix — the billingconductor ValidationException construction/serialization path: the input-validation layer must build the constraint message from the member name and its modelled pattern (aggregating multiple violations under the `<N> validation errors detected:` prefix) and the error serializer must emit it as `Message`.

Cause 1 of 8: UpdatePricingPlan — fixture billingconductor-pricing-plan-arn-validation-errors, step task#1. `Arn` (PricingPlanArn, @required) is non-conforming. AWS: 400 + Message naming `arn` and the PricingPlanArn pattern. Emulator: 400, Message missing.

Cause 2 of 8: DeletePricingPlan — fixture billingconductor-pricing-plan-arn-validation-errors, step task#2. Same PricingPlanArn violation on `Arn`. AWS: 400 + one-violation Message. Emulator: 400, Message missing.

Cause 3 of 8: AssociatePricingRules — fixture billingconductor-pricing-plan-arn-validation-errors, step task#3. TWO violations: `PricingRuleArns` (list of PricingRuleArn) and `Arn` (PricingPlanArn). AWS: 400 + aggregated Message with "2 validation errors detected:" and the list clause for pricingRuleArns. Emulator: 400, Message missing — multi-violation aggregation unexercised.

Cause 4 of 8: DisassociatePricingRules — fixture billingconductor-pricing-plan-arn-validation-errors, step task#4. Same two-violation shape as cause 3. AWS: 400 + aggregated Message. Emulator: 400, Message missing.

Cause 5 of 8: ListPricingRulesAssociatedToPricingPlan — fixture billingconductor-pricing-plan-arn-validation-errors, step task#5. Violation on `PricingPlanArn` (note: the member here is named pricingPlanArn, not arn, so the Message must name that member). AWS: 400 + Message naming `pricingPlanArn`. Emulator: 400, Message missing.

Cause 6 of 8: UpdatePricingRule — fixture billingconductor-pricing-rule-arn-validation-errors, step task#1. `Arn` violates PricingRuleArn pattern. AWS: 400 + one-violation Message. Emulator: 400, Message missing.

Cause 7 of 8: DeletePricingRule — fixture billingconductor-pricing-rule-arn-validation-errors, step task#2. Same PricingRuleArn violation on `Arn`. AWS: 400 + one-violation Message. Emulator: 400, Message missing.

Cause 8 of 8: ListPricingPlansAssociatedWithPricingRule — fixture billingconductor-pricing-rule-arn-validation-errors, step task#3. Violation on member `PricingRuleArn`, which the Message must name instead of `arn`. AWS: 400 + Message naming `pricingRuleArn`. Emulator: 400, Message missing.

Traces: billingconductor-pricing-plan-arn-validation-errors#task#1..task#5; billingconductor-pricing-rule-arn-validation-errors#task#1..task#3.

## Evidence
Replay verdict `mismatch` on all 8 steps; expected_status 400 and actual_status 400 throughout, so only the body diverges.

Representative (billingconductor-pricing-plan-arn-validation-errors#task#1, UpdatePricingPlan):
  divergence_key: service=billingconductor|UpdatePricingPlan|status=400->400|keys=$.Message
  detail: $.Message: missing (expected "1 validation error detected: Value 'not-an-arn' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws(-cn)?:billingconductor::(aws|[0-9]{12}):pricingplan/)?(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})", got None)

Multi-violation aggregation (billingconductor-pricing-plan-arn-validation-errors#task#3, AssociatePricingRules):
  detail: $.Message: missing (expected "2 validation errors detected: Value '[not-an-arn-2]' at 'pricingRuleArns' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: (arn:aws(-cn)?:billingconductor::[0-9]{12}:pricingrule/)?[a-zA-Z0-9]{10}]; Value 'not-an-arn' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws(-cn)?:billingconductor::(aws|[0-9]{12}):pricingplan/)?(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})", got None)

Non-`arn` member names (billingconductor-pricing-plan-arn-validation-errors#task#5 and billingconductor-pricing-rule-arn-validation-errors#task#3):
  detail: $.Message: missing (expected "1 validation error detected: Value 'not-an-arn' at 'pricingPlanArn' failed to satisfy constraint: ...", got None)
  detail: $.Message: missing (expected "1 validation error detected: Value 'not-an-arn' at 'pricingRuleArn' failed to satisfy constraint: ...", got None)

Expected: a `Message` member in the body, built from the offending member name and its modelled pattern. Actual: no `Message` member. Quoted strings illustrate the construction rule.

Model citations: ValidationException{Message: String @required} httpError 400; PricingPlanArn @pattern ^(arn:aws(-cn)?:billingconductor::(aws|[0-9]{12}):pricingplan/)?(BasicPricingPlan|Passthrough|[a-zA-Z0-9]{10})$; PricingRuleArn @pattern ^(arn:aws(-cn)?:billingconductor::[0-9]{12}:pricingrule/)?[a-zA-Z0-9]{10}$; UpdatePricingPlan lists ValidationException (400) among its errors.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), trace set simparity-wire-aws-v1.
2. Fixture billingconductor-pricing-plan-arn-validation-errors, step task#1: PUT /update-pricing-plan with Arn="not-an-arn". Observe HTTP 400 with no `Message` member in the body.
3. Steps task#2..task#5 of that fixture repeat it for DeletePricingPlan, AssociatePricingRules, DisassociatePricingRules and ListPricingRulesAssociatedToPricingPlan; task#3/task#4 additionally send PricingRuleArns=["not-an-arn-2"] and must yield one aggregated "2 validation errors detected: ..." Message; task#5 must name member `pricingPlanArn`.
4. Fixture billingconductor-pricing-rule-arn-validation-errors, steps task#1..task#3: UpdatePricingRule, DeletePricingRule and ListPricingPlansAssociatedWithPricingRule with a non-conforming rule ARN; task#3 must name member `pricingRuleArn`.
5. Re-replay all 8 steps after the fix; each must return 400 with a `Message` naming the offending member and its pattern and the correct violation count.
