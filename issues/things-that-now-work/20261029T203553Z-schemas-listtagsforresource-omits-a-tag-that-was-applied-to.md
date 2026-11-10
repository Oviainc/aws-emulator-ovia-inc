Issue-Source: trace-sweep-divergence
Issue-Trace: schemas-registry-tagging
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: schemas-registry-tagging
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas|ListTagsForResource|2xx->2xx|$.tags.team
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# ListTagsForResource omits a tag that was applied to the registry, so the tag set read back is incomplete

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Cause 1 of 1 - a tag applied to a registry is not visible to a later ListTagsForResource. AWS: Smithy models ListTagsForResource GET /tags/{ResourceArn} -> 200 with Tags (wire name 'tags') as a Tags map, and the tagging operations of a service are read-after-write with respect to it: a key the caller applied to a resource appears in the next read of that resource's tags, with the value it was applied with, alongside the keys already held. In the recording the registry's tag set read back at this step contains the key 'team' with value 'eventing'. Emulator: answers 200 with a tags map that is missing that key entirely ($.tags.team expected 'eventing', got None); the other keys of the set match, so the response is well-formed and only the applied key is absent. Hypothesis for the mechanism: the step that applies the tag in this fixture is TagResource, which is an unimplemented scaffold stub in this checkout - open issue 20261022T034503Z-schemas-tagresource-stub.md tracks the stub itself, but that issue does not carry this replay key and so does not cover this divergence; the read-back divergence must be re-replayed in its own right, because a TagResource that is implemented but does not write into the tag set ListTagsForResource reads (state.rs writable_tags_for_arn -> tags_for_arn, which deliberately resolves a write through the read path) would leave this step failing in exactly the same way. Why: a client that tags a resource and then reads its tags back - the standard verify-after-tag and the way cost-allocation and access-control tooling discovers tags - sees the tag missing and concludes the write was lost. Fix: the TagResource handler and the tag set it writes through (state.rs writable_tags_for_arn), so that a key it applies is reported by the ListTagsForResource handler for the same ResourceArn. Traces: schemas-registry-tagging#task#4.

## Evidence
Replay of fixture schemas-registry-tagging step task#4, service=schemas, operation ListTagsForResource, expected 200 / actual 200: $.tags.team: missing (expected 'eventing', got None). Only that key diverged in the cluster; no other member of the response mismatched.

Smithy (services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json): ListTagsForResource GET /tags/{ResourceArn}, output Tags -> Tags map with jsonName 'tags', errors BadRequestException 400 / ForbiddenException 403 / InternalServerErrorException 500 / NotFoundException 404.

Code: services/service-com-amazonaws-schemas-schemas/src/state.rs tags_for_arn() ~line 96 and writable_tags_for_arn() ~line 135 - a tag write resolves through the same ARN resolution the read uses, so a key missing from the read means nothing was written for that resource. Related open issue (does not carry this replay key): 20261022T034503Z-schemas-tagresource-stub.md.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture schemas-registry-tagging from the start so the registry is created and tagged, then step task#4: GET /tags/<registry ARN>.
3. Observe 200 whose tags map does not contain the key applied earlier in the fixture ('team'), while the keys carried from the create are present.
