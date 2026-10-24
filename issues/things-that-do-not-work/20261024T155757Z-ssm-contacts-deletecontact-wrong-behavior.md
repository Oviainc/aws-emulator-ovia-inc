Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# DeleteContact deletes a contact that is a member of a rotation instead of answering ConflictException, leaving the rotation pointing at a dead ARN

_Filed: 2026-10-24T15:57:57Z_
_Operation: DeleteContact | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A contact that is a member of an on-call rotation is a resource that another resource depends on. Incident Manager refuses to delete it: the call answers 409 ConflictException whose DependentEntities names the rotation (RelationType plus the rotation's ARN in DependentResourceIds), and the contact stays.

The emulator's DeleteContact only looks for dependent CONTACT CHANNELS. Rotation membership is never consulted, so the delete answers 200 and removes the contact while `RotationRecord::contact_ids` keeps its ARN. The store is then internally inconsistent in a way the API itself exposes:

* GetRotation / ListRotations keep reporting a ContactIds entry that GetContact answers 404 for;
* CreateRotation and CreateRotationOverride in this same crate REFUSE a ContactIds/NewContactIds entry that names no stored contact (ResourceNotFoundException), so the emulator will not let a caller create the state it has just produced by deleting;
* a caller cannot tell from the rotation read which member is gone.

Expected: DeleteContact on a contact that appears in any rotation's ContactIds (and on a rotation override's NewContactIds, which is stored the same way) answers 409 ConflictException with ResourceId = the contact ARN, ResourceType = contact, and DependentEntities listing the rotation ARN(s); the contact remains readable. Actual: HTTP 200, contact removed, rotation left with a dangling member ARN.

Note the contrary case is already right and must stay: the model's own DeleteContact doc says deleting a contact does NOT remove it from escalation plans or response plans, so a contact referenced only by another contact's Plan stages must still delete successfully.

## Evidence

Spec: the model's `DependentEntity` shape documentation states the rule verbatim — "Information about a resource that another resource is related to or depends on. For example, if a contact is a member of a rotation, the rotation is a dependent entity of the contact." (smithy_query shape=com.amazonaws.ssmcontacts#DependentEntity). DeleteContact declares ConflictException (HTTP 409) which carries that DependentEntities list, and DeleteContact's own doc scopes the non-blocking case to escalation plans/response plans only, not rotations.

Handler: services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/delete_contact.rs lines 195-219 — the only dependency scan is `state.channel_arns_of(&input.contact_id)`; after it the handler unconditionally does `state.contacts.remove(...)` / `alias_index.remove(...)`. src/state.rs offers no rotation-membership lookup at all (`rotation_overrides_of`, `channel_arns_of` and `tags_of` are the only cross-resource helpers), and `RotationRecord::contact_ids` is never pruned by any delete (contrast DeleteRotation, src/ops/delete_rotation.rs lines 191-195, which does prune `plan.rotation_ids` when a rotation goes away).

Tests: tests/all/contact_lifecycle.rs:600 `a_contact_with_channels_refuses_deletion_until_the_channels_are_gone` asserts ONLY the contact-channel conflict. tests/all/rotation_deletes.rs covers the opposite direction (deleting a rotation prunes plan references). No test creates a rotation and then deletes one of its member contacts, so nothing asserts today's 200 — the gap is unguarded in either direction.

## Steps to Reproduce

1. Onboard the account (seed an ACTIVE Incident Manager replication set) so the data plane is open.
2. CreateContact with Alias=oncall-member, Type=PERSONAL, Plan={} and keep the returned ContactArn.
3. CreateRotation with Name=primary, ContactIds=[<that ContactArn>], TimeZoneId=UTC and a Recurrence carrying DailySettings; keep the RotationArn.
4. Call DeleteContact with ContactId=<that ContactArn>. Observed: HTTP 200. Expected: HTTP 409 ConflictException whose DependentEntities names the rotation ARN, with the contact still present.
5. Call GetRotation with RotationId=<the RotationArn>: ContactIds still contains the deleted contact's ARN.
6. Call GetContact with that same ARN: HTTP 404 ResourceNotFoundException — the rotation now reports a member that does not exist, and CreateRotation would itself refuse that ARN.
