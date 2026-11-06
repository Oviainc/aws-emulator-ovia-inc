Issue-Source: trace-sweep-divergence
Issue-Trace: greengrassv2-component-version-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: greengrassv2-component-version-lifecycle greengrassv2-deployment-tag-round-trip
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrass|TagResource|2xx->2xx|status
Issue-Commit: 0ac12ca7e3c0915a225bf066c02ef0a786935230

# greengrassv2 TagResource answers HTTP 200 instead of AWS's 204 No Content

> _Note: this issue was identified against the code base at commit `0ac12ca7e3c0915a225bf066c02ef0a786935230`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: TagResource returns the wrong HTTP success status for the greengrassv2 tagging route.

AWS: recorded behavior only for the status code. The Smithy model (services/service-com-amazonaws-greengrassv2-greengrassv2/smithy-model/com-amazonaws-greengrassv2-greengrassv2.json) declares TagResource with POST /tags/{resourceArn}, a required @httpLabel `resourceArn` (GenericV2ARN, pattern ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):(components|deployments|coreDevices):), a required `tags` TagMap (@length 1..200), an EMPTY output shape (TagResourceResponse has no members) and http code 200 — but the recorded AWS responses for this operation are HTTP 204 with no body, which is the requirement here: an operation whose output shape carries no members returns 204 No Content on this surface. Errors are unaffected (InternalServerException 500, ResourceNotFoundException 404, ValidationException 400). No error code or message diverges in this cause — the only diverging field is the HTTP status line.

Emulator: answers HTTP 200 on a successful tag write. The tagging itself persists correctly (the subsequent ListTagsForResource/GetDeployment steps in both fixtures carry the written tags), so this is purely the success status of the response.

Why: a client that branches on the exact success status — or any conformance/contract check that asserts 204 No Content for an empty-output tagging call — sees a 200 with a zero-length body and treats the response as a malformed or unexpected success. A 200 with an empty body is also indistinguishable from a 200 carrying a dropped payload, so an SDK or proxy that only accepts 204 for this route rejects an otherwise successful tag write.

Fix: the TagResource operation's success status, in its handler/serializer binding. `SUCCESS_STATUS` for this operation is wired to the model's defaulted code rather than the recorded 204; note that UntagResource (DELETE /tags/{resourceArn}) carries the same defaulted 200 in the model and should be checked for the same defect while fixing this. Nothing about the state write or the error channel needs to change.

Related but NOT covering this: the open issue "Implement greengrassv2 TagResource: unimplemented scaffold stub" (20261103T065241Z-greengrassv2-tagresource-stub.md) describes TagResource as a 501 scaffold. The operation is now implemented and persists tags, so that issue's 501 symptom no longer reproduces and it does not own this status divergence.

Traces: greengrassv2-component-version-lifecycle#task#6 (representative) and greengrassv2-deployment-tag-round-trip#task#4 — both report `status: expected 204, got 200` for TagResource.

## Evidence
Replay cluster service=greengrass|TagResource|status=204->200|keys=status, verdict mismatch, affected_count 2.

Expected (recorded AWS): HTTP 204, empty body.
Actual (emulator):       HTTP 200, empty body.
Detail reported by the replay: "status: expected 204, got 200".

Affected steps: greengrassv2-component-version-lifecycle#task#6, greengrassv2-deployment-tag-round-trip#task#4.

Source of the defect: services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/tag_resource.rs line 211 declares `const SUCCESS_STATUS: u16 = 200u16;`, and line 284 returns an empty `TagResourceOutput {}`. The model's http trait for TagResource carries the defaulted `code: 200`, which is where the 200 comes from; the recorded wire responses are 204.

No request id, host, port, generated name or timestamp is involved — the divergence is the status line alone.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a greengrassv2 component version (or a deployment) so a taggable resource ARN exists, e.g. the component ARN produced by CreateComponentVersion.
3. Call TagResource POST /tags/{resourceArn} with a valid greengrassv2 resource ARN and a non-empty `tags` map (1..200 entries).
4. Observe the response status is 200 with an empty body; AWS returns 204 with an empty body.
5. Equivalently, replay fixture greengrassv2-component-version-lifecycle step task#6, or greengrassv2-deployment-tag-round-trip step task#4.
