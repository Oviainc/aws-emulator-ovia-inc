Issue-Source: trace-sweep-divergence
Issue-Trace: synthetics-group-canary-association-errors
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: synthetics-group-canary-association-errors synthetics-group-error-paths synthetics-tag-resource-and-list-tags-roundtrip
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics|CreateGroup|2xx->2xx|$.Group.Tags
Issue-Key: synthetics|ListTagsForResource|2xx->2xx|$
Issue-Key: synthetics|GetGroup|2xx->2xx|$.Group.Tags
Issue-Commit: dbd8b6da40b3283b718ba9768f41bda1f397bd18

# synthetics: tags never surface on the wire — CreateGroup/GetGroup omit Group.Tags and ListTagsForResource answers with no Tags member

> _Note: this issue was identified against the code base at commit `dbd8b6da40b3283b718ba9768f41bda1f397bd18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
Three causes, all in the service's tag surface: no response the emulator serves ever carries a tag map, whether the map is empty (where AWS sends {}) or populated (where AWS sends the tags). All three must be closed before this issue is resubmitted. Note the open stub issue 20261029T184613Z-synthetics-tagresource-stub.md (TagResource unimplemented) is the upstream WRITE path for causes 2 and 3; implementing it is necessary but NOT sufficient, because the read paths below additionally suppress the map they would then find, and cause 1 is independent of it.

=== Cause 1 of 3: CreateGroup omits Group.Tags entirely for a group created without tags, where AWS returns an empty map ===
AWS: Smithy: CreateGroup (POST /group) -> CreateGroupResponse.Group -> Group, whose member Tags targets TagMap (map, @length min 1 max 50, "The list of key-value pairs that are associated with the canary"). Recorded behavior: a CreateGroup that supplies no Tags answers 200 with Group.Tags PRESENT as an empty object ({}) — the member is always serialized, and emptiness is expressed by an empty map, not by absence. The TagMap @length min 1 constrains an INPUT map a caller sends, not the output projection.
Emulator: the member is dropped whenever the stored map is empty. The projection is deliberate and shared: src/ops/list_tags_for_resource.rs documents "`TagMap` is modelled `length 1..50`, so a resource carrying no tags is reported with no map at all rather than an empty one — the same projection `Canary$Tags` and `Group$Tags` use for the very same stored field", and StoredGroup::to_group (src/state.rs), used by the CreateGroup handler, applies it. Replay saw $.Group.Tags missing where {} was expected.
Why: a client reading response['Group']['Tags'] gets a KeyError/nil instead of an empty map, so the common pattern of reading the current tag set and writing it back breaks on every untagged group; code that distinguishes "no tags" from "field not returned" cannot tell the two apart.
Fix: the Group projection in src/state.rs (StoredGroup::to_group) and the equivalent Canary projection — serialize the tag map unconditionally, with {} for an empty one — rather than the (!tags.is_empty()).then(...) guard.

=== Cause 2 of 3: ListTagsForResource answers an empty body for a resource that carries tags ===
AWS: Smithy: ListTagsForResource (GET /tags/{ResourceArn}) -> ListTagsForResourceResponse.Tags (TagMap); errors BadRequestException 400, ConflictException 409, InternalFailureException 500, NotFoundException 404, TooManyRequestsException 429. Recorded behavior: after a successful TagResource on a group ARN, ListTagsForResource on that ARN answers 200 with Tags carrying exactly the key/value pairs that were applied.
Emulator: answers 200 with a body containing NO members at all ({}): the store holds no tags for the resource — TagResource is an unimplemented scaffold stub, so the write never lands — and the handler's (!tags.is_empty()) guard then suppresses even the empty map. Replay saw $: empty_body, expected {'Tags': {...}}, got {}. The specific keys/values in the fixture are its own request values, not part of the requirement; the requirement is that every tag applied to the resource is echoed back.
Why: tagging is unobservable end to end. Tag-based inventory, cost-allocation checks and permission scoping by tag all read an empty result and conclude the resource is untagged.
Fix: the tag store write path behind TagResource/UntagResource (tracked by the stub issues) plus the read projection in src/ops/list_tags_for_resource.rs; the lookup itself (state::tags_of_resource) is the right seam — the Some(empty) case must serialize {} rather than drop the member.

=== Cause 3 of 3: GetGroup omits Group.Tags for a tagged group ===
AWS: Smithy: GetGroup (GET /group/{GroupIdentifier}) -> GetGroupResponse.Group -> Group with member Tags (TagMap). Recorded behavior: reading back a group that has been tagged answers 200 with Group.Tags holding that group's current tag set, i.e. GetGroup is a second, equivalent view of what ListTagsForResource reports for the group's ARN.
Emulator: Group.Tags is absent (replay: $.Group.Tags missing, expected the two-entry map). Same two reasons as cause 2 — nothing persisted the tags, and the Group projection suppresses an empty map.
Why: a client that uses GetGroup as the single read for a group's configuration never sees its tags and cannot round-trip them; the group view and the tag view of the same resource disagree with AWS in the same direction.
Fix: StoredGroup::to_group in src/state.rs (shared with cause 1) once the tag write path lands, so GetGroup and ListTagsForResource are projections of one stored map.

Traces: synthetics-group-canary-association-errors#setup#1 (cause 1, also seen as synthetics-group-error-paths#setup#1), synthetics-tag-resource-and-list-tags-roundtrip#task#3 (cause 2), synthetics-tag-resource-and-list-tags-roundtrip#task#4 (cause 3).

## Evidence
Corpus simparity-wire-aws-v1, 33 steps, 19 passed, score 57.6%.

Cause 1 — fixture synthetics-group-canary-association-errors, step setup#1, CreateGroup, expected 200 / actual 200 (cluster also covers synthetics-group-error-paths#setup#1):
  $.Group.Tags: missing (expected {}, got None)
  Source: src/state.rs StoredGroup::to_group, called from src/ops/create_group.rs; the same empty-map-as-absent rule is written out in src/ops/list_tags_for_resource.rs lines ~159-166.

Cause 2 — fixture synthetics-tag-resource-and-list-tags-roundtrip, step task#3, ListTagsForResource, expected 200 / actual 200:
  $: empty_body (expected {'Tags': {'env': 'test', 'team': 'sim2real'}}, got {})
  The preceding TagResource step in the same fixture is an unimplemented stub (see open issue 20261029T184613Z-synthetics-tagresource-stub.md), so nothing is persisted; the read path then also drops the member.

Cause 3 — fixture synthetics-tag-resource-and-list-tags-roundtrip, step task#4, GetGroup, expected 200 / actual 200:
  $.Group.Tags: missing (expected {'env': 'test', 'team': 'sim2real'}, got None)

Model: Group.Tags -> TagMap (map, @length 1..50, key TagKey @pattern ^(?!aws:)[a-zA-Z+-=._:/]+$, value TagValue @length 0..256); ListTagsForResourceResponse.Tags -> TagMap. The tag keys/values quoted above are this fixture's request values and are not themselves a requirement.

## Steps to Reproduce
Cause 1: CreateGroup(Name=g) with no Tags -> 200; the response's Group has no Tags member at all, where AWS returns "Tags": {}.

Cause 2: CreateGroup(Name=g); TagResource(ResourceArn=<group arn>, Tags={k1:v1, k2:v2}); ListTagsForResource(ResourceArn=<group arn>) -> 200 with an empty body, where AWS returns Tags with k1/k2.

Cause 3: continuing from cause 2, GetGroup(GroupIdentifier=g) -> 200 with Group lacking Tags, where AWS returns Group.Tags = {k1:v1, k2:v2}.
