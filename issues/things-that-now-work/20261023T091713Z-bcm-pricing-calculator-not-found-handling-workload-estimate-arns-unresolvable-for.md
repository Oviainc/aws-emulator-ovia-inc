Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-pricing-calculator-workload-estimate-tagging
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-pricing-calculator-workload-estimate-tagging bcm-pricing-calculator-not-found-reads
Issue-Shape-Id: com.amazonaws.bcmpricingcalculator#AWSBCMPricingCalculator
Issue-Key: bcm-pricing-calculator|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: bcm-pricing-calculator|ListTagsForResource|2xx->4xx|status
Issue-Key: bcm-pricing-calculator|TagResource|2xx->4xx|$
Issue-Key: bcm-pricing-calculator|TagResource|2xx->4xx|status
Issue-Key: bcm-pricing-calculator|UntagResource|2xx->4xx|$
Issue-Key: bcm-pricing-calculator|UntagResource|2xx->4xx|status
Issue-Key: bcm-pricing-calculator|ListTagsForResource|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|ListTagsForResource|4xx->4xx|status
Issue-Key: bcm-pricing-calculator|TagResource|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|TagResource|4xx->4xx|status
Issue-Key: bcm-pricing-calculator|UntagResource|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|UntagResource|4xx->4xx|status
Issue-Key: bcm-pricing-calculator|GetWorkloadEstimate|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|GetWorkloadEstimate|4xx->4xx|$headers.content-type
Issue-Key: bcm-pricing-calculator|ListWorkloadEstimateUsage|4xx->4xx|$.message
Issue-Key: bcm-pricing-calculator|ListWorkloadEstimateUsage|4xx->4xx|$headers.content-type
Issue-Commit: 8da75f4f8e37c6bec2b6e28a02ca64766de7a47e

# Not-found handling: workload-estimate ARNs unresolvable for tagging, wrong status class, paraphrased messages

