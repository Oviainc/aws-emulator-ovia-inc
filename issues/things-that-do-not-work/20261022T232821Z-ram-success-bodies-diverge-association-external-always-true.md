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
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# RAM success bodies diverge: association `external` always true for non-org principals, and Tag entries omit the `__type` discriminator

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateResourceShare

## Description
Three net-new wire divergences in the RAM (AmazonResourceSharing, restJson1, API version 2018-01-04) emulator. All three are 200-vs-200 BODY divergences: the status and the error channel are right, the serialized success document is not. They share one fault family — a member of a nested structure the emulator fills in from its own derivation instead of reporting what RAM reports — so they are bundled. THREE causes, all must be closed.

Cause 1 of 3: DisassociateResourceShare reports `resourceShareAssociations[].external = true` for a principal AWS reports as non-external.
  AWS: Smithy: DisassociateResourceShare output `resourceShareAssociations` -> ResourceShareAssociationList -> ResourceShareAssociation; member `external` (Boolean) "Indicates whether the principal belongs to the same organization in Organizations as the Amazon Web Services account that owns the resource share." The recorded response for this step is HTTP 200 with `resourceShareAssociations[0].external = false` — i.e. RAM placed the disassociated principal INSIDE the owning account's organization. The model states no algorithm for deriving the flag, so beyond the member's documentation this is recorded behavior only.
  Emulator: HTTP 200, same association record, but `external = true`.
  Why: `external` is the only signal a client has for whether a share is reaching outside its organization. A consumer that branches on it (external-sharing dashboards, compliance checks that assert no external principal, `allowExternalPrincipals` reconciliation logic) sees an in-organization principal reported as an external one and reports a false positive on every association it reads back. Because the flag is stored on the association record at attach time and merely cloned on teardown, every later read of the same association (GetResourceShareAssociations, AssociateResourceShare retry) carries the same wrong value.
  Hypothesis: the emulator has no notion of organization membership at all. `entity_is_external(owner, entity)` returns false only when the entity is itself the owning account (bare id, or an ARN whose account field equals the owner) or is syntactically an Organizations-scoped entity (`organizations::...:organization/ou/root`, or a bare `o-`/`ou-`/`r-` id); every other principal — another account, an IAM principal inside one, a service principal — is defaulted to external. A principal that AWS resolves as in-organization through actual Organizations membership therefore cannot be reported as non-external, and a share whose owning account id the emulator derives from the request credentials rather than from the recorded owner compounds it, because the owner-equality test is then made against the wrong account.
  Fix: the state slice that derives the flag, not the handler — the association record's `external` is set where the association is created/refreshed and then only cloned by the teardown path, so the derivation is what must change (make organization membership a modelled property of the stored principal/share rather than inferring it from the entity string).
  Traces: ram-principal-association-workflow#task#5 (DisassociateResourceShare, `$.resourceShareAssociations[0].external`: expected false, got true).

Cause 2 of 3: TagResource omits the `__type` discriminator on every entry of `tagsCreated`.
  AWS: Smithy: TagResource -> TagResourceResponse, member `tagsCreated` (TagList of `com.amazonaws.ram#Tag`, members `key`/`value` only). The model declares no `__type` member — it is the AWS JSON structure discriminator, so the model says nothing about it and this is recorded behavior only: the recorded 200 response carries `tagsCreated[0].__type = "Tag"` alongside the key/value pair.
  Emulator: HTTP 200 with `tagsCreated[0]` present but no `__type` member (`missing (expected 'Tag', got None)`).
  Why: a client that reads the response document generically — a discriminator-driven deserializer, a proxy that re-serializes the body, a fixture/assertion suite recorded against RAM — sees a structurally different object for the same operation, and anything keyed on the discriminator finds nothing to dispatch on. The tag data itself is correct, so this is a body-shape divergence rather than a data one.
  Fix: the serializer side. The handler already reads the pair back out of the share it wrote; the shortfall is that the emitted `Tag` structure carries no discriminator. Like `TagResourceResponse$tagsCreated` and `ResourceShareConfiguration$exclusiveAccountAccess` before it (both documented in the crate as members added to the shipped model to match this corpus), the schema-driven serializer cannot emit what the model does not declare, so closing this means teaching the `Tag` serialization to carry the discriminator rather than patching one handler.
  Traces: ram-resource-share-tagging#task#2 (TagResource, `$.tagsCreated[0].__type`).

