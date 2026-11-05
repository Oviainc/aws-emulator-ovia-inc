Issue-Source: trace-sweep-divergence
Issue-Trace: fis-tag-operations-on-deleted-template
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-tag-operations-on-deleted-template
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: fis|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 499424707e34574cc6d85e5e4758775b0381202c

# FIS tag operations serialize a non-modeled error type on the 404 for a deleted experiment template

> _Note: this issue was identified against the code base at commit `499424707e34574cc6d85e5e4758775b0381202c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The FIS tag-management routes (POST/GET /tags/{resourceArn}) answer the right HTTP status when the ARN names an experiment template the store no longer holds, but they announce the failure with an error type the recorded AWS response does not carry in that shape. Both causes below are the SAME fault reached through the two tag operations the corpus exercises, and both are produced by the same code path: the operation shape models NO errors at all, and the crate widens a service-level `ResourceNotFoundException` in through `extra_errors`, so the REST-JSON error serializer writes `x-amzn-errortype: ResourceNotFoundException` on a response whose recorded error-type header is of a different shape. 2 causes.

Cause 1 of 2 — TagResource on a deleted experiment template (fixture fis-tag-operations-on-deleted-template, step task#4).
AWS: recorded behavior only. Smithy (com.amazonaws.fis#TagResource) binds POST /tags/{resourceArn} with success code 200 and declares an EMPTY error list — `ResourceNotFoundException` is not among this operation's modeled errors, so nothing in the model authorizes that error type on this route. The recording shows the request answered with HTTP 404 and an `x-amzn-errortype` header whose value the corpus asserts by SHAPE (the recorded value is shaped, not asserted literally) and that shape is not the bare exception name. Hypothesis: the recorded header carries AWS's fuller error-type form (`<ExceptionName>:<coral namespace URI>`) rather than the bare name, which is why the recorder shapes the value instead of asserting it.
Emulator: answers HTTP 404 — the status matches — but sets `x-amzn-errortype: ResourceNotFoundException`, a bare exception name. The replay verdict is `shape_mismatch` on `$headers.x-amzn-errortype`: expected `<shaped-not-asserted>`, got `ResourceNotFoundException`.
Why: a client that classifies a FIS failure from the `x-amzn-errortype` header — SDK error-shape resolution, a retry/error-class policy, or a log-and-alert rule keyed on the header — reads a header shaped differently from the one real FIS returns on this route, and sees an error type the service's own model never declares for TagResource. Code that matches the header's full value, or that distinguishes the service's ARN-layer refusals from a modeled operation error, takes a different branch against the emulator than against AWS.
Fix: the error channel for the tag routes — the error raised by the TagResource handler's missing-resource branch and the `x-amzn-errortype` value the REST-JSON error serializer writes for it. The operation models no errors, so what this route may announce on a 404 has to be settled where the error is widened in, not left to the generic serializer.
Traces: fis-tag-operations-on-deleted-template#task#4. Code: services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/tag_resource.rs lines 90-98 (`extra_errors = ["ValidationException", "ResourceNotFoundException"]`) and lines 180-186 (the `tags_mut` miss raising `ResourceNotFoundException`).

Cause 2 of 2 — ListTagsForResource on the same deleted experiment template (fixture fis-tag-operations-on-deleted-template, step task#5).
AWS: recorded behavior only. Smithy (com.amazonaws.fis#ListTagsForResource) binds GET /tags/{resourceArn} with success code 200 and likewise declares an EMPTY error list. The recording answers HTTP 404 with an `x-amzn-errortype` header asserted by shape, not the bare exception name.
Emulator: answers HTTP 404 with `x-amzn-errortype: ResourceNotFoundException`. Replay verdict `shape_mismatch` on `$headers.x-amzn-errortype`: expected `<shaped-not-asserted>`, got `ResourceNotFoundException`.
Why: same client consequence as cause 1, on the READ side — a tag reader that branches on the header's error type after a template is deleted diverges from AWS, and the read and the write must agree on the announced error type or a client cannot treat the two as the same refusal.
Fix: the same error channel, reached from the ListTagsForResource handler's missing-resource branch; whatever value the tag routes' 404 announces must be chosen once for both operations.
Traces: fis-tag-operations-on-deleted-template#task#5. Code: services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/list_tags_for_resource.rs lines 90-97 (`extra_errors = ["ResourceNotFoundException"]`).

Note: this is NOT covered by any open FIS issue. The four other divergences this replay reported are owned by 20261030T125712Z-fis-action-and-target-resource-type-catalog-content.md (action / target-resource-type catalog descriptions); that issue concerns catalog CONTENT and carries no replay key for these tag steps.

## Evidence
Replay of simparity-wire-aws-v1 (48 fixtures, 279 steps, 269 passed, 10 mismatched).

Cause 1 — fixture fis-tag-operations-on-deleted-template, step task#4, operation TagResource, service fis:
  verdict: mismatch
  expected_status 404, actual_status 404
  detail: $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')
  affected: [fis-tag-operations-on-deleted-template#task#4]

Cause 2 — fixture fis-tag-operations-on-deleted-template, step task#5, operation ListTagsForResource, service fis:
  verdict: mismatch
  expected_status 404, actual_status 404
  detail: $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')
  affected: [fis-tag-operations-on-deleted-template#task#5]

Model evidence (smithy_query query=operation, model services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json):
  TagResource: http POST /tags/{resourceArn}, code 200, errors: []
  ListTagsForResource: http GET /tags/{resourceArn}, code 200, errors: []
Neither operation models ResourceNotFoundException; the emulator widens it in itself.

Fixed vs variable parts of the diverging wire value: the error-type header NAME (`x-amzn-errortype`) and the HTTP status (404) are fixed; the header's VALUE is the requirement at issue — the emulator emits the bare modeled exception name where the recording asserts a differently shaped error-type value for this route. The resource ARN in the request and the template id inside it are per-run values from the fixture's own earlier steps and are not part of the requirement.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture fis-tag-operations-on-deleted-template from trace set simparity-wire-aws-v1. Its earlier steps create an FIS experiment template and then delete it, leaving the template's ARN valid in form but absent from the store.
3. Step task#4 issues TagResource (POST /tags/{resourceArn}) against the deleted template's ARN. Observe HTTP 404 with response header `x-amzn-errortype: ResourceNotFoundException`, where the recording's error-type header for that response is of another shape -> shape_mismatch on $headers.x-amzn-errortype.
4. Step task#5 issues ListTagsForResource (GET /tags/{resourceArn}) against the same ARN. Observe the identical 404 + `x-amzn-errortype: ResourceNotFoundException` and the identical shape_mismatch.
5. Both steps are reported by replay_wire_corpus as verdict=mismatch with expected_status 404 / actual_status 404 and no stub_path and no owned_by.
