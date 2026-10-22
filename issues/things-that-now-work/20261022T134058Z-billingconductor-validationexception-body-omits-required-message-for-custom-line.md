Issue-Source: trace-sweep-divergence
Issue-Trace: billingconductor-custom-line-item-arn-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: billingconductor-custom-line-item-arn-validation-errors billingconductor-create-operations-name-validation
Issue-Shape-Id: com.amazonaws.billingconductor#AWSBillingConductor
Issue-Key: billingconductor|UpdateCustomLineItem|4xx->4xx|$.Message
Issue-Key: billingconductor|DeleteCustomLineItem|4xx->4xx|$.Message
Issue-Key: billingconductor|ListCustomLineItemVersions|4xx->4xx|$.Message
Issue-Key: billingconductor|ListResourcesAssociatedToCustomLineItem|4xx->4xx|$.Message
Issue-Key: billingconductor|CreateBillingGroup|4xx->4xx|$.Message
Issue-Key: billingconductor|CreatePricingPlan|4xx->4xx|$.Message
Issue-Key: billingconductor|CreatePricingRule|4xx->4xx|$.Message
Issue-Key: billingconductor|CreateCustomLineItem|4xx->4xx|$.Message
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# ValidationException body omits required Message for custom line item and create-name operations

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCustomLineItem

## Description
This bundle carries 8 causes, all the same fault: a modelled @pattern violation on a custom-line-item ARN member or on a resource `Name` member is correctly answered with HTTP 400 ValidationException, but the error body is serialized WITHOUT the @required `Message` member. Every cause below must be closed before submitting. Same root fault as the two sibling bundles filed in this run ("...for billing group and resource association operations" and "...for pricing plan and pricing rule ARN operations"); neither covers these steps, so they are filed here and must be re-replayed separately. The `Name` causes are the sensitive-member variant of the rule and are worth fixing together with the ARN ones.

Shared AWS behavior — Smithy: com.amazonaws.billingconductor#ValidationException is @error("client"), httpError 400, members {Message: String @required, Reason, Fields}. The required Message for a constraint violation is built from the request: `<N> validation error(s) detected: Value '<supplied value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <the member's @pattern with anchors stripped>`. Crucially, when the offending member is @sensitive the supplied value is NOT echoed and the clause degrades to `Value at '<memberName>' failed to satisfy constraint: ...` — BillingGroupName, PricingPlanName, PricingRuleName and CustomLineItemName are all @sensitive with @pattern ^[a-zA-Z0-9_\+=\.\-@]+$ and @length 1..128, which is why the four Create* recordings carry no value in the message. That construction rule, including the sensitive-member suppression, is the requirement; the recorded strings are examples. CustomLineItemArn @pattern is ^(arn:aws(-cn)?:billingconductor::[0-9]{12}:customlineitem/)?[a-zA-Z0-9]{10}$.

Shared Emulator behavior — from the replay output every step answered 400 with `$.Message: missing (... got None)`; no Message member is emitted.

Shared Why — clients learn which member failed and why only from the exception message. With the required member absent the SDK deserializes it as null, so callers to Create* cannot tell that `name` was rejected for its character set (as opposed to a conflict or a limit), and callers to the custom-line-item ops cannot tell a malformed ARN from a missing resource. Strict deserializers may fault on the missing required member instead of raising the modelled error.

Shared Fix — the billingconductor ValidationException construction/serialization path: the input-validation layer must build the constraint message from the member name and its modelled pattern, suppressing the value for @sensitive members, and the error serializer must emit it as `Message`.

Cause 1 of 8: UpdateCustomLineItem — fixture billingconductor-custom-line-item-arn-validation-errors, step task#1. `Arn` (CustomLineItemArn, @required) is non-conforming. AWS: 400 + Message naming `arn` and the CustomLineItemArn pattern. Emulator: 400, Message missing.

Cause 2 of 8: DeleteCustomLineItem — fixture billingconductor-custom-line-item-arn-validation-errors, step task#2. Same CustomLineItemArn violation on `Arn`. AWS: 400 + one-violation Message. Emulator: 400, Message missing.

Cause 3 of 8: ListCustomLineItemVersions — fixture billingconductor-custom-line-item-arn-validation-errors, step task#3. Same CustomLineItemArn violation on `Arn`. AWS: 400 + one-violation Message. Emulator: 400, Message missing.

