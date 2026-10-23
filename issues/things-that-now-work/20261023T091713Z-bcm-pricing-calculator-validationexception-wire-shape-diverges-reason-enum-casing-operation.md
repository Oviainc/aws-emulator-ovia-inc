Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted bcm-pricing-calculator-validation-error-paths
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator|CreateBillScenario|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|CreateBillScenario|4xx->4xx|$.reason
Issue-Key: bcm-pricing-calculator|CreateBillScenario|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|GetBillScenario|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|GetBillScenario|4xx->4xx|$.reason
Issue-Key: bcm-pricing-calculator|GetBillScenario|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|DeleteBillScenario|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|DeleteBillScenario|4xx->4xx|$.reason
Issue-Key: bcm-pricing-calculator|DeleteBillScenario|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|ListBillScenarios|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|ListBillScenarios|4xx->4xx|$.reason
Issue-Key: bcm-pricing-calculator|ListBillScenarios|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|ListBillScenarioUsageModifications|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|ListBillScenarioUsageModifications|4xx->4xx|$.reason
Issue-Key: bcm-pricing-calculator|ListBillScenarioUsageModifications|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|UpdatePreferences|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|UpdatePreferences|4xx->4xx|$.reason
Issue-Key: bcm-pricing-calculator|UpdatePreferences|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|BatchCreateBillScenarioUsageModification|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|BatchCreateBillScenarioUsageModification|4xx->4xx|$.reason
Issue-Key: bcm-pricing-calculator|BatchCreateBillScenarioUsageModification|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|CreateWorkloadEstimate|4xx->4xx|$.fieldList
Issue-Key: bcm-pricing-calculator|CreateWorkloadEstimate|4xx->4xx|$.reason
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|4xx->4xx|$.fieldList
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|4xx->4xx|$.reason
Issue-Key: bcm-pricing-calculator|UpdateWorkloadEstimate|4xx->4xx|$.fieldList
Issue-Key: bcm-pricing-calculator|UpdateWorkloadEstimate|4xx->4xx|$.reason
Issue-Commit: 8da75f4f8e37c6bec2b6e28a02ca64766de7a47e

# ValidationException wire shape diverges: reason enum casing, operation-interpolated message, missing fieldList

