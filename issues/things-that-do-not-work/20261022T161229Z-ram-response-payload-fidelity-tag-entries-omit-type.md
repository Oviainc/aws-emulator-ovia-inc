Issue-Source: trace-sweep-divergence
Issue-Trace: ram-resource-share-tagging
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-resource-share-tagging ram-principal-association-workflow
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|TagResource|2xx->2xx|$.tagsCreated[*].__type
Issue-Key: ram|UntagResource|2xx->2xx|$.deletedTags[*].__type
Issue-Key: ram|DisassociateResourceShare|2xx->2xx|$.resourceShareAssociations[*].external
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# RAM response payload fidelity: Tag entries omit __type discriminator and association external flag inverted

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three net-new wire divergences in the RAM (AWS Resource Access Manager, restJson1, sdkId "ram") emulator. All three steps return HTTP 200 as modelled, but the serialized response body differs structurally from the recorded AWS body. Causes 1 and 2 share a single root cause (the Tag structure serializer); cause 3 is the association record the share-association handlers build. Three causes total, all in service "ram".

Cause 1 of 3 — TagResource: tagsCreated[] Tag entries omit the __type discriminator.
AWS: Smithy: TagResource (POST /tagresource, http code 200) returns TagResourceResponse with member tagsCreated -> TagList -> com.amazonaws.ram#Tag {key, value}. The model does not declare a __type member, so this is recorded behavior only: the recorded AWS response serializes each element of tagsCreated with a __type field whose value is the shape name "Tag" alongside key/value (RAM's JSON serializer emits the structure discriminator for nested structures).
Emulator: the same step returns 200 but each tagsCreated element has no __type member at all ($.tagsCreated[0].__type: missing, expected 'Tag', got None).
Why: a client that deserializes RAM responses with a discriminator-driven or strict/exhaustive JSON decoder (and any consumer that round-trips or diffs the raw body, e.g. recorded-response fixtures, proxies, or SDK response-stub comparison) sees a differently shaped Tag element than AWS produces. Code written against the real wire format cannot rely on the discriminator being present, so Tag-bearing responses are not byte-compatible with AWS.
Fix: the RAM Tag/TagList response serializer (the shared structure-serialization path used for Tag output members), not the TagResource handler logic — the tag data itself is correct.
Traces: ram-resource-share-tagging#task#2 (TagResource).

Cause 2 of 3 — UntagResource: deletedTags[] Tag entries omit the same __type discriminator.
AWS: Smithy: UntagResource (POST /untagresource, http code 200) returns UntagResourceResponse with member deletedTags -> TagList -> com.amazonaws.ram#Tag {key, value}. Again __type is not modelled, so recorded behavior only: the recorded AWS response emits __type = "Tag" on every deletedTags element.
Emulator: returns 200 with deletedTags elements lacking __type ($.deletedTags[0].__type: missing, expected 'Tag', got None). This confirms the defect is in the shared Tag serializer rather than in one operation.
Why: same client impact as cause 1, on the untag path. Fixing only TagResource would leave this one diverging, so the fix must be made where Tag is serialized.
Fix: same component as cause 1 — the RAM Tag structure serializer, applied to every operation that returns a TagList.
Traces: ram-resource-share-tagging#task#4 (UntagResource).

Cause 3 of 3 — DisassociateResourceShare: resourceShareAssociations[].external reported true where AWS reports false.
AWS: Smithy: DisassociateResourceShare (POST /disassociateresourceshare, http code 200) returns resourceShareAssociations -> ResourceShareAssociationList -> ResourceShareAssociation, whose member external targets Boolean and is documented as "Indicates whether the principal belongs to the same organization in Organizations as the Amazon Web Services account that owns the resource share." The recorded AWS response for the disassociation of a principal that was associated in the same account/organization as the share owner carries external = false.
Emulator: returns 200 but the association record reports external = true ($.resourceShareAssociations[0].external: expected False, got True), i.e. the flag does not reflect whether the principal is in-organization and appears to be filled in with a constant rather than derived from the principal and the share owner.
Why: external is how a client distinguishes an in-organization principal from an external (outside-organization) one. Console-style and automation clients use it to decide whether a share is exposed outside the organization and to drive invitation handling — an inverted/constant true makes every in-organization principal look external, so callers conclude a share reaches outside the organization when it does not.
Fix: the RAM resource-share-association state/handler that constructs ResourceShareAssociation records (the association-building path shared by AssociateResourceShare/DisassociateResourceShare/GetResourceShareAssociations), so external is derived from whether the principal is the owning account or an account in the owner's organization rather than hardcoded.
Traces: ram-principal-association-workflow#task#5 (DisassociateResourceShare).

