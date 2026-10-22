Issue-Source: trace-sweep-divergence
Issue-Trace: billingconductor-billing-group-arn-validation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: billingconductor-billing-group-arn-validation-errors billingconductor-batch-resource-association-validation-errors
Issue-Shape-Id: com.amazonaws.billingconductor#AWSBillingConductor
Issue-Key: billingconductor|UpdateBillingGroup|4xx->4xx|$.Message
Issue-Key: billingconductor|GetBillingGroupCostReport|4xx->4xx|$.Message
Issue-Key: billingconductor|AssociateAccounts|4xx->4xx|$.Message
Issue-Key: billingconductor|DisassociateAccounts|4xx->4xx|$.Message
Issue-Key: billingconductor|DeleteBillingGroup|4xx->4xx|$.Message
Issue-Key: billingconductor|BatchAssociateResourcesToCustomLineItem|4xx->4xx|$.Message
Issue-Key: billingconductor|BatchDisassociateResourcesFromCustomLineItem|4xx->4xx|$.Message
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# ValidationException body omits required Message for billing group and resource association operations

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateBillingGroup

## Description
This bundle carries 7 causes, all the same fault in a different operation: when an input member violates its modelled @pattern constraint the emulator correctly selects HTTP 400 ValidationException but serializes the error body WITHOUT the `Message` member, which the model marks @required. Every cause below must be closed before submitting.

Shared AWS behavior — Smithy: com.amazonaws.billingconductor#ValidationException is a @error("client") with httpError 400 and members {Message: String @required, Reason, Fields}. Because Message is required, the AWS JSON/REST-JSON error body ALWAYS carries it, and for a constraint violation its text is built from the request: `<N> validation error(s) detected: Value '<supplied value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <the member's @pattern, anchors stripped>`, with multiple violations joined by `; ` and the count in the prefix. For list members the inner clause is `Member must satisfy constraint: [Member must satisfy regular expression pattern: ...]`. @sensitive members are rendered without the value (`Value at '<memberName>' failed ...`). That construction rule — not any one recorded string — is the requirement here. Status 400 is already correct in every cause; the defect is purely the absent required member.

Shared Emulator behavior — from the replay output, every one of these steps answered 400 with `$.Message: missing (expected "...", got None)`. No Message member is emitted at all.

Shared Why — SDK clients surface a constraint failure through the exception's message (`e.getMessage()` / `err.message` / `e.response['Error']['Message']`). With Message absent the SDK deserializes a required field as null/empty, so a caller gets a bare ValidationException naming neither the offending member nor the constraint. Code and operators that log or branch on the violated member cannot tell a bad ARN from a bad account id, and strict SDK deserializers may fault on the missing required member rather than raising the modelled error.

Shared Fix — the billingconductor ValidationException construction/serialization path: the input-validation layer that detects the @pattern violation must build the constraint-violation message from the member name and the member's modelled pattern (aggregating multiple violations with the `<N> validation errors detected:` prefix) and the error serializer must emit it as `Message` in the body.

Cause 1 of 7: UpdateBillingGroup — fixture billingconductor-billing-group-arn-validation-errors, step task#1. `Arn` (target BillingGroupArn, @required, @pattern ^(arn:aws(-cn)?:billingconductor::[0-9]{12}:billinggroup/)?[a-zA-Z0-9]{10,12}$) is sent as a non-conforming string. AWS: 400 with Message describing the `arn` pattern violation (example: "1 validation error detected: Value 'not-an-arn' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws(-cn)?:billingconductor::[0-9]{12}:billinggroup/)?[a-zA-Z0-9]{10,12}"). Emulator: 400, Message missing.

Cause 2 of 7: GetBillingGroupCostReport — fixture billingconductor-billing-group-arn-validation-errors, step task#2. Same BillingGroupArn pattern violation on `Arn`. AWS: 400 with the one-violation Message. Emulator: 400, Message missing.

Cause 3 of 7: AssociateAccounts — fixture billingconductor-billing-group-arn-validation-errors, step task#3. TWO violations in one request: `AccountIds` (list of AccountId, @pattern ^[0-9]{12}$) and `Arn` (BillingGroupArn). AWS: 400 with the aggregated two-violation Message, prefix "2 validation errors detected:", the list member rendered as "Member must satisfy constraint: [Member must satisfy regular expression pattern: [0-9]{12}]". Emulator: 400, Message missing — so the multi-violation aggregation path is unexercised too.

