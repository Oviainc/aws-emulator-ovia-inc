Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-tag-and-untag-resource-roundtrip
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-tag-and-untag-resource-roundtrip rolesanywhere-tag-lifecycle
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|ListTagsForResource|2xx->2xx|$.tags[key="added-tag"]
Issue-Key: rolesanywhere|ListTagsForResource|2xx->2xx|$.tags[*]
Issue-Commit: 45d3476fddc0c303f86586399cfbfaaac87d2cc4

# rolesanywhere UntagResource does not remove tags from resource state

> _Note: this issue was identified against the code base at commit `45d3476fddc0c303f86586399cfbfaaac87d2cc4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
IAM Roles Anywhere tag removal is not applied to stored resource state: UntagResource answers success but the keys it was asked to drop are still returned by a subsequent ListTagsForResource. Two recorded fixtures diverge on this, both reading the tag set back after an untag.

Cause 1 of 2: a tag added with TagResource and then removed with UntagResource is still listed. Fixture rolesanywhere-tag-and-untag-resource-roundtrip, step task#6 (ListTagsForResource).
AWS: Smithy (com.amazonaws.rolesanywhere) models UntagResource POST /UntagResource with required members resourceArn (AmazonResourceName, @length 1..1011) and tagKeys (TagKeyList, @length 0..200), an empty output shape UntagResourceResponse and HTTP 200; ListTagsForResource is GET /ListTagsForResource, @readonly, returning tags (TagList, @length 0..200). Because UntagResource returns 200 with no body, its only observable effect is the state change, and the @readonly read-back is the way a client confirms it: after UntagResource succeeds for a key, that key MUST NOT appear in the resource's tags. The recording shows exactly that - the untagged key is absent from the tags list.
Emulator: ListTagsForResource returns HTTP 200 (the expected status) but the tag list still carries the removed entry - the replay reports the element at key "added-tag" as extra (expected None, got a tag entry with that key and its previously set value). So UntagResource reported success and changed nothing.
Why: a client that removes a tag and re-reads to confirm sees the tag still present, so tag-driven logic (cost allocation, ABAC/condition keys on aws:ResourceTag, cleanup loops that untag-then-verify) never converges: the loop either spins or concludes removal is impossible, and a test asserting the key is gone fails against the emulator while passing on AWS. Nothing in the response signals the no-op, so the error surfaces far from its cause.
Fix: the UntagResource handler and the tag store it writes to in the rolesanywhere service crate - the handler must delete each requested key from the resource's tag map (and remain idempotent for keys that are not present, which AWS tolerates) so the @readonly ListTagsForResource read path reflects it. No change is needed in ListTagsForResource serialization: the status and element shape it returns already match, only the stored set is wrong.
Traces: rolesanywhere-tag-and-untag-resource-roundtrip#task#6.

Cause 2 of 2: the whole tag set seeded at create time survives untagging, so the expected empty list comes back populated. Fixture rolesanywhere-tag-lifecycle, step task#7 (ListTagsForResource).
AWS: same modeled contract as above. After the tags that were put on the profile are untagged, ListTagsForResource returns 200 with an empty tags list (TagList permits @length min 0), which is what the recording asserts - no elements at all.
Emulator: ListTagsForResource returns 200 but with a non-empty tags list; the replay reports $.tags[0] as extra (expected None, got the create-time tag entry, key "Environment" in this recording). The key and value here are this recording's request values, not a requirement - the requirement is that every key passed to UntagResource is absent afterwards and that a fully untagged resource serializes tags as an empty list rather than retaining its earlier entries.
Why: a client cannot tell a resource it has cleaned up from one it has not; teardown that untags before delete leaves the emulator reporting stale tags, and any assertion of "no tags remain" fails. Combined with Cause 1 this means the emulator's tag state is append-only.
Fix: same component - the UntagResource handler and the service's tag state; the removal must cover tags written by CreateProfile/CreateTrustAnchor-time tagging as well as those added later by TagResource, i.e. one shared tag map per resource ARN rather than a create-time copy the untag path cannot reach.
Traces: rolesanywhere-tag-lifecycle#task#7.

Neither cluster is owned by an open issue or blocked by a stub. The open issue "Implement rolesanywhere ListTagsForResource: unimplemented scaffold stub" (20261029T195344Z-rolesanywhere-listtagsforresource-stub.md) names the same operation, but it does not cover this: ListTagsForResource does answer here, with the correct 200 and the correct element shape - the defect is the mutation that UntagResource fails to perform. The other open rolesanywhere issues cover profile/trust-anchor/CRL field defaults, timestamp formats and refusal wording, not tag state.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false).

Cause 1 - rolesanywhere-tag-and-untag-resource-roundtrip#task#6, ListTagsForResource:
  expected_status 200, actual_status 200
  detail: $.tags[key="added-tag"]: extra (expected None, got {'key': 'added-tag', 'value': 'v1'})
  Expected (AWS recording): the key removed by the preceding UntagResource is absent from tags.
  Actual (emulator): the removed key is still present with its old value.

Cause 2 - rolesanywhere-tag-lifecycle#task#7, ListTagsForResource:
  expected_status 200, actual_status 200
  detail: $.tags[0]: extra (expected None, got {'key': 'Environment', 'value': 'Test'})
  Expected (AWS recording): tags serialized as an empty list after the untag.
  Actual (emulator): the create-time tag entry is still listed.

Model evidence (Smithy, com.amazonaws.rolesanywhere): UntagResource POST /UntagResource, code 200, input {resourceArn: AmazonResourceName required @length 1..1011, tagKeys: TagKeyList required @length 0..200}, output UntagResourceResponse (empty), errors AccessDeniedException(403)/ResourceNotFoundException(404)/ValidationException(400). ListTagsForResource GET /ListTagsForResource, @readonly, code 200, output {tags: TagList @length 0..200}. Status and error wording do not diverge in either step; only the stored tag set does.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. Replay the wire corpus (replay_wire_corpus) for trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture rolesanywhere-tag-and-untag-resource-roundtrip. It creates a resource, calls TagResource to add a key, calls UntagResource for that key, then at step task#6 calls ListTagsForResource for the same resourceArn. Observe HTTP 200 whose tags list still contains the untagged key, where the recording has it absent.
4. Cause 2: replay fixture rolesanywhere-tag-lifecycle. It tags a profile, untags it, then at step task#7 calls ListTagsForResource. Observe HTTP 200 whose tags list still contains the create-time entry, where the recording has an empty tags list.
5. Both steps pass once UntagResource deletes the requested keys from the resource's tag state and the read-back returns the remaining keys (an empty list when all were removed).
