Issue-Source: trace-sweep-divergence
Issue-Trace: ram-principal-association-workflow
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ram-principal-association-workflow ram-resource-share-tagging
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram|DisassociateResourceShare|2xx->2xx|$.resourceShareAssociations[*].external
Issue-Key: ram|TagResource|2xx->2xx|$.tagsCreated[*].__type
Issue-Key: ram|UntagResource|2xx->2xx|$.deletedTags[*].__type
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# ram: 3 net-new response-fidelity causes in resource-share association and tagging responses

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateResourceShare

## Description
Replaying the RAM wire fixtures turns up three net-new divergences in the 200 response bodies of the resource-share association and tagging operations. All three are body-shape/field-value defects on an otherwise correct status code, so a client sees a successful call carrying wrong or incomplete data. Three causes, all in service `ram`:

Cause 1 of 3: DisassociateResourceShare reports every returned association as external.
  AWS: Smithy: DisassociateResourceShare -> DisassociateResourceShareResponse$resourceShareAssociations (list of ResourceShareAssociation). ResourceShareAssociation$external is a Boolean documented as "Indicates whether the principal belongly to the same organization in Organizations as the Amazon Web Services account that owns the resource share." For a principal association whose principal is inside the resource-share owner's own organization — the case the recorded workflow exercises, where the share and the principal belong to the caller's account/organization — AWS returns external=false. The recorded response for this step carries external=false. Remaining members of the association (resourceShareArn, associatedEntity, associationType, status, lastUpdatedTime) matched.
  Emulator: returns external=true for that same association. Replay detail: `$.resourceShareAssociations[0].external: value_mismatch (expected False, got True)`, HTTP 200 -> 200.
  Why: `external` is the flag a client uses to distinguish an organization-internal share from one shared outside the organization. Reporting true unconditionally makes every disassociated principal look like an out-of-organization principal, so governance/reporting code that branches on it (or filters associations by it) draws the opposite conclusion from the real service, and an org-membership check built on RAM's answer always fails.
  Fix: the DisassociateResourceShare handler / the association record in the RAM state store. `external` must be derived from whether the association's principal is in the same organization as the resource share's owning account, not emitted as a constant; the same derivation has to be used wherever ResourceShareAssociation is serialized (Associate/Disassociate/GetResourceShareAssociations) so the value is consistent across reads.
  Traces: ram-principal-association-workflow#task#5 (DisassociateResourceShare).

Cause 2 of 3: TagResource's returned Tag entries omit the structure type discriminator.
  AWS: Smithy: TagResource -> TagResourceResponse$tagsCreated (TagList of Tag{key,value}); the model itself says nothing about a discriminator member, so: recorded behavior only — the recorded RAM response serializes each element of `tagsCreated` with a `__type` member whose value is the structure name `Tag`, alongside key/value. Status 200.
  Emulator: answers 200 with the tag element present but no `__type` member at all. Replay detail: `$.tagsCreated[0].__type: missing (expected 'Tag', got None)`. The member name `__type` and its value `Tag` are both fixed text — the structure name, not a per-run value; only key/value vary with the request.
  Why: a client or SDK/middleware that reads the AWS JSON structure discriminator to decide how to deserialize a list element gets an untyped object, so the element is dropped or deserialized as an unknown shape and the tags the call just created read back as absent. Any byte- or shape-level comparison against the real service also fails.
  Fix: the RAM response serializer for TagList/Tag (shared by every operation that returns Tag), so the discriminator is emitted for every Tag element rather than patched into one handler.
  Traces: ram-resource-share-tagging#task#2 (TagResource).

Cause 3 of 3: UntagResource's returned Tag entries omit the same discriminator.
  AWS: Smithy: UntagResource -> UntagResourceResponse$deletedTags (TagList of Tag). Recorded behavior only, same as Cause 2: each element of `deletedTags` is serialized with `__type` = `Tag` plus key/value. Status 200.
  Emulator: answers 200 with `deletedTags[0]` lacking `__type`. Replay detail: `$.deletedTags[0].__type: missing (expected 'Tag', got None)`.
  Why: identical client consequence to Cause 2 — the deleted-tag entries cannot be typed by a discriminator-driven deserializer, so the confirmation of which tags were removed is lost.
  Fix: same component as Cause 2, the RAM Tag/TagList serializer; listed as its own cause because it is a separate operation and a separate replay step that must be re-run to confirm the fix.
  Traces: ram-resource-share-tagging#task#4 (UntagResource).

No open issue covers RAM resource-share association or tagging responses (searched the open issues for 'ram' and 'resource-share'); none of the three clusters was reported as blocked by an unimplemented operation or owned by an existing issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, data listener accepting; bootstrap_signal=false), 3 divergence clusters, all service=ram, all verdict=mismatch with expected_status 200 and actual_status 200:

