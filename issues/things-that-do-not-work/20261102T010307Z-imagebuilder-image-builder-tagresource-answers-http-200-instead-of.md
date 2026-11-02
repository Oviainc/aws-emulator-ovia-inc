Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-component-tagging-workflow
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-component-tagging-workflow
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|TagResource|2xx->2xx|status
Issue-Commit: 5ed0f439ad5612c0d4c92447a77ec26b3261926f

# Image Builder TagResource answers HTTP 200 instead of the 204 No Content AWS returns

> _Note: this issue was identified against the code base at commit `5ed0f439ad5612c0d4c92447a77ec26b3261926f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: TagResource returns the wrong HTTP success status.

AWS: Image Builder's TagResource is a POST on /tags/{resourceArn} whose output structure (TagResourceResponse) has no members, and the real service answers it with HTTP 204 No Content and an empty body. Recorded behavior only for the status: the on-disk Smithy model (services/service-com-amazonaws-imagebuilder-imagebuilder/smithy-model/com-amazonaws-imagebuilder-imagebuilder.json) declares TagResource with http {method: POST, uri: /tags/{resourceArn}, code: 200} and an empty output shape, so the model's declared code disagrees with what the service actually sends; the recorded response for this step is the authority and it is 204. No error code or error message is involved — this is a success-path status only, so there is no message text to reproduce. (Smithy, for completeness: TagResource.resourceArn @required @httpLabel ImageBuilderArn with the imagebuilder ARN pattern; TagResource.tags @required TagMap @length(min 1, max 50); modeled errors InvalidParameterException 400, ResourceNotFoundException 404, ServiceException 500.)

Emulator: the tagging call succeeds and the tags are persisted, but the response carries HTTP 200 rather than 204. The handler is implemented and correct in its state effects; only the success status is wrong. It is fixed at 200 by the operation's success-status constant rather than taking the 204 the service uses for this empty-output tag mutation.

Why: a client that keys off the response status cannot distinguish the outcomes it is written for. Code that treats 204 as the success signal for tag mutations (a strict status assertion, a conditional that only accepts 204 as 'applied with no content', or a generated client/SDK-adjacent layer that selects a no-content body path on 204) sees an unexpected 200 and either reports the tag write as a failure or tries to parse a body where AWS sends none. It also makes TagResource inconsistent with the no-content mutation statuses of the rest of the tagging surface, so conformance tests and HTTP-level recorders replayed against the emulator diverge on an operation whose side effect actually worked, which is the most expensive kind of mismatch to debug.

Fix: the operation's success-status binding in its handler/serializer — services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/tag_resource.rs declares `const SUCCESS_STATUS: u16 = 200u16;`, which is what the response serializer emits for the empty TagResourceOutput. It belongs there (and should be checked against the matching constant on the sibling untag path so the two tag mutations agree), not in the state code, which already behaves correctly.

Note: the open issue 'Implement imagebuilder TagResource: unimplemented scaffold stub' (20261101T001020Z-imagebuilder-tagresource-stub.md) concerns TagResource answering 501 as an unimplemented scaffold and does NOT cover this; the operation is now implemented and persists tags, and this divergence is the status of its successful response. The replay reported this cluster with an empty stub_path and no owning issue.

Traces: imagebuilder-component-tagging-workflow#task#3 (TagResource, expected 204, got 200).

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture imagebuilder-component-tagging-workflow, step task#3, operation TagResource (service imagebuilder), verdict mismatch:

  detail: "status: expected 204, got 200"
  expected_status: 204
  actual_status: 200
  divergence_key: service=imagebuilder|TagResource|status=204->200|keys=status
  stub_path: "" (not blocked by an unimplemented operation)
  owned_by: [] (no open issue carries this replay key)

Expected: HTTP 204 with an empty body.
Actual:   HTTP 200.

No body keys diverged and no error was returned — the tags were written; only the success status differs. Source of the fixed status: services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/tag_resource.rs line 133, `const SUCCESS_STATUS: u16 = 200u16;`. The handler's success return is `Ok(TagResourceOutput {})` (line 189), an empty output structure, which is the shape AWS answers with 204.

## Steps to Reproduce
1. Boot the imagebuilder emulator from this checkout.
2. Create a component so there is a taggable resource (CreateComponent), yielding a component build-version ARN.
3. Call TagResource with resourceArn set to that ARN and a non-empty tags map (e.g. one key/value pair).
4. Observe the HTTP response status is 200 with an empty JSON body, where AWS Image Builder answers 204 No Content.
5. Equivalently, replay fixture imagebuilder-component-tagging-workflow and observe step task#3 fail with "status: expected 204, got 200" while the tag write itself succeeds (a following ListTagsForResource/GetComponent shows the tag).