Cause 4 of 8: ListResourcesAssociatedToCustomLineItem — fixture billingconductor-custom-line-item-arn-validation-errors, step task#4. Same CustomLineItemArn violation on `Arn`. AWS: 400 + one-violation Message. Emulator: 400, Message missing.

Cause 5 of 8: CreateBillingGroup — fixture billingconductor-create-operations-name-validation, step task#1. `Name` (BillingGroupName, @required, @sensitive, @pattern ^[a-zA-Z0-9_\+=\.\-@]+$) contains characters outside the pattern. AWS: 400 + Message in the value-suppressed form "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_\+=\.\-@]+". Emulator: 400, Message missing.

Cause 6 of 8: CreatePricingPlan — fixture billingconductor-create-operations-name-validation, step task#2. Same violation on `Name` (PricingPlanName, @sensitive). AWS: 400 + value-suppressed Message. Emulator: 400, Message missing.

Cause 7 of 8: CreatePricingRule — fixture billingconductor-create-operations-name-validation, step task#3. Same violation on `Name` (PricingRuleName, @sensitive). AWS: 400 + value-suppressed Message. Emulator: 400, Message missing.

Cause 8 of 8: CreateCustomLineItem — fixture billingconductor-create-operations-name-validation, step task#4. Same violation on `Name` (CustomLineItemName, @sensitive). AWS: 400 + value-suppressed Message. Emulator: 400, Message missing.

Traces: billingconductor-custom-line-item-arn-validation-errors#task#1..task#4; billingconductor-create-operations-name-validation#task#1..task#4.

## Evidence
Replay verdict `mismatch` on all 8 steps; expected_status 400 and actual_status 400 throughout, so only the body diverges.

Representative (billingconductor-custom-line-item-arn-validation-errors#task#1, UpdateCustomLineItem):
  divergence_key: service=billingconductor|UpdateCustomLineItem|status=400->400|keys=$.Message
  detail: $.Message: missing (expected "1 validation error detected: Value 'not-an-arn' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws(-cn)?:billingconductor::[0-9]{12}:customlineitem/)?[a-zA-Z0-9]{10}", got None)

Sensitive-member (value-suppressed) form, billingconductor-create-operations-name-validation#task#1..task#4 (CreateBillingGroup / CreatePricingPlan / CreatePricingRule / CreateCustomLineItem), identical detail on each:
  detail: $.Message: missing (expected "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_\\+=\\.\\-@]+", got None)
  divergence_keys: service=billingconductor|CreateBillingGroup|status=400->400|keys=$.Message (and the CreatePricingPlan / CreatePricingRule / CreateCustomLineItem equivalents)

Expected: a `Message` member in the body, built from the offending member name and its modelled pattern, with the value omitted for @sensitive members. Actual: no `Message` member. Quoted strings illustrate the construction rule.

Model citations: ValidationException{Message: String @required} httpError 400; CustomLineItemArn @pattern ^(arn:aws(-cn)?:billingconductor::[0-9]{12}:customlineitem/)?[a-zA-Z0-9]{10}$; BillingGroupName @sensitive @length 1..128 @pattern ^[a-zA-Z0-9_\+=\.\-@]+$; CreateBillingGroup.Name @required targeting BillingGroupName; UpdateCustomLineItem.Arn @required targeting CustomLineItemArn; both operations list ValidationException (400).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), trace set simparity-wire-aws-v1.
2. Fixture billingconductor-custom-line-item-arn-validation-errors, step task#1: POST /update-custom-line-item with Arn="not-an-arn" (violates CustomLineItemArn @pattern). Observe HTTP 400 whose JSON body has no `Message` member.
3. Steps task#2..task#4 of that fixture repeat it for DeleteCustomLineItem, ListCustomLineItemVersions and ListResourcesAssociatedToCustomLineItem.
4. Fixture billingconductor-create-operations-name-validation, steps task#1..task#4: CreateBillingGroup, CreatePricingPlan, CreatePricingRule and CreateCustomLineItem with a `Name` containing characters outside ^[a-zA-Z0-9_\+=\.\-@]+$. Observe HTTP 400 with no `Message`.
5. Re-replay all 8 steps after the fix; each must return 400 with a `Message` naming the offending member and its pattern, and the four Create* ones must NOT echo the supplied name (sensitive member).
