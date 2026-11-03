Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: groundstation.tag-and-untag-resource-lifecycle
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation.tag-and-untag-resource-lifecycle|run#15|AssertionError|assert refused == ("NotFoundException", "Resource was not found"), (
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# TagResource on a deleted resource's ARN answers ResourceNotFoundException with an ARN-bearing message, where Ground Station answers NotFoundException "Resource was not found"

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One cause.

Cause 1 of 1: the not-found refusal of the tagging operations carries the wrong error CODE (and the wrong message) when the ARN names a resource that has been deleted.

AWS: recorded behavior only for the code — the Smithy model for TagResource (services/service-com-amazonaws-groundstation-groundstation/smithy-model/com-amazonaws-groundstation-groundstation.json) declares only DependencyException (531), InvalidParameterException (431) and ResourceNotFoundException (434), and the ResourceNotFoundException shape's own doc text is "Resource was not found.". The recording shows Ground Station's tagging plane refusing a TagResource against the ARN of a Config that has just been deleted with error code `NotFoundException` and message `Resource was not found` — the whole message is FIXED text, with no variable part: no ARN, id or type is interpolated into it.

Emulator: the same call is refused with code `ResourceNotFoundException` and message `No resource found with ARN arn:aws:groundstation:<region>:<account-id>:config/<type>/<uuid>` — i.e. a different code, plus a message whose fixed text ("No resource found with ARN ") AWS never emits and into which the emulator interpolates the request's resourceArn. The handler builds this at services/service-com-amazonaws-groundstation-groundstation/src/ops/tag_resource.rs:170-176 from the helper `crate::state::tagged_resource_not_found_message`.

Why: a client's error handling is keyed on the code. Code that catches `NotFoundException` (botocore's `gs.exceptions.NotFoundException`, or an explicit `Error.Code == "NotFoundException"` switch) does not match `ResourceNotFoundException`, so a tag-cleanup path that is supposed to swallow "the resource is already gone" instead raises, and a retry/compensation loop that keys off the code takes the unmodelled-error branch. A client that logs or asserts on the message sees text AWS never produces.

Fix: the tagging operations' not-found channel in this crate — the refusal constructed in the TagResource handler (and the shared helper it calls in src/state.rs) — must answer the code and the fixed message AWS uses for a tagged-resource ARN that resolves to nothing. UntagResource and ListTagsForResource share that helper and should be made consistent with whatever the recording shows for them; this check only pins TagResource.

Traces: groundstation.tag-and-untag-resource-lifecycle (the earlier tag/untag/list assertions in that check all pass; only the post-delete refusal diverges).

## Evidence
Check groundstation.tag-and-untag-resource-lifecycle, final assertion:

  expected: ("NotFoundException", "Resource was not found")
  actual:   ("ResourceNotFoundException", "No resource found with ARN arn:aws:groundstation:us-west-2:000000000000:config/tracking/6cc2dd8d-730d-412e-99af-f7dff9b7e704")

AssertionError: expected TagResource on a deleted Config's ARN to fail NotFoundException, got ('ResourceNotFoundException', 'No resource found with ARN arn:aws:groundstation:us-west-2:000000000000:config/tracking/6cc2dd8d-730d-412e-99af-f7dff9b7e704')

The region, account id and uuid in the actual message are per-run values from the recording, not part of the requirement; the requirement is the code `NotFoundException` and the fixed message `Resource was not found` with no interpolated values.

Emulator source: src/ops/tag_resource.rs:170-176 constructs ResourceNotFoundException with crate::state::tagged_resource_not_found_message(&input.resource_arn).

## Steps to Reproduce
1. CreateConfig with configData={"trackingConfig": {"autotrack": "PREFERRED"}} and tags={"initial":"yes"}; keep configId, configType and configArn.
2. TagResource / UntagResource / ListTagsForResource against that ARN (these behave correctly).
3. DeleteConfig(configId, configType).
4. TagResource(resourceArn=<the deleted Config's ARN>, tags={"post-delete":"x"}).
5. Observe the ClientError's Error.Code and Error.Message: the emulator answers ResourceNotFoundException / "No resource found with ARN <arn>"; AWS answers NotFoundException / "Resource was not found".
