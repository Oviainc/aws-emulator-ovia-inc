Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-pricing-calculator-not-found-reads
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-pricing-calculator-not-found-reads bcm-pricing-calculator-validation-error-paths bcm-pricing-calculator-workload-estimate-tagging
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator|ListTagsForResource|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|TagResource|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|UntagResource|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|CreateWorkloadEstimate|4xx->4xx|$.fieldList[*].message
Issue-Key: bcm-pricing-calculator|CreateWorkloadEstimate|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|4xx->4xx|$.fieldList[*].message
Issue-Key: bcm-pricing-calculator|BatchCreateWorkloadEstimateUsage|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|UpdateWorkloadEstimate|4xx->4xx|$.fieldList[*].message
Issue-Key: bcm-pricing-calculator|UpdateWorkloadEstimate|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|ListTagsForResource|2xx->2xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|TagResource|2xx->2xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|UntagResource|2xx->2xx|$headers.content-type
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bcm-pricing-calculator announces awsJson1_1 on every response and drops the rejected value from its ValidationException clause

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Replaying the simparity-wire-aws-v1 corpus against this emulator produces 9 divergence clusters, ALL on bcm-pricing-calculator, and they reduce to TWO root causes. A fixer must close BOTH before submitting: the fixtures named under Cause 2 also carry the Cause 1 header defect, so neither fixture set replays clean until both are fixed.

=== Cause 1 of 2: every response is announced as application/x-amz-json-1.1, not the awsJson1_0 media type the protocol implies ===
Representative fixture: bcm-pricing-calculator-not-found-reads, step task#3 (ListTagsForResource).

AWS: the service's wire protocol is aws.protocols#awsJson1_0 (smithy_query query="service_info" on services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json: service AWSBCMPricingCalculator, version 2024-06-19, protocols [awsJson1_0], sigv4Name bcm-pricing-calculator). An awsJson1_0 service answers with `content-type: application/x-amz-json-1.0` on EVERY response — success bodies, modelled exceptions and pre-dispatch refusals alike — and the recorded responses in this corpus confirm that: every replayed step, at HTTP 200 and at HTTP 400, expects `application/x-amz-json-1.0`.

Emulator: every response carries `content-type: application/x-amz-json-1.1`. The replay reports `$headers.content-type: value_mismatch (expected 'application/x-amz-json-1.0', got 'application/x-amz-json-1.1')` on all 9 clusters, i.e. on 200 responses (ListTagsForResource, TagResource, UntagResource in the tagging fixture) and on 400 responses (the not-found reads and all three validation-error paths). This is deliberate in the checkout, not an accident: services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/service.rs declares `RESPONSE_MEDIA_TYPE = ResponseMediaType::new(AWS_JSON_1_0, AWS_JSON_1_1)` and its module header asserts "Every recorded response of this service ... carries content-type: application/x-amz-json-1.1". The corpus says the opposite, so the premise the override was built on is wrong and the re-announcement must go.

Why: a client (and any recording proxy or SDK middleware) selects its JSON-protocol decoder off the response media type. Announcing 1.1 for a 1.0 service makes the emulator's responses describe themselves as a protocol the service does not speak; strict clients and protocol-conformance tooling reject or mis-decode the body, and the mismatch is systemic — it affects all 36 modelled operations, not just the ones replayed.

Fix: the service's response media-type seam — remove the 1.0->1.1 re-announcement so the framework's model-derived media type stands. That is the `RESPONSE_MEDIA_TYPE` pair and the four seams the header says carry it (`override_protocol`, the ModelledValidation re-announcement reached via `override_operation`, `override_routing_error`, `override_framework_error`) in this crate's src/service.rs; the request side needs no change (requests route by X-Amz-Target and decode under either awsJson media type).

Traces (examples of the one defect): bcm-pricing-calculator-not-found-reads#task#3 (ListTagsForResource, 400), #task#4 (TagResource, 400), #task#5 (UntagResource, 400); bcm-pricing-calculator-workload-estimate-tagging#task#2 (ListTagsForResource, 200; cluster also covers task#4 and task#6), #task#3 (TagResource, 200), #task#5 (UntagResource, 200); and the three validation-error-paths steps listed under Cause 2, which carry this header defect as well.

=== Cause 2 of 2: the ValidationException fieldList clause omits the value that was rejected ===
Representative fixture: bcm-pricing-calculator-validation-error-paths, step task#1 (CreateWorkloadEstimate).

AWS: a constrained-member violation is refused 400 `ValidationException` (smithy_query query="shape" shape="ValidationException": errorKind client, @httpError 400, members message/reason/fieldList; ValidationExceptionField has required name + required message). The recorded refusal words each field clause as a SUBJECT that names the value submitted, then the requirement: fixed text `Value `, then the rejected value (the recording redacts it to `[redacted]`, because the submitted value is request data), then the fixed text ` failed to satisfy constraint: Member must satisfy regular expression pattern: `, then the member's @pattern. The variable parts are the submitted value and the pattern taken from the member being refused; everything else is fixed text. For CreateWorkloadEstimate.name the pattern reported is `[a-zA-Z0-9-]+` (model: WorkloadEstimateName, @pattern ^[a-zA-Z0-9-]+$, @length max 64) and for BatchCreateWorkloadEstimateUsage.workloadEstimateId / UpdateWorkloadEstimate.identifier it is `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}` (model: ResourceId, @length 36/36). Hypothesis (neither model nor recording states it): the recorder's `[redacted]` span covers AWS's usual `'<value>' at '<memberPath>'` subject, so restoring the submitted value into the subject is what closes this; the unanchored spelling of the pattern is NOT part of the divergence — the recording and the emulator agree on it.