Cause 3 of 3: UntagResource omits the same `__type` discriminator on every entry of `deletedTags`.
  AWS: Smithy: UntagResource -> UntagResourceResponse, member `deletedTags` (the same TagList of `com.amazonaws.ram#Tag`). Again `__type` is not modelled, so recorded behavior only: the recorded 200 response carries `deletedTags[0].__type = "Tag"`.
  Emulator: HTTP 200 with `deletedTags[0]` present but no `__type` member.
  Why: same as cause 2, on the removal path — the two tag mutations are the two places RAM reports Tag structures back to the caller, so a client gets the discriminator-free shape on both.
  Fix: the same `Tag` serialization as cause 2; listed as its own cause because it is a separate operation and a separate replay step, and a fix to only one of the two response members leaves this step still diverging.
  Traces: ram-resource-share-tagging#task#4 (UntagResource, `$.deletedTags[0].__type`).

Scope notes: no other RAM step diverged (29 of 90 steps passed; the remaining 32 blocked steps are unimplemented-operation stubs whose issues are filed separately, and the replay reported no state-reset failures, so none of the three causes above is contaminated by a previous fixture's state). None of these three is claimed by an existing open issue — the open list contains no RAM entry.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, /__admin/health 200, state_reset_failures=0, bootstrap_signal=false). 3 divergence clusters, all service=ram, all verdict=mismatch with expected_status 200 and actual_status 200:

1) fixture ram-principal-association-workflow, step task#5, DisassociateResourceShare
   divergence_key: service=ram|DisassociateResourceShare|status=200->200|keys=$.resourceShareAssociations[0].external
   detail: $.resourceShareAssociations[0].external: value_mismatch (expected False, got True)
   Expected: 200 body with resourceShareAssociations[0].external = false
   Actual:   200 body with resourceShareAssociations[0].external = true

2) fixture ram-resource-share-tagging, step task#2, TagResource
   divergence_key: service=ram|TagResource|status=200->200|keys=$.tagsCreated[0].__type
   detail: $.tagsCreated[0].__type: missing (expected 'Tag', got None)
   Expected: 200 body whose tagsCreated[0] carries __type = "Tag"
   Actual:   200 body whose tagsCreated[0] has no __type member

3) fixture ram-resource-share-tagging, step task#4, UntagResource
   divergence_key: service=ram|UntagResource|status=200->200|keys=$.deletedTags[0].__type
   detail: $.deletedTags[0].__type: missing (expected 'Tag', got None)
   Expected: 200 body whose deletedTags[0] carries __type = "Tag"
   Actual:   200 body whose deletedTags[0] has no __type member

Model evidence (services/service-com-amazonaws-ram-amazonresourcesharing/smithy-model/com-amazonaws-ram-amazonresourcesharing.json): sdkId RAM, protocol restJson1, version 2018-01-04. ResourceShareAssociation$external (Boolean) documented as "Indicates whether the principal belongs to the same organization in Organizations as the Amazon Web Services account that owns the resource share." com.amazonaws.ram#Tag declares members `key` and `value` only — no `__type`. TagResource output member `tagsCreated` (TagList); UntagResource output member `deletedTags` (TagList).

Code evidence for cause 1: services/service-com-amazonaws-ram-amazonresourcesharing/src/state.rs:977 `entity_is_external(owner, entity)` returns false only for an Organizations-scoped entity or an entity equal to the owning account, and its own comment states "every other entity ... is one this service cannot place inside the owner's organization, and an association it cannot place there is reported as external". The flag is written at state.rs:511 and :522 (associate path) and merely cloned by the teardown path (state.rs:552 `disassociate`, surfaced through `AssociationRecord::view` at state.rs:352), so the handler at src/ops/disassociate_resource_share.rs:372-380 reports whatever the attach derived.

## Steps to Reproduce
Prerequisite: build/boot the emulator from mainline and replay trace set simparity-wire-aws-v1 (replay_wire_corpus). No state preconditioning is needed beyond the fixtures themselves.

Cause 1: replay fixture ram-principal-association-workflow up to step task#5. The fixture creates a resource share, associates a principal with it, then calls DisassociateResourceShare naming that principal. Compare the 200 response's resourceShareAssociations[0].external against the recorded value: the recording says false, the emulator answers true. The same wrong value is observable on any later read of that association (e.g. GetResourceShareAssociations for the same share/principal).

Causes 2 and 3: replay fixture ram-resource-share-tagging. At step task#2 POST /tagresource attaches a tag to the stored share and answers 200 with tagsCreated; at step task#4 POST /untagresource removes it and answers 200 with deletedTags. Inspect each returned Tag object: the recorded bodies carry __type = "Tag" on every entry, the emulator's bodies carry only key and value.

Confirming a fix: re-replay ram-principal-association-workflow#task#5 (cause 1) AND ram-resource-share-tagging#task#2 and #task#4 (causes 2 and 3). All three must pass; closing only the Tag serialization leaves cause 1 diverging, and vice versa.