1) fixture ram-principal-association-workflow, step task#5, DisassociateResourceShare
   detail: $.resourceShareAssociations[0].external: value_mismatch (expected False, got True)
   divergence_key: service=ram|DisassociateResourceShare|status=200->200|keys=$.resourceShareAssociations[0].external
   Expected: resourceShareAssociations[0].external = false (principal in the share owner's organization)
   Actual:   resourceShareAssociations[0].external = true

2) fixture ram-resource-share-tagging, step task#2, TagResource
   detail: $.tagsCreated[0].__type: missing (expected 'Tag', got None)
   divergence_key: service=ram|TagResource|status=200->200|keys=$.tagsCreated[0].__type
   Expected: tagsCreated[0] includes member "__type" with the fixed value "Tag" (structure name) in addition to key/value
   Actual:   tagsCreated[0] has no "__type" member

3) fixture ram-resource-share-tagging, step task#4, UntagResource
   detail: $.deletedTags[0].__type: missing (expected 'Tag', got None)
   divergence_key: service=ram|UntagResource|status=200->200|keys=$.deletedTags[0].__type
   Expected: deletedTags[0] includes member "__type" with the fixed value "Tag"
   Actual:   deletedTags[0] has no "__type" member

Model facts used (services/service-com-amazonaws-ram-amazonresourcesharing/smithy-model/com-amazonaws-ram-amazonresourcesharing.json):
- DisassociateResourceShare: POST /disassociateresourceshare, 200; output resourceShareAssociations: ResourceShareAssociationList, clientToken: String.
- ResourceShareAssociation$external: Boolean — "Indicates whether the principal belongs to the same organization in Organizations as the Amazon Web Services account that owns the resource share."
- TagResource: POST /tagresource, 200; output tagsCreated: TagList. UntagResource: POST /untagresource, 200; output deletedTags: TagList. TagList member = Tag{key: TagKey, value: TagValue}; the model declares no discriminator member, hence the "recorded behavior only" basis for the __type requirement.

## Steps to Reproduce
1. git clone the emulator repo and boot the emulator from that checkout (RAM is bootstrapped: service-com-amazonaws-ram-amazonresourcesharing).
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: replay fixture ram-principal-association-workflow up to step task#5 — create a resource share, associate a principal inside the caller's own organization, then call DisassociateResourceShare with that resourceShareArn and principal. Inspect the 200 body: resourceShareAssociations[0].external comes back true; the recording has false.
4. Cause 2: replay fixture ram-resource-share-tagging up to step task#2 — call TagResource with the share's resourceShareArn and one tag. Inspect the 200 body: tagsCreated[0] carries key/value but no "__type": "Tag" member.
5. Cause 3: continue the same fixture to step task#4 — call UntagResource with that tag's key. Inspect the 200 body: deletedTags[0] likewise has no "__type": "Tag" member.
6. Re-replay ram-principal-association-workflow#task#5, ram-resource-share-tagging#task#2 and ram-resource-share-tagging#task#4 after the fix; all three must pass before this issue is closed.

## Resolution notes

All three causes are FIXED. Two of the report's claims about what AWS does are
contradicted by the recording itself, and the fix follows the recording:

### Causes 2 and 3 — `__type` on a tag-mutation entry

RULE: `TagResource$tagsCreated` and `UntagResource$deletedTags` are serialized as
self-describing JSON documents — every entry carries the AWS JSON structure
discriminator `__type` with the serialized structure's own local name — while a `Tag`
reported as part of a resource's STATE (`ResourceShare$tags`,
`ResourceSharePermission*$tags`) carries the modelled members alone.

The report asks for the discriminator "for every Tag element". The recording disagrees:
the SAME fixture reports `ResourceShare$tags` WITHOUT it (those steps replayed clean
before this change and still do), so the discriminator belongs to the two
tag-mutation members, not to the `Tag` shape. Implemented in
`src/ops/types/tag_wire.rs`, which states the schema and serializer of the two response
shapes once and points their element writer at a document view of `Tag`.

### Cause 1 — `external` on a teardown report

RULE: `DisassociateResourceShare` answers with the association records it REMOVES from
the share; the call leaves no principal participating in the share, so there is no
principal to place in the owning account's organization and every record it reports
carries `external = false`. The stored derivation (the entity against the share's
owning account) is untouched, and every operation that reports an association still
participating in the share reports it.

The report asks for one derivation used by Associate/Disassociate/Get alike. Two
controlled replays show the recording does not work that way: forcing every principal
association to `false` makes `AssociateResourceShare` (task#2) and
`GetResourceShareAssociations` diverge, and deriving the flag from the record's status
makes `GetResourceShareAssociations` (task#6, AFTER the teardown) diverge — so the
recorded `external` is the derived value everywhere except in the teardown's own report.
Implemented as `state::AssociationRecord::removal_view`, used by both the fresh and the
idempotent-repeat path of the teardown handler.

### Tests

`tests/all/document_entries_and_removal_reports.rs` (new, declared in `tests/all.rs`)
asserts both rules over tables: the document rule over the length and shape of the
reported list, with each entry's discriminator-stripped object matched against the pair
the share reports through `GetResourceShares`; the teardown rule over entity form,
association type and how many of each one call names — including a row whose entities
derive BOTH values — plus the idempotent repeat and the read-back that shows the stored
flag unmoved. `tests/all/response_payload_fidelity.rs` and
`tests/all/mutation_reports.rs` carried the opposite expectations (they were written when
these causes were declined) and now assert these rules.