> _Note: this issue was identified against the code base at commit `8da75f4f8e37c6bec2b6e28a02ca64766de7a47e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBillScenario

## Description
The emulator returns ValidationException with the right HTTP status (400) but the wrong body for bcm-pricing-calculator. Three distinct causes, 10 divergence clusters. A fixer MUST close all three before submitting.

Cause 1 of 3: ValidationException.reason is serialized in the model's member-value casing instead of the casing the service puts on the wire.
  AWS: Smithy: ValidationException { message (required), reason: ValidationExceptionReason, fieldList: ValidationExceptionFieldList }, errorKind client, httpError 400; ValidationExceptionReason values are unknownOperation | cannotParse | fieldValidationFailed | invalidRequestFromMember | disallowedRate | other. On the wire the recorded responses carry the SCREAMING_SNAKE form of the enum: the member-account rejection serializes "reason": "INVALID_REQUEST_FROM_MEMBER". This is recorded behavior (7 recordings, 7 different operations, all identical), and it is what a client's enum parser sees.
  Emulator: serializes "reason": "invalidRequestFromMember".
  Why: a client that matches the reason against the documented wire value takes the unknown-variant branch, so it cannot tell a member-account rejection from an unparseable request and retries or surfaces a generic failure instead of telling the operator to call from the payer account.
  Fix: the ValidationException serializer / reason-enum rendering in the bcm-pricing-calculator crate's awsJson error path, not the individual handlers — all seven operations go through it.
  Traces: bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted task#1 (CreateBillScenario), task#3 (GetBillScenario), task#7 (DeleteBillScenario), task#9 (ListBillScenarios), task#12 (ListBillScenarioUsageModifications), task#17 (UpdatePreferences), task#18 (BatchCreateBillScenarioUsageModification).

Cause 2 of 3: the member-account rejection message is built from the operation name instead of being the service's fixed string.
  AWS: recorded behavior only. The recorded message is operation-independent and byte-identical across all seven recorded operations: "Operation not permitted for member accounts. This API is only allowed for regular or payer accounts." The rule is therefore known from the recording: the gate emits one constant string and does NOT name the operation.
  Emulator: interpolates the operation into it, e.g. "Operation CreateBillScenario is not permitted for member accounts. This API is only allowed for regular or payer accounts." and likewise for GetBillScenario, DeleteBillScenario, ListBillScenarios, ListBillScenarioUsageModifications, UpdatePreferences, BatchCreateBillScenarioUsageModification.
  Why: the message is the only human-facing part of the gate; a client or test that matches the documented sentence does not match the emulator's, and the interpolation asserts a per-operation message shape the service does not have.
  Fix: the member-account gate's error construction in the bcm-pricing-calculator crate (one shared constant), same place as Cause 1.
  Traces: same seven steps as Cause 1.

Cause 3 of 3: pattern-violation ValidationExceptions carry no reason and no fieldList at all.
  AWS: Smithy: ValidationException.reason and ValidationException.fieldList are modeled members; ValidationExceptionField { name (required), message (required) }. For an input that violates a member's @pattern the recorded responses set reason = "fieldValidationFailed" and one fieldList entry naming the offending member, e.g. name = "name" for CreateWorkloadEstimate (WorkloadEstimateName @pattern ^[a-zA-Z0-9-]+$), name = "workloadEstimateId" for BatchCreateWorkloadEstimateUsage and name = "identifier" for UpdateWorkloadEstimate (ResourceId @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$). The per-field message is built from the request — it names the constraint the member must satisfy ("failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>") — so that construction rule is the requirement; the exact recorded wording, including the value redaction, is an example.
  Emulator: both members are absent from the body (reason: None, fieldList: None); only message is present.
  Why: the structured half of the error is what a client uses to point the user at the bad input. With fieldList absent a client cannot tell WHICH member it got wrong, and with reason absent it cannot distinguish a field-validation failure from a parse failure or a member-account rejection; form-level error reporting built on fieldList silently shows nothing.
  Fix: the input-validation layer of these operations must emit the modeled reason/fieldList (one ValidationExceptionField per failing member) rather than a message-only error — the shared validation-error builder, not three per-handler patches.
  Traces: bcm-pricing-calculator-validation-error-paths task#1 (CreateWorkloadEstimate), task#2 (BatchCreateWorkloadEstimateUsage), task#3 (UpdateWorkloadEstimate).

Note: the seven member-account steps ALSO diverge on the response content-type header; that is a separate service-wide cause filed as the bcm-pricing-calculator content-type issue in this same sweep, so these steps will still show that one key until both are fixed. A previously filed issue, "CreateWorkloadEstimate pattern-violation ValidationException message is framed differently from AWS" (20261022T174624Z...), is marked FIXED and covers only the message framing, not the missing reason/fieldList reported here.

## Evidence
Cause 1+2 (7 clusters, each affected_count 1, expected 400 / actual 400): detail for bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted#task#1 — "$.message: value_mismatch (expected 'Operation not permitted for member accounts. This API is only allowed for regular or payer accounts.', got 'Operation CreateBillScenario is not permitted for member accounts. This API is only allowed for regular or payer accounts.')" and "$.reason: value_mismatch (expected 'INVALID_REQUEST_FROM_MEMBER', got 'invalidRequestFromMember')". The same two keys diverge identically at task#3 GetBillScenario, task#7 DeleteBillScenario, task#9 ListBillScenarios, task#12 ListBillScenarioUsageModifications, task#17 UpdatePreferences, task#18 BatchCreateBillScenarioUsageModification — only the interpolated operation name changes, while the expected message never does.

Cause 3 (3 clusters, expected 400 / actual 400):
- validation-error-paths#task#1 CreateWorkloadEstimate: "$.fieldList: missing (expected [{'message': 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+', 'name': 'name'}], got None)"; "$.reason: missing (expected 'fieldValidationFailed', got None)".
- validation-error-paths#task#2 BatchCreateWorkloadEstimateUsage: fieldList expected one entry with name 'workloadEstimateId' and the UUID pattern, got None; reason expected 'fieldValidationFailed', got None.
- validation-error-paths#task#3 UpdateWorkloadEstimate: fieldList expected one entry with name 'identifier' and the UUID pattern, got None; reason expected 'fieldValidationFailed', got None.

Model grounding: smithy_query on services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json — ValidationException (httpError 400; members message/reason/fieldList), ValidationExceptionReason values, ValidationExceptionField (name+message required), CreateWorkloadEstimate.name @pattern ^[a-zA-Z0-9-]+$, ResourceId @pattern ^[a-f0-9]{8}-...-[a-f0-9]{12}$.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1+2: inspect fixture bcm-pricing-calculator-bill-scenario-estimate-member-account-restricted, step task#1 — call CreateBillScenario from a member-account context and read the 400 body: $.reason must be INVALID_REQUEST_FROM_MEMBER and $.message must be the fixed sentence with no operation name. Repeat for task#3, task#7, task#9, task#12, task#17, task#18.
4. Cause 3: inspect fixture bcm-pricing-calculator-validation-error-paths, steps task#1/task#2/task#3 — call CreateWorkloadEstimate with a name violating ^[a-zA-Z0-9-]+$, BatchCreateWorkloadEstimateUsage with a non-UUID workloadEstimateId, and UpdateWorkloadEstimate with a non-UUID identifier; each 400 body must carry reason='fieldValidationFailed' and a fieldList entry naming the offending member and its pattern constraint.
5. All ten steps must report passed.
