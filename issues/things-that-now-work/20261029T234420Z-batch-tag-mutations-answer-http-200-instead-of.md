Issue-Source: trace-sweep-divergence
Issue-Trace: batch-consumable-resource-pool-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-consumable-resource-pool-lifecycle batch-scheduling-policy-standalone-lifecycle batch-service-environment-standalone-lifecycle
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|TagResource|2xx->2xx|status
Issue-Key: batch|UntagResource|2xx->2xx|status
Issue-Commit: a1f92506050befa9acc0ff29de706b69960d0a20

# batch tag mutations answer HTTP 200 instead of the recorded 204 No Content

> _Note: this issue was identified against the code base at commit `a1f92506050befa9acc0ff29de706b69960d0a20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource, UntagResource

## Description
Two net-new root causes, both on the AWS Batch tag-mutation plane: every operation whose output shape has no members is serialized as HTTP 200 with a body, where the recorded service answers HTTP 204 with no content. Neither cause is marked ALREADY FILED or OWNED in the replay report. The open issue 20261029T173319Z-batch-refusal-classification-and-message-fidelity-tag-plane-family and 20261029T173319Z-batch-8-net-new-root-causes-on-the.md cover message text on other batch operations; neither covers the response status of the tag mutations.

Cause 1 of 2 - TagResource answers 200 where AWS answers 204.
AWS: Smithy: TagResource, POST /v1/tags/{resourceArn}, input resourceArn (@httpLabel, required) + tags (TagrisTagsMap, required, @length 1..50), output shape TagResourceResponse has NO members, errors ClientException (httpError 400) / ServerException (500). The model's @http trait declares code 200, but the service does not use it for this empty-output mutation: the recorded response is HTTP 204 with no entity body (recorded behavior only - the model does not state 204). The success contract is therefore: on success, no content, and the status itself is the only success signal.
Emulator: answers HTTP 204's semantic equivalent as HTTP 200 (replay detail: "status: expected 204, got 200"), i.e. it serializes the empty TagResourceResponse structure as a body-bearing 200.
Why: a client that distinguishes a no-content success from a body-bearing success gets the wrong branch - HTTP middleware, caches and proxies that treat 204 as "no entity, nothing to parse" now receive an entity, and response-shape assertions in SDK/integration tests keyed on the service's documented no-content success fail against the emulator while passing against AWS.
Fix: the empty-output response serializer used by the batch crate's TagResource handler - an operation whose output shape has no members must emit the service's no-content status with no body, instead of defaulting to the modelled @http code.
Traces: batch-consumable-resource-pool-lifecycle#task#6 (representative); the same cause also fails batch-scheduling-policy-standalone-lifecycle#task#5 and batch-service-environment-standalone-lifecycle#task#5.

Cause 2 of 2 - UntagResource answers 200 where AWS answers 204.
AWS: Smithy: UntagResource, DELETE /v1/tags/{resourceArn}, input resourceArn (@httpLabel, required) + tagKeys (TagKeysList, @httpQuery "tagKeys", required, @length 1..50), output shape UntagResourceResponse has NO members, errors ClientException (400) / ServerException (500). As with TagResource the model declares @http code 200 while the recorded response is HTTP 204 with no entity body (recorded behavior only).
Emulator: answers HTTP 200 with a serialized empty body (replay detail: "status: expected 204, got 200").
Why: same as cause 1 - the untag success is reported with a status class the service never uses for it, so any client or intermediary keyed on no-content success behaves differently against the emulator than against AWS. It also makes the tag and untag paths indistinguishable from the body-bearing reads of the same crate.
Fix: the same empty-output response serializer, as reached from the batch crate's UntagResource handler (the teardown path exercises it too, so the fix must hold on both the task and teardown phases).
Traces: batch-consumable-resource-pool-lifecycle#task#8 (representative); the same cause also fails batch-scheduling-policy-standalone-lifecycle#task#7, batch-scheduling-policy-standalone-lifecycle#teardown#11 and batch-service-environment-standalone-lifecycle#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (14 fixtures, 98 steps, 85 passed, 13 mismatched).

Cause 1 - divergence_key service=batch|TagResource|status=204->200|keys=status: expected_status 204, actual_status 200, detail "status: expected 204, got 200"; cluster of 3 steps (batch-consumable-resource-pool-lifecycle#task#6, batch-scheduling-policy-standalone-lifecycle#task#5, batch-service-environment-standalone-lifecycle#task#5). Only the status diverges - no body member is asserted, because the recorded response has no body.

Cause 2 - divergence_key service=batch|UntagResource|status=204->200|keys=status: expected_status 204, actual_status 200, detail "status: expected 204, got 200"; cluster of 4 steps (batch-consumable-resource-pool-lifecycle#task#8, batch-scheduling-policy-standalone-lifecycle#task#7, batch-scheduling-policy-standalone-lifecycle#teardown#11, batch-service-environment-standalone-lifecycle#task#7).

Smithy evidence (com-amazonaws-batch-awsbatchv20160810.json): TagResourceResponse = structure with members {}; UntagResourceResponse = structure with members {}; both operations' @http code is 200 in the model, which the recorded wire contradicts with 204.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator.
2. Cause 1: fixture batch-consumable-resource-pool-lifecycle, step task#6 - POST /v1/tags/{resourceArn} with a non-empty tags map against a consumable resource created earlier in the fixture. Observed HTTP 200; the recorded service answers HTTP 204 with no body. Same on batch-scheduling-policy-standalone-lifecycle#task#5 (scheduling-policy ARN) and batch-service-environment-standalone-lifecycle#task#5 (service-environment ARN).
3. Cause 2: fixture batch-consumable-resource-pool-lifecycle, step task#8 - DELETE /v1/tags/{resourceArn}?tagKeys=<key> for the keys just written. Observed HTTP 200; the recorded service answers HTTP 204 with no body. Same on batch-scheduling-policy-standalone-lifecycle#task#7, batch-scheduling-policy-standalone-lifecycle#teardown#11 and batch-service-environment-standalone-lifecycle#task#7.
4. Assert the response status only - neither recorded response carries a body.
