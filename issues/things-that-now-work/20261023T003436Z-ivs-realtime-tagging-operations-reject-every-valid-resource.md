Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-realtime-tag-and-untag-resource-roundtrip
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-realtime-tag-and-untag-resource-roundtrip
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: ivs-realtime|ListTagsForResource|2xx->4xx|status
Issue-Key: ivs-realtime|TagResource|2xx->4xx|status
Issue-Key: ivs-realtime|UntagResource|2xx->4xx|status
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# IVS RealTime tagging operations reject every valid resource ARN with 400

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
The whole ivs-realtime tagging surface is unreachable: every request to /tags/{resourceArn} with a well-formed IVS stage ARN is refused with HTTP 400 before the handler runs, so tags can neither be set, read back, nor removed. 3 causes, one per tagging operation, all sharing the same resourceArn admission defect.

Shared root cause: the modeled ResourceArn pattern on all three operations is ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:[a-z-]/[a-zA-Z0-9-]+$, whose resource-type element [a-z-] matches exactly ONE character. No real IVS ARN can satisfy it - 'stage' is five characters - so an emulator that enforces that pattern literally in its centralized validate step refuses every ARN AWS accepts, with a 400 ValidationException, before any tagging handler is reached. AWS accepts arn:aws:ivs:<region>:<account>:stage/<id> on all three operations (recorded behavior). Note that this also masks the fact that TagResource is still a scaffolded stub (src/ops/tag_resource.rs carries the SCAFFOLDED, NOT IMPLEMENTED marker and returns 501): because admission fails first, the step reports 400 rather than 501, so no stub issue owns it and the tagging surface must be both admitted and implemented to close this.

Cause 1 of 3: ListTagsForResource answers 400 instead of 200 with the resource's tags. Fixture ivs-realtime-tag-and-untag-resource-roundtrip, step task#3 (cluster also covers the same fixture's step task#5, the read-back after UntagResource).
  AWS: Smithy: ListTagsForResource GET /tags/{resourceArn}, output member tags (Tags, required), errors ResourceNotFoundException (404), InternalServerException (500), ValidationException (400). A stage ARN that exists returns 200 with the tag map that was set on it.
  Emulator: status 400, and $.tags missing (the expected map {'owner': 'behavioral', 'env': 'sim2real'} is absent).
  Why: a client cannot read tags off any IVS RealTime resource; tag-driven inventory, cost-allocation and policy code sees a validation failure for a request AWS accepts.
  Fix: the resourceArn admission/validation for the /tags/{resourceArn} route (the single-character resource-type element in the ResourceArn pattern must admit a multi-character type such as stage), then the ListTagsForResource handler and the tag state it reads.
  Traces: ivs-realtime-tag-and-untag-resource-roundtrip#task#3, ivs-realtime-tag-and-untag-resource-roundtrip#task#5.

Cause 2 of 3: TagResource answers 400 instead of succeeding with an empty body. Fixture ivs-realtime-tag-and-untag-resource-roundtrip, step task#2.
  AWS: Smithy: TagResource POST /tags/{resourceArn}, input resourceArn (httpLabel, required) + tags (required), empty output structure, errors ResourceNotFoundException (404) / InternalServerException (500) / ValidationException (400). Recorded behavior: AWS answers HTTP 204 (the modeled success code is the defaulted 200, so the recording is the authority here) with no body, and the tags are then readable via ListTagsForResource.
  Emulator: status 400 (expected 204). Nothing is persisted, so the later read-back fails too.
  Why: tags can never be attached to an emulated IVS RealTime resource, and a caller that treats TagResource's refusal as a client error will not retry.
  Fix: same admission path as cause 1, then implement the TagResource handler (src/ops/tag_resource.rs is still the 501 scaffold) to persist into the stage's tag state and answer the recorded empty-body success status.
  Traces: ivs-realtime-tag-and-untag-resource-roundtrip#task#2.

Cause 3 of 3: UntagResource answers 400 instead of succeeding with an empty body. Fixture ivs-realtime-tag-and-untag-resource-roundtrip, step task#4.
  AWS: Smithy: UntagResource DELETE /tags/{resourceArn}, input resourceArn (httpLabel, required) + tagKeys (httpQuery 'tagKeys', required), empty output, idempotent, errors ResourceNotFoundException (404) / InternalServerException (500) / ValidationException (400). Recorded behavior: AWS answers HTTP 204 with no body and the named keys are gone from the subsequent ListTagsForResource.
  Emulator: status 400 (expected 204).
  Why: tags cannot be removed, so no tag lifecycle round-trip works against the emulator.
  Fix: same admission path as cause 1, then the UntagResource handler and the tag state it mutates.
  Traces: ivs-realtime-tag-and-untag-resource-roundtrip#task#4.

Related prior art, not a duplicate: the same 'tagging rejects valid resource ARNs' defect was filed and resolved for the sibling services as 20261022T071923Z-ivs-9-behavioral-divergences-empty-error-message-on.md (ivs) and 20261022T071105Z-ivschat-fidelity-3-causes-resourcenotfoundexception-message-rule-createchattoken.md (ivschat). Both are resolved and neither covers ivs-realtime.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline. All three causes come from one fixture, ivs-realtime-tag-and-untag-resource-roundtrip.

Cause 1 - step task#3, operation ListTagsForResource, verdict mismatch (cluster size 2, also step task#5):
  status: expected 200, got 400
  $.tags: missing (expected {'owner': 'behavioral', 'env': 'sim2real'}, got None)

Cause 2 - step task#2, operation TagResource, verdict mismatch:
  status: expected 204, got 400

Cause 3 - step task#4, operation UntagResource, verdict mismatch:
  status: expected 204, got 400

Model evidence: smithy_query(operation="TagResource,UntagResource,ListTagsForResource") on services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/smithy-model/com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime.json - resourceArn is an httpLabel with pattern ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:[a-z-]/[a-zA-Z0-9-]+$ (single-character resource-type element, unsatisfiable by 'stage/<id>'), length 1..128.
Code evidence: services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/src/ops/tag_resource.rs lines 3-12 (SCAFFOLDED, NOT IMPLEMENTED) and line 158 (Scaffolded::not_implemented) - the stub is never reached because admission fails first, which is why the step reports 400 and not 501. Stage tags themselves are already stored: src/state.rs line 102 (StageRecord.tags) and lines 126-128 (projected on every Stage).

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. POST /CreateStage to obtain a stage ARN of the form arn:aws:ivs:<region>:<account>:stage/<id>.
3. Cause 2: POST /tags/<url-encoded stage ARN> with {"tags": {"owner": "behavioral", "env": "sim2real"}}. Observe HTTP 400; AWS answers 204 with no body. Fixture ivs-realtime-tag-and-untag-resource-roundtrip, step task#2.
4. Cause 1: GET /tags/<url-encoded stage ARN>. Observe HTTP 400 with no tags member; AWS answers 200 with the tag map. Fixture ivs-realtime-tag-and-untag-resource-roundtrip, steps task#3 and task#5.
5. Cause 3: DELETE /tags/<url-encoded stage ARN>?tagKeys=owner. Observe HTTP 400; AWS answers 204 with no body. Fixture ivs-realtime-tag-and-untag-resource-roundtrip, step task#4.
