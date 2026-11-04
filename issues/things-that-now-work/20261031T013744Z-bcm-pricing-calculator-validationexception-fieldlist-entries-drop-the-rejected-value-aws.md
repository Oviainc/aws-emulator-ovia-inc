Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-pricing-calculator-validation-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-pricing-calculator-validation-error-paths
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator|CreateWorkloadEstimate|4xx->4xx|$.fieldList[*].message
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|4xx->4xx|$.fieldList[*].message
Issue-Key: bcm-pricing-calculator|UpdateWorkloadEstimate|4xx->4xx|$.fieldList[*].message
Issue-Commit: 63c612bdac710916216a1ca8dd5dbf0b76505045

# ValidationException fieldList entries drop the rejected value AWS carries in the field clause

> _Note: this issue was identified against the code base at commit `63c612bdac710916216a1ca8dd5dbf0b76505045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkloadEstimate

## Description
Three of the ten divergence clusters in this replay are this one fault, reached through three operations. A modelled @pattern violation is refused with the right status (400) and the right error code (ValidationException), and the envelope `message` MATCHES the recording — the replay reports no divergence on $.message — but the per-field entry $.fieldList[0].message diverges: AWS's field clause names the value that was rejected and the emulator's names only the member. A fixer must close all three causes below.

AWS: a constrained-member violation is refused 400 ValidationException (smithy_query query="shape" shape="ValidationException" -> errorKind client, @httpError 400, members message (required), reason, fieldList; ValidationExceptionField has required `name` and required `message`). The wording of the field clause is recorded behavior only: the recorded clause is the subject that names the SUBMITTED VALUE, then the requirement — fixed text `Value `, then the value taken from the request, then the fixed text ` failed to satisfy constraint: Member must satisfy regular expression pattern: `, then the violated member's @pattern with the model's `^`/`$` anchors stripped. The recorder scrubs the request-derived span of that subject to `[redacted]`, so the recorded expected string is `Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`. The variable parts are the submitted value, the member being refused and that member's pattern; everything else is fixed text, and the unanchored spelling of the pattern is NOT in dispute (recording and emulator agree on it). Hypothesis (neither the model nor the recording states it): the scrubbed span covers AWS's usual `'<value>' at '<memberPath>'` subject, which is why no `at '<member>'` survives in the recorded text. What IS certain from the replay is that the scrubber matched a token the emulator does not emit at all, and that the emulator's literal `at '<member>'` is not it.

Important: the two halves of the body are NOT the same sentence in AWS. The envelope `message` for this service is recorded (and asserted by the behavioral suite, see resolved issue 20261022T174624Z-bcm-pricing-calculator-createworkloadestimate-pattern-violation-validationexception-message-is-framed-differently) as `N validation error(s) detected: Value at '<member>' failed to satisfy constraint: ...` — no value echoed — and it matches today. Only the fieldList entry carries the value. The fix must therefore change the fieldList rendering ALONE and leave the envelope clause as it is; rendering the value into both halves re-opens that resolved behavioral issue, and rendering it into neither leaves this one open.

Emulator: one clause is built and rendered twice, so the fieldList entry inherits the valueless envelope wording. src/service.rs:167-220 (`constraint_clause`) is documented as rendering "the member path and the violated constraint, and NOTHING of the value that was rejected" and builds the subject as the literal `Value` (line 212-215), and the crate header (src/service.rs:101-107) states that the `message` envelope and the `fieldList` entries are the SAME clauses by construction. Result: `Value at '<member>' failed to satisfy constraint: ...` in fieldList[0].message where the recording has the value-bearing subject.

Why: fieldList is the machine-readable half of this error — `name` says which member, `message` says what about it — and it is what a client-side error renderer and a support tool surface to a user. With the submitted value dropped, a caller that sent several values for one member (a batch entry, a list member) cannot tell WHICH value was refused, and any client asserting the AWS wording of this error class fails against the emulator while passing against AWS. Status and code are correct, so code that branches only on those is unaffected.

Fix: this crate's refusal wording, specifically the fieldList rendering — services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/service.rs, the ModelledValidation path that builds ValidationExceptionField entries, which currently reuses `constraint_clause`. Give the field entry its own subject that carries the submitted value (the Violation already carries the path, and the value is available where the violation is raised) and leave `constraint_clause` serving the envelope. The clause count, ordering, the MAX_VIOLATIONS bound, the reason value fieldValidationFailed, the unanchored @pattern spelling and the envelope joining are all correct and must not change.

Cause 1 of 3: CreateWorkloadEstimate.name (@pattern ^[a-zA-Z0-9-]+$ on WorkloadEstimateName, @length max 64) — fixture bcm-pricing-calculator-validation-error-paths, step task#1. fieldList[0].message expected `Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+`, got `Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+`.
Cause 2 of 3: BatchCreateWorkloadEstimateUsage.workloadEstimateId (ResourceId, @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$, @length 36/36) — same fixture, step task#2. Expected `Value [redacted] failed to satisfy constraint: ...`, got `Value at 'workloadEstimateId' failed to satisfy constraint: ...`.
Cause 3 of 3: UpdateWorkloadEstimate.identifier (same ResourceId constraints) — same fixture, step task#3. Expected `Value [redacted] failed to satisfy constraint: ...`, got `Value at 'identifier' failed to satisfy constraint: ...`.

Traces (examples of the one defect, one per cause): bcm-pricing-calculator-validation-error-paths#task#1, #task#2, #task#3.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, steps_total 64, steps_passed 38, steps_mismatched 26, score_pct 59.4, divergence_count 10 — all on bcm-pricing-calculator, verdict mismatch, no stub_path and no owned_by on any cluster.

Three clusters, all status 400 -> 400, error code ValidationException on both sides, $.message matching, the ONLY diverging key being $.fieldList[0].message:

  CreateWorkloadEstimate — bcm-pricing-calculator-validation-error-paths#task#1
    expected 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+'
    actual   "Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+"
  BatchCreateWorkloadEstimateUsage — bcm-pricing-calculator-validation-error-paths#task#2
    expected 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}'
    actual   "Value at 'workloadEstimateId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}"
  UpdateWorkloadEstimate — bcm-pricing-calculator-validation-error-paths#task#3
    expected 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}'
    actual   "Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}"

Note what does NOT diverge and must stay: the status, the __type, the envelope $.message, the reason value, the clause count/order and the anchor-stripped pattern spelling. These three clusters also report no content-type key, i.e. the 1.0 announcement on this refusal class is correct (contrast the seven member-account clusters filed separately).

Model grounding (smithy_query, model_path services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json): ValidationException errorKind client, @httpError 400, members message (required) / reason / fieldList; ValidationExceptionField { name required, message required }; CreateWorkloadEstimate.name -> WorkloadEstimateName required, @length max 64, @pattern ^[a-zA-Z0-9-]+$; BatchCreateWorkloadEstimateUsage.workloadEstimateId and UpdateWorkloadEstimate.identifier -> ResourceId required, @length 36/36, @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$.

Code grounding: src/service.rs:174-220 (constraint_clause; subject literal `Value` at 212-215, requirement for ViolationKind::Pattern at 197-200), src/service.rs:101-107 (the envelope message and the fieldList entries are the same clauses), src/service.rs:46-58 and 82-91 (the no-value subject is a deliberate service-wide choice).

Related issues, neither of which covers this: RESOLVED 20261029T170742Z-bcm-pricing-calculator-announces-awsjson1-1-on-every asked for the value to be restored into the subject and the divergence is still present, and it proposed changing both body halves; RESOLVED 20261022T174624Z-bcm-pricing-calculator-createworkloadestimate-pattern-violation-validationexception-message-is-framed-differently pins the ENVELOPE message without the value. The open 20261029T234207Z-bcm-pricing-calculator-tagresource-s-reserved-aws-prefix issue concerns the reserved 'aws:' tag-key message, a different clause.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture bcm-pricing-calculator-validation-error-paths steps task#1, task#2 and task#3: CreateWorkloadEstimate with a name outside ^[a-zA-Z0-9-]+$, BatchCreateWorkloadEstimateUsage with a workloadEstimateId that is not a 36-char lowercase-hex UUID, UpdateWorkloadEstimate with such an identifier.
4. Each is refused 400 ValidationException as recorded and the envelope message matches, but read $.fieldList[0].message: it begins "Value at '<member>' failed to satisfy constraint: ..." and carries nothing of the value that was submitted, where the recording's subject is the submitted value (scrubbed to [redacted] in the fixture).
5. Equivalently with a client: call CreateWorkloadEstimate(name="bad name!") and read the error's fieldList entry — fieldList[0].name == "name" is right, fieldList[0].message omits "bad name!".
6. After fixing, re-replay all three steps of bcm-pricing-calculator-validation-error-paths, and re-run the behavioral check bcm-pricing-calculator.create-workload-estimate-invalid-name-pattern-rejected to confirm the ENVELOPE message still omits the value.