Cause 4 of 7: DisassociateAccounts — fixture billingconductor-billing-group-arn-validation-errors, step task#4. Same two-violation shape as cause 3 (AccountIds + Arn). AWS: 400 with the aggregated Message. Emulator: 400, Message missing.

Cause 5 of 7: DeleteBillingGroup — fixture billingconductor-billing-group-arn-validation-errors, step task#5. BillingGroupArn pattern violation on `Arn`. AWS: 400 with the one-violation Message. Emulator: 400, Message missing.

Cause 6 of 7: BatchAssociateResourcesToCustomLineItem — fixture billingconductor-batch-resource-association-validation-errors, step task#1. `ResourceArns` (CustomLineItemBatchAssociationsList) carries an ARN that is not a billingconductor customlineitem/billinggroup ARN, violating the member pattern (arn:aws(-cn)?:billingconductor::[0-9]{12}:(customlineitem|billinggroup)/)?[a-zA-Z0-9]{10,12}. AWS: 400 with the list-member constraint Message. Emulator: 400, Message missing.

Cause 7 of 7: BatchDisassociateResourcesFromCustomLineItem — fixture billingconductor-batch-resource-association-validation-errors, step task#2. Same `ResourceArns` list-member pattern violation. AWS: 400 with the list-member constraint Message. Emulator: 400, Message missing.

Traces: billingconductor-billing-group-arn-validation-errors#task#1..task#5; billingconductor-batch-resource-association-validation-errors#task#1, #task#2.

## Evidence
Replay verdict `mismatch` on all 7 steps; expected_status 400 and actual_status 400 in every case, so the divergence is confined to the body.

Representative (billingconductor-billing-group-arn-validation-errors#task#1, UpdateBillingGroup):
  divergence_key: service=billingconductor|UpdateBillingGroup|status=400->400|keys=$.Message
  detail: $.Message: missing (expected "1 validation error detected: Value 'not-an-arn' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws(-cn)?:billingconductor::[0-9]{12}:billinggroup/)?[a-zA-Z0-9]{10,12}", got None)

Multi-violation aggregation (billingconductor-billing-group-arn-validation-errors#task#3, AssociateAccounts):
  detail: $.Message: missing (expected "2 validation errors detected: Value '[not-an-id]' at 'accountIds' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: [0-9]{12}]; Value 'not-an-arn' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws(-cn)?:billingconductor::[0-9]{12}:billinggroup/)?[a-zA-Z0-9]{10,12}", got None)

List-member constraint (billingconductor-batch-resource-association-validation-errors#task#1):
  detail: $.Message: missing (expected "1 validation error detected: Value '[arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0]' at 'resourceArns' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy regular expression pattern: (arn:aws(-cn)?:billingconductor::[0-9]{12}:(customlineitem|billinggroup)/)?[a-zA-Z0-9]{10,12}]", got None)

Expected: body contains a `Message` member (required by the model) built from the offending member name and its modelled pattern. Actual: no `Message` member in the body. The quoted strings are examples of the construction rule, not literal requirements.

Model citations: ValidationException{Message: String @required} httpError 400; BillingGroupArn @pattern ^(arn:aws(-cn)?:billingconductor::[0-9]{12}:billinggroup/)?[a-zA-Z0-9]{10,12}$; AccountId @pattern ^[0-9]{12}$; UpdateBillingGroup/AssociateAccounts/DeleteBillingGroup/GetBillingGroupCostReport and both Batch*ResourcesToCustomLineItem ops all list ValidationException among their errors.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), trace set simparity-wire-aws-v1.
2. Fixture billingconductor-billing-group-arn-validation-errors, step task#1: POST /update-billing-group with Arn="not-an-arn" (violates BillingGroupArn @pattern). Observe HTTP 400 whose JSON body has no `Message` member.
3. Same fixture steps task#2..task#5 repeat it for GetBillingGroupCostReport, AssociateAccounts, DisassociateAccounts and DeleteBillingGroup; task#3/task#4 additionally send AccountIds=["not-an-id"], which should produce a single aggregated "2 validation errors detected: ..." Message.
4. Fixture billingconductor-batch-resource-association-validation-errors, steps task#1 and task#2: PUT /batch-associate-resources-to-custom-line-item and the disassociate counterpart with ResourceArns containing a non-billingconductor ARN. Observe HTTP 400 with no `Message`.
5. Re-replay all 7 steps after the fix; each must return 400 with a `Message` naming the offending member and its pattern, and with the correct "<N> validation error(s) detected:" prefix.