> _Note: this issue was identified against the code base at commit `8da75f4f8e37c6bec2b6e28a02ca64766de7a47e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Three distinct causes around resource resolution and not-found classification, 8 divergence clusters. A fixer MUST close all three before submitting.

Cause 1 of 3: the tagging operations cannot resolve a workload estimate's ARN, so tagging an estimate that exists fails outright.
  AWS: Smithy: TagResource(arn: Arn @required, tags @required) -> empty output; UntagResource(arn, tagKeys) -> empty output; ListTagsForResource(arn) -> { tags }. Arn is @pattern ^arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+$, and ResourceNotFoundException is modeled only for an ARN that names no resource. The recording tags a workload estimate created in the same fixture: TagResource answers 200 with an empty body, ListTagsForResource answers 200 with the tags map, UntagResource answers 200 with an empty body.
  Emulator: all three answer HTTP 404 with ResourceNotFoundException (resourceType 'WorkloadEstimate') for the ARN of the estimate it had just created, so no tag is ever stored and ListTagsForResource returns no tags map at all.
  Why: tagging is completely non-functional for workload estimates. A client that tags on create fails the create flow, and cost-allocation or ownership tags can never be set or read — and the error it gets claims the resource does not exist when it does, sending the operator looking for the wrong problem.
  Fix: the ARN parsing/resource-lookup used by the tagging operations in the bcm-pricing-calculator crate must resolve a workload-estimate ARN against the estimate state (and the tag state must persist), rather than failing the lookup.
  Traces: workload-estimate-tagging#task#2 (ListTagsForResource; cluster also covers task#4 and task#6), #task#3 (TagResource), #task#5 (UntagResource).

Cause 2 of 3: for an ARN that names nothing, the tagging operations return the wrong error class and HTTP status.
  AWS: Smithy: ListTagsForResource/TagResource/UntagResource model both ResourceNotFoundException (httpError 404) and ValidationException (httpError 400). The recordings for an unresolvable workload-estimate ARN show the service answering HTTP 400 — it classifies the bad ARN as a validation failure, not as a not-found — with the message "Resource not found".
  Emulator: answers HTTP 404 (ResourceNotFoundException) for the same request.
  Why: the status class decides client behavior. 400 is a non-retryable input error a client surfaces to the caller; 404 drives "create it then retry" logic and, in SDKs, a different exception type and a different catch. A client written against the real service never enters its ResourceNotFound branch for these operations and so never handles the emulator's error at all.
  Fix: the tagging operations' error classification (the not-found path must raise the modeled ValidationException with a 400, not ResourceNotFoundException) in the bcm-pricing-calculator crate's tagging handlers.
  Traces: not-found-reads#task#3 (ListTagsForResource), #task#4 (TagResource), #task#5 (UntagResource).

Cause 3 of 3: the workload-estimate not-found message echoes the requested identifier instead of the service's fixed message.
  AWS: recorded behavior only, but the rule is determinable: across two different operations (GetWorkloadEstimate and ListWorkloadEstimateUsage) the recorded 404 body carries the identical, identifier-independent string "The requested WorkloadEstimate was not found. Check the WorkloadEstimate ID and try again." — the service names the resource TYPE and never echoes the id. The status (404) and the error class (ResourceNotFoundException, httpError 404 in the model) already match, so this cause is message-framing only.
  Emulator: builds the message from the request instead, e.g. "No workload estimate found with identifier <uuid>", and does the same with an ARN in the tagging paths ("No resource found with ARN arn:aws:bcm-pricing-calculator::000000000000:workload-estimate/<uuid>").
  Why: a client or operator matching the documented message does not match, and echoing the caller's identifier back reflects the request rather than reporting the service's condition — the same defect already filed and fixed for the sibling service as "GetExport's ResourceNotFoundException message echoes the requested ARN instead of AWS's fixed generic not-found message" (20261022T003746Z..., bcm-data-exports). The emulator's ARN string also shows an empty region segment, which is not itself asserted here.
  Fix: the not-found error construction in the bcm-pricing-calculator crate's shared error builder — emit the fixed per-resource-type message rather than interpolating the requested identifier.
  Traces: not-found-reads#task#1 (GetWorkloadEstimate), #task#2 (ListWorkloadEstimateUsage).

Note: the not-found-reads#task#1/#task#2 steps and the tagging steps ALSO diverge on the response content-type header, filed separately as the bcm-pricing-calculator content-type issue in this same sweep. The open bcm-data-exports issue "TagResource not-found answers 404 instead of 400" (20261023T074806Z...) is the same shape of defect in a DIFFERENT service and does not cover these steps.

## Evidence
Cause 1 (tagging an estimate that exists), fixture bcm-pricing-calculator-workload-estimate-tagging:
- #task#2 ListTagsForResource (affected_count 3: task#2, task#4, task#6) — "status: expected 200, got 404"; "$.tags: missing (expected {'env': 'test'}, got None)".
- #task#3 TagResource — "status: expected 200, got 404"; "$: value_mismatch (expected {}, got {'__type': 'ResourceNotFoundException', 'message': 'No resource found with ARN arn:aws:bcm-pricing-calculator::000000000000:workload-estimate/93358b4d-...', 'resourceId': '...', 'resourceType': 'WorkloadEstimate'})".
- #task#5 UntagResource — "status: expected 200, got 404"; same ResourceNotFoundException body where an empty object was recorded.
The estimate in question was created by this fixture's own setup#1 (which answered 200), so the resource exists.

Cause 2 (unresolvable ARN), fixture bcm-pricing-calculator-not-found-reads:
- #task#3 ListTagsForResource — "status: expected 400, got 404"; "$.message: value_mismatch (expected 'Resource not found', got 'No resource found with ARN arn:aws:bcm-pricing-calculator::000000000000:workload-estimate/bc5852c1-...')".
- #task#4 TagResource — identical: expected 400, got 404, same message divergence.
- #task#5 UntagResource — identical: expected 400, got 404, same message divergence.

Cause 3 (message framing on a correct 404), fixture bcm-pricing-calculator-not-found-reads:
- #task#1 GetWorkloadEstimate — expected_status 404, actual_status 404; "$.message: value_mismatch (expected 'The requested WorkloadEstimate was not found. Check the WorkloadEstimate ID and try again.', got 'No workload estimate found with identifier bc5852c1-ada1-44f4-acb3-9ed4765531aa')".
- #task#2 ListWorkloadEstimateUsage — expected 404, actual 404; the SAME expected message and the SAME interpolated actual, which is what establishes the expected text as a fixed, operation- and identifier-independent string.

Model grounding: smithy_query on services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/smithy-model/com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator.json — ListTagsForResource/TagResource/UntagResource inputs (arn: Arn @required, @pattern ^arn:aws[-a-z0-9]*:bcm-pricing-calculator:[-a-z0-9]*:[0-9]{12}:[-a-z0-9/:_]+$), their error lists (ResourceNotFoundException httpError 404 AND ValidationException httpError 400), ListTagsForResourceResponse.tags, TagResource/UntagResource empty outputs, ResourceNotFoundException { message, resourceId, resourceType } httpError 404.

state_reset_failures for this replay was 0, so the tagging fixture's 404s are not state bleed from another fixture.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: fixture bcm-pricing-calculator-workload-estimate-tagging — run setup#1 (CreateWorkloadEstimate), then task#3 TagResource with that estimate's ARN (expect 200 and an empty body), task#2/task#4/task#6 ListTagsForResource (expect 200 and the tags map, e.g. env=test), task#5 UntagResource (expect 200 and an empty body). All four currently answer 404 ResourceNotFoundException.
4. Cause 2: fixture bcm-pricing-calculator-not-found-reads steps task#3/task#4/task#5 — call ListTagsForResource, TagResource and UntagResource with a well-formed workload-estimate ARN that names no estimate; each must answer HTTP 400 (ValidationException, message 'Resource not found'), not HTTP 404.
5. Cause 3: fixture bcm-pricing-calculator-not-found-reads steps task#1/task#2 — call GetWorkloadEstimate and ListWorkloadEstimateUsage with an unknown workloadEstimateId; the 404 body's message must be the fixed 'The requested WorkloadEstimate was not found. Check the WorkloadEstimate ID and try again.' and must not echo the identifier.
6. All eight steps must report passed.
