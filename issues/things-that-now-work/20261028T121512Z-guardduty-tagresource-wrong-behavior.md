Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# TagResource and UntagResource write tags onto a sub-resource whose delete has already been accepted, which the crate's own rule says is "only reported, never used"

_Filed: 2026-10-28T12:15:12Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate holds one rule for a detector sub-resource (IPSet, ThreatIntelSet, trusted/threat entity set) whose delete has been accepted: the record is RETAINED only so its own Get can report the DELETE_PENDING/DELETED status, it is dropped from every listing, and every operation that would ACT on it refuses it exactly like an id the detector never held. The tag surface is the one path that ignores that rule: `tagged_resource_tags_mut` resolves the sub-resource with a plain `get_mut(id)` and no `is_deleting` filter, so `TagResource` merges tags into — and `UntagResource` removes tags from — a record the crate has already declared out of use. The refusal path for exactly this case already exists (`crate::state::resource_arn_rejection`, which answers the "not owned" message for a well-formed ARN this endpoint resolves nothing for), and the `is_deleting` predicate the sibling updates use is already public. The consequence is a stored write (and a changed tag map reported by `GetIPSet` / `ListTagsForResource`) against a resource that `ListIPSets` no longer names and that `UpdateIPSet` refuses to touch — two reads of one store that disagree about whether the resource can still be acted on. The READ side (`ListTagsForResource`) is not the defect: `GetIPSet` deliberately reports a deleting record, so reporting its tags is consistent; only the two WRITES break the rule.

## Evidence

The rule, stated in this crate — services/service-com-amazonaws-guardduty-guarddutyapiservice/src/state.rs:3056-3058: "/// This is the count [`IP_SETS_PER_DETECTOR`] bounds: a list whose delete has\n/// been accepted is only reported, never used, so it no longer occupies one of\n/// the detector's slots." and src/state.rs:3039-3045: "a list whose delete has been accepted is no longer one of the detector's IPSets, so `ListIPSets` stops naming it ... being readable on its own id is not the same as being part of the collection a list enumerates."

The sibling handler that enforces it — src/ops/update_ip_set.rs:180-194: "// A list whose delete has already been accepted is no longer one the account\n// can act on, so it is refused exactly like an id the detector never held ... while an\n// update would otherwise walk a DELETED record back into use." followed by `let ip_set = detector.ip_sets.get_mut(&input.ip_set_id).filter(|ip_set| !ip_set.is_deleting());` and, when it is `None`, `Err(... ip_set_id_rejection(..))`. The same check is in src/ops/update_threat_entity_set.rs:202-215, src/ops/update_trusted_entity_set.rs:209 and src/ops/update_threat_intel_set.rs:175 (`.is_none_or(crate::state::EntitySetRecord::is_deleting)`).

The path that skips it — src/state.rs:8179-8189 (`tagged_resource_tags_mut`): `TaggedSubResource::Filter => detector.filters.get_mut(id).map(|f| &mut f.tags), TaggedSubResource::IpSet => detector.ip_sets.get_mut(id).map(|s| &mut s.tags), TaggedSubResource::ThreatIntelSet => detector.threat_intel_sets.get_mut(id).map(|s| &mut s.tags), ...` — no `is_deleting` filter on any arm.

The writes that use it — src/ops/tag_resource.rs:162 `let Some(tags) = state.tagged_resource_tags_mut(&region, &input.resource_arn) else { ... resource_arn_rejection ... }` then :191-193 `for (key, value) in &input.tags { tags.insert(key.clone(), value.clone()); }`; src/ops/untag_resource.rs:157 and :171-173 `for key in &input.tag_keys { tags.remove(key); }`.

Test coverage: tests/all/resource_tag_addition.rs and tests/all/resource_tag_removal.rs drive both operations against every taggable resource class, but every resource in the fixture is freshly created — no case deletes a sub-resource first, so nothing pins the current acceptance.

## Steps to Reproduce

1. CreateDetector in the request Region and note the detectorId.
2. CreateIPSet on that detector with {"tags": {"Env": "prod"}} and note the ipSetId.
3. DeleteIPSet for that ipSetId — accepted; the record is retained (DELETED_IP_SETS_RETAINED) and ListIPSets no longer names it, while GetIPSet still reports it with the DELETE_PENDING/DELETED status.
4. Confirm the crate refuses other actions on it: UpdateIPSet with the same ipSetId is refused HTTP 400 with the ipSetId rejection.
5. POST /tags/arn%3Aaws%3Aguardduty%3A<region>%3A<account>%3Adetector%2F<detectorId>%2Fipset%2F<ipSetId> with {"tags": {"Owner": "me"}}.
6. Expected per the crate's own rule: refused like an id the detector never held (HTTP 400, InvalidInputException naming resourceArn). Actual: HTTP 204, and GetIPSet / ListTagsForResource now report the new tag on the deleted IPSet. The same holds for UntagResource against the same ARN.
