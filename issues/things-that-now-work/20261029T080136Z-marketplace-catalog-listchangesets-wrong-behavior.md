Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 889b7a0ea20193cdb3c9577846ef2b90647eb8e8

# ListChangeSets reports an empty EntityIdList for an Intent=VALIDATE change set, which the member's own documentation forbids

_Filed: 2026-10-29T08:01:36Z_
_Operation: ListChangeSets | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `889b7a0ea20193cdb3c9577846ef2b90647eb8e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ChangeSetSummaryListItem$EntityIdList` is documented as always naming at least one entity, but the emulator serves `"EntityIdList": []` for every `Intent=VALIDATE` change set.

`StartChangeSet` reserves an entity id only when the intent is APPLY (`None if intent == crate::state::INTENT_APPLY => Some(store.mint_entity_id()), None => None` in start_change_set.rs:618-621), so every change of a VALIDATE change set that names no existing entity is stored with `entity_id: None`. `ChangeSetRecord::entity_ids()` therefore returns an empty vector and `to_summary()` hands it to the response member unconditionally (`entity_id_list: Some(self.entity_ids())`).

The crate already has the idiom for "a list the contract excludes empty" — `ListTagsForResource` omits the member rather than serving `[]` — but `EntityIdList` is emitted empty instead, so a client that reads the first element of a documented-non-empty list gets nothing for a VALIDATE change set.

## Evidence

Model (smithy_query query="shape" shape="ChangeSetSummaryListItem"), member `EntityIdList`: "This object is a list of entity IDs (string) that are a part of a change set. The entity ID list is a maximum of 20 entities. **It must contain at least one entity.**"

Handler/state side — the member is always present, empty included:
- `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:825`: `entity_id_list: Some(self.entity_ids()),`
- `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs:806-814`: `pub fn entity_ids(&self) -> Vec<String> { ... for id in self.changes.iter().filter_map(|c| c.entity_id.as_ref()) ... }` — empty when every change has `entity_id: None`
- `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/start_change_set.rs:618-621`: `None if intent == crate::state::INTENT_APPLY => { Some(store.mint_entity_id()) } None => None,`

Sibling rule in the same crate for a list whose contract excludes the empty case:
- `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/list_tags_for_resource.rs:188-191`: "`TagList` is `@length(min: 1 …)`, so a resource carrying no tags reports no `Tags` member rather than an empty list that bound excludes." / `tags: (!tags.is_empty()).then(|| tags.to_vec()),`

Test coverage: `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/tests/all/change_set_lifecycle.rs:781-784` currently pins the empty list — `assert_eq!(item["EntityIdList"], json!([]), "a validation reserves no entity, got {item:?}")` — whose stated reason ("a validation reserves no entity") is what the quoted model sentence contradicts.

## Steps to Reproduce

1. POST /StartChangeSet with {"Catalog":"AWSMarketplace","Intent":"VALIDATE","ChangeSet":[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct"},"Details":"{\"ProductTitle\":\"validated\"}"}]} — note the returned ChangeSetId
2. Wait for the change set to reach SUCCEEDED (CHANGE_SET_PREPARING + CHANGE_SET_APPLYING)
3. POST /ListChangeSets with {"Catalog":"AWSMarketplace"} and read the summary for that ChangeSetId
4. Observe `"EntityIdList": []` — a change set summary whose entity id list contains no entity, which `ChangeSetSummaryListItem$EntityIdList` documents as impossible ("It must contain at least one entity")