Emulator: the clause names the member path but carries nothing of the value, so the subject segment is `Value at '<member>'` where the recording has `Value <submitted value>`. Replay: `$.fieldList[0].message: value_mismatch (expected 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+', got "Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+")`, and the same shape of mismatch with `at 'workloadEstimateId'` and `at 'identifier'`. The status (400) and the error code (ValidationException) match; only the clause's subject diverges. This too is deliberate in the checkout: src/service.rs's `constraint_clause` is documented as rendering "the member path and the violated constraint, and NOTHING of the value that was rejected", and the module header makes that a service-wide dialect — the corpus shows AWS does carry the value.

Why: the field clause is what a user and a client-side error renderer read to see WHICH submitted value was rejected. With the value dropped, a caller that submitted several values for one member (or that surfaces the service message verbatim) cannot tell what was wrong, and any client asserting on the AWS wording of this error class fails against the emulator while passing against AWS.

Fix: this crate's validation refusal wording — the clause builder used for both halves of the body (the `message` envelope clauses and the per-violation `fieldList` entries), in services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/service.rs (`constraint_clause` and the ModelledValidation path that renders fieldList). The clause count, ordering and envelope joining are correct and should not change; only the subject segment must carry the submitted value.

Traces (examples of the one defect): bcm-pricing-calculator-validation-error-paths#task#1 (CreateWorkloadEstimate, name), #task#2 (BatchCreateWorkloadEstimateUsage, workloadEstimateId), #task#3 (UpdateWorkloadEstimate, identifier).

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, steps_total 64, steps_passed 37, steps_mismatched 27, score_pct 57.8, divergence_count 9 — all 9 on service bcm-pricing-calculator, verdict mismatch, no stub_path and no owned_by on any of them.

Cause 1 (6 clusters, plus present on the 3 Cause-2 clusters):
  expected_status == actual_status (200 and 400 both occur)
  expected header: content-type: application/x-amz-json-1.0
  actual   header: content-type: application/x-amz-json-1.1
  e.g. bcm-pricing-calculator-not-found-reads#task#3 (ListTagsForResource, 400->400); bcm-pricing-calculator-workload-estimate-tagging#task#3 (TagResource, 200->200).

Cause 2 (3 clusters), status 400->400, code ValidationException in both, only the clause subject differs:
  CreateWorkloadEstimate (validation-error-paths#task#1)
    expected $.fieldList[0].message: 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+'
    actual   $.fieldList[0].message: "Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+"
  BatchCreateWorkloadEstimateUsage (validation-error-paths#task#2)
    expected 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}'
    actual   "Value at 'workloadEstimateId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}"
  UpdateWorkloadEstimate (validation-error-paths#task#3)
    expected 'Value [redacted] failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-...-[a-f0-9]{12}'
    actual   "Value at 'identifier' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-...-[a-f0-9]{12}"

Model corroboration (smithy_query on services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json): protocols [awsJson1_0]; ValidationException @httpError 400 with required message and fieldList of ValidationExceptionField{name,message}; WorkloadEstimateName @pattern ^[a-zA-Z0-9-]+$; ResourceId @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$ @length 36/36.

No prior open issue covers either cause: a listing filtered on 'bcm-pricing-calculator' returns none.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Cause 1: replay bcm-pricing-calculator-not-found-reads step task#3 (ListTagsForResource on an ARN the store does not hold) and bcm-pricing-calculator-workload-estimate-tagging steps task#2/task#3/task#5 (ListTagsForResource / TagResource / UntagResource on an existing workload estimate). Inspect the response's content-type header on BOTH the 400 and the 200: it reads application/x-amz-json-1.1 where the recording has application/x-amz-json-1.0. Any operation of this service shows it.
4. Cause 2: replay bcm-pricing-calculator-validation-error-paths steps task#1, task#2 and task#3 — CreateWorkloadEstimate with a name outside ^[a-zA-Z0-9-]+$, BatchCreateWorkloadEstimateUsage with a workloadEstimateId that is not a 36-char UUID, UpdateWorkloadEstimate with such an identifier. Each is refused 400 ValidationException as recorded, but $.fieldList[0].message begins "Value at '<member>' failed to satisfy constraint: ..." instead of carrying the submitted value in the subject.
5. After fixing, re-replay all three fixtures (bcm-pricing-calculator-not-found-reads, bcm-pricing-calculator-workload-estimate-tagging, bcm-pricing-calculator-validation-error-paths): the validation-error-paths steps only pass once BOTH causes are closed.
