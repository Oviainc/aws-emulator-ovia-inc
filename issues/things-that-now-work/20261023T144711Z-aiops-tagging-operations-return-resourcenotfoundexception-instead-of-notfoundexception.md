Issue-Source: trace-sweep-divergence
Issue-Trace: aiops-error-tagging-nonexistent-investigation-group
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: aiops-error-tagging-nonexistent-investigation-group
Issue-Shape-Id: com.amazonaws.aiops#AIOps
Issue-Key: aiops|ListTagsForResource|4xx->4xx|$.message
Issue-Key: aiops|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: aiops|TagResource|4xx->4xx|$.message
Issue-Key: aiops|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: aiops|UntagResource|4xx->4xx|$.message
Issue-Key: aiops|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# aiops tagging operations return ResourceNotFoundException instead of NotFoundException for a missing InvestigationGroup

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
All three AIOps resource-tagging operations emit the wrong error shape when the resourceArn names an InvestigationGroup that does not exist. The HTTP status (404) is correct in every case, but the error type advertised on the wire is the model-local `ResourceNotFoundException` rather than the `NotFoundException` the service actually returns, so an SDK/client cannot match the error it was written against. This is a single fault in how the tagging operations map their not-found condition onto a wire error; it is reproduced independently by each of the three operations below.

This bundle contains 3 causes.

Cause 1 of 3: ListTagsForResource on a nonexistent InvestigationGroup (fixture aiops-error-tagging-nonexistent-investigation-group, step task#1).
AWS: recorded behavior only. The recording shows HTTP 404 with header `x-amzn-errortype: NotFoundException` (recorded body message, as an example: 'Resource was not found'). Smithy: ListTagsForResource binds GET /tags/{resourceArn} with required httpLabel member `resourceArn`, and its modeled 404 error is `ResourceNotFoundException` — the model does not declare the `NotFoundException` the wire recording actually carries, so the recording is the authority for the error code here.
Emulator: answers 404 but with `x-amzn-errortype: ResourceNotFoundException` and message 'InvestigationGroup does not exist.'
Why: a client catching NotFoundException (the shape the real tagging surface returns) sees an unknown/unmodelled error type and falls through to its generic error path instead of its not-found path — retry and provisioning logic that keys off the error code breaks.
Fix: the aiops ListTagsForResource handler's not-found branch and the crate's error-to-wire mapping that renders the `x-amzn-errortype` header for tagging errors.

Cause 2 of 3: TagResource on a nonexistent InvestigationGroup (fixture aiops-error-tagging-nonexistent-investigation-group, step task#2).
AWS: recorded behavior only — HTTP 404 with `x-amzn-errortype: NotFoundException` (example message 'Resource was not found'). Smithy: TagResource binds POST /tags/{resourceArn}, required members `resourceArn` (httpLabel) and `tags`; modeled 404 error is `ResourceNotFoundException`.
Emulator: 404 with `x-amzn-errortype: ResourceNotFoundException`, message 'InvestigationGroup does not exist.'
Why: same as cause 1 — a tag-on-create workflow that distinguishes 'resource gone' from other client errors by error code mis-routes the failure.
Fix: the aiops TagResource handler's not-found branch plus the shared tagging error mapping.

Cause 3 of 3: UntagResource on a nonexistent InvestigationGroup (fixture aiops-error-tagging-nonexistent-investigation-group, step task#3).
AWS: recorded behavior only — HTTP 404 with `x-amzn-errortype: NotFoundException` (example message 'Resource was not found'). Smithy: UntagResource binds DELETE /tags/{resourceArn}, required members `resourceArn` (httpLabel) and `tagKeys` (httpQuery, @length max 50); modeled 404 error is `ResourceNotFoundException`.
Emulator: 404 with `x-amzn-errortype: ResourceNotFoundException`, message 'InvestigationGroup does not exist.'
Why: same as above; cleanup paths that treat NotFoundException as 'already deleted, succeed idempotently' instead surface a hard failure.
Fix: the aiops UntagResource handler's not-found branch plus the shared tagging error mapping.

Note on messages: the recorded body message 'Resource was not found' is cited only as an example — the generic text has no rule stated by the model or the recording, so the requirement in all three causes is the 404 status together with the `NotFoundException` error code. The one other aiops divergence in this replay (CreateInvestigationGroup accepting an unassumable roleArn) is already owned by open issue 20261022T184922Z-aiops-createinvestigationgroup-accepts-an-unassumable-rolearn-instead-of-returning.md and is not part of this bundle.

Traces: aiops-error-tagging-nonexistent-investigation-group#task#1 (ListTagsForResource), #task#2 (TagResource), #task#3 (UntagResource).

## Evidence
Replay of fixture aiops-error-tagging-nonexistent-investigation-group (trace set simparity-wire-aws-v1), 3 mismatch clusters:

step task#1 — ListTagsForResource: expected 404, got 404.
  $headers.x-amzn-errortype: expected 'NotFoundException', got 'ResourceNotFoundException'
  $.message: expected 'Resource was not found', got 'InvestigationGroup does not exist.'

step task#2 — TagResource: expected 404, got 404.
  $headers.x-amzn-errortype: expected 'NotFoundException', got 'ResourceNotFoundException'
  $.message: expected 'Resource was not found', got 'InvestigationGroup does not exist.'

step task#3 — UntagResource: expected 404, got 404.
  $headers.x-amzn-errortype: expected 'NotFoundException', got 'ResourceNotFoundException'
  $.message: expected 'Resource was not found', got 'InvestigationGroup does not exist.'

Model cited: services/service-com-amazonaws-aiops-aiops/smithy-model/com-amazonaws-aiops-aiops.json — ListTagsForResource/TagResource/UntagResource each bind /tags/{resourceArn} with required resourceArn and declare ResourceNotFoundException (httpError 404, doc 'The specified resource doesn't exist.'); no NotFoundException shape is bound, which is why the recording governs the error code.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay fixture aiops-error-tagging-nonexistent-investigation-group from trace set simparity-wire-aws-v1.
2. step task#1: call aiops ListTagsForResource (GET /tags/{resourceArn}) with a resourceArn for an InvestigationGroup that was never created. Observe 404 with x-amzn-errortype: ResourceNotFoundException; expected x-amzn-errortype: NotFoundException.
3. step task#2: call aiops TagResource (POST /tags/{resourceArn}) with that same nonexistent resourceArn and any tag map. Observe the same wrong error code.
4. step task#3: call aiops UntagResource (DELETE /tags/{resourceArn}?tagKeys=...) with that same nonexistent resourceArn. Observe the same wrong error code.
5. In all three steps the status is correctly 404; only the advertised error type (and the example message) diverge.