No existing open issue covers RAM (the seven open issues are for docdb, rds, fis, dynamodb, sagemaker, elasticbeanstalk and entityresolution), so all three causes are net-new. The replay reported state_reset_failures = 0 and bootstrap_signal = false, so these are genuine per-fixture fidelity gaps, not state bleed.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up = true, /__admin/health 200; 15 fixtures, 90 steps, 29 passed, 29 mismatched, 32 blocked, state_reset_failures = 0, bootstrap_signal = false). Three divergence clusters, all service = ram, all with empty stub_path and empty owned_by:

1. fixture ram-resource-share-tagging step task#2, operation TagResource, verdict mismatch, expected_status 200 / actual_status 200 — detail: "$.tagsCreated[0].__type: missing (expected 'Tag', got None)"; divergence_key service=ram|TagResource|status=200->200|keys=$.tagsCreated[0].__type; affected: [ram-resource-share-tagging#task#2].
   Expected: each tagsCreated element contains __type = "Tag" next to key/value. Actual: the member is absent (None).

2. fixture ram-resource-share-tagging step task#4, operation UntagResource, verdict mismatch, expected_status 200 / actual_status 200 — detail: "$.deletedTags[0].__type: missing (expected 'Tag', got None)"; divergence_key service=ram|UntagResource|status=200->200|keys=$.deletedTags[0].__type; affected: [ram-resource-share-tagging#task#4].
   Expected: each deletedTags element contains __type = "Tag". Actual: the member is absent (None).

3. fixture ram-principal-association-workflow step task#5, operation DisassociateResourceShare, verdict mismatch, expected_status 200 / actual_status 200 — detail: "$.resourceShareAssociations[0].external: value_mismatch (expected False, got True)"; divergence_key service=ram|DisassociateResourceShare|status=200->200|keys=$.resourceShareAssociations[0].external; affected: [ram-principal-association-workflow#task#5].
   Expected: resourceShareAssociations[0].external = false (in-organization principal). Actual: true.

Model evidence (services/service-com-amazonaws-ram-amazonresourcesharing/smithy-model/com-amazonaws-ram-amazonresourcesharing.json): TagResource output {tagsCreated: TagList}, UntagResource output {deletedTags: TagList}, TagList member com.amazonaws.ram#Tag {key: TagKey, value: TagValue} — no __type member is modelled, so the discriminator is recorded behavior only. DisassociateResourceShare output {resourceShareAssociations: ResourceShareAssociationList, clientToken}; ResourceShareAssociation.external targets Boolean, documented as indicating whether the principal belongs to the same organization as the account owning the share. Statuses are not in dispute in any of the three clusters (200 expected, 200 returned); only the response body shape/value diverges.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it (health endpoint /__admin/health answers 200, data listener accepting).
2. Replay the wire fixture corpus trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture ram-resource-share-tagging up to step task#4 and inspect the JSON body of the TagResource call (step task#2) against a resource share with at least one tag applied. Compare $.tagsCreated[0] with the recorded response: the emulator's element has only key/value, while the recording also carries __type = "Tag".
4. Cause 2: in the same fixture, inspect the UntagResource call (step task#4) and compare $.deletedTags[0]: __type = "Tag" is present in the recording and missing from the emulator's body.
5. Cause 3: replay fixture ram-principal-association-workflow through step task#5 — create a resource share, associate a principal in the owner's own account/organization, then call DisassociateResourceShare for that principal. Read $.resourceShareAssociations[0].external in the 200 response: the recording has false, the emulator returns true.
6. After fixing, re-replay all three bindings to confirm: ram-resource-share-tagging#task#2, ram-resource-share-tagging#task#4 and ram-principal-association-workflow#task#5.
