Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# Intent=VALIDATE skips the pair-uniqueness, self-contradiction and EntityTags checks for every change that creates an entity, so a request APPLY refuses or fails is reported SUCCEEDED

_Filed: 2026-10-28T13:35:34Z_
_Operation: StartChangeSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Intent=VALIDATE` exists to test a request before it is applied, so it must reach the same verdict the same request would reach with `Intent=APPLY`. In this crate it does not. Every check downstream of "which entity does this change act on" is keyed on `ChangeRecord::entity_id`, and `StartChangeSet` only populates that field when the intent is APPLY: a change that names no `Entity.Identifier` (a create) gets `Some(minted_id)` under APPLY and `None` under VALIDATE. Three rules then silently do nothing for a VALIDATE change set:

1. the "same change type against the same entity" refusal in `StartChangeSet`'s second pass (guarded by `if let Some(entity_id) = entity_id.as_deref()`), so a change set that APPLY refuses 422 is accepted 200 under VALIDATE;
2. `contradicting_details`, which `continue`s on `entity_id == None`, so a change set whose two changes claim different `ProductTitle`/`Visibility` for the same entity is reported SUCCEEDED with no `FailureDescription` under VALIDATE while APPLY reports FAILED/CLIENT_ERROR;
3. the `EntityTags` ceiling in `judge_change_set`, which `continue`s on the same condition.

The net effect is the opposite of what VALIDATE is for: the intent whose job is to surface a defect before it is applied is the intent that hides it, and it turns a refusal/failure into a success.

## Evidence

Smithy, `StartChangeSetRequest$Intent` (smithy_query doc): "The intent related to the request. The default is APPLY. To test your request before applying changes to your entities, use VALIDATE."

Smithy, `StartChangeSet` (smithy_query doc): "Within a single ChangeSet, you can't start the same change type against the same entity multiple times."

The fork that empties the field — services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/start_change_set.rs:597-601:
```
                        None if intent == crate::state::INTENT_APPLY => {
                            Some(store.mint_entity_id())
                        }
                        None => None,
```
and state.rs:650-654 documents the consequence: "Absent for a change of a `VALIDATE` change set that names no existing entity, since a validation never creates one."

Rule 1 skipped — start_change_set.rs:607-618:
```
            if let Some(entity_id) = entity_id.as_deref() {
                // "Within a single ChangeSet, you can't start the same change
                // type against the same entity multiple times" — judged here on
                // the entity the changes RESOLVED to ...
                if !claimed_entities.insert((entity_id.to_string(), change.change_type.clone())) {
```

Rule 2 skipped — state.rs:2445-2451 (`contradicting_details`):
```
    for change in changes {
        let Some(entity_id) = change.entity_id.clone() else {
            continue;
        };
```

Rule 3 skipped — state.rs:1643-1647 (`judge_change_set`):
```
        for (index, change) in record.changes.iter().enumerate() {
            verdict.change_errors[index] = details_defects(change.details_value().as_ref());
            let Some(entity_id) = change.entity_id.as_deref() else {
                continue;
            };
```

Test coverage: tests/all/change_set_failures.rs:495-507 asserts a VALIDATE change set IS "what reports the defect before it is applied" — but only for `details_defects`, the one check above the `entity_id` guard. `grep -r VALIDATE tests/` finds no case at all for the pair rule, the contradiction rule or the tag ceiling under VALIDATE, and tests/all/change_pair_uniqueness.rs contains no `Intent` at all, so nothing pins the current behaviour.

## Steps to Reproduce

1. POST /StartChangeSet with {"Catalog":"AWSMarketplace","ChangeSet":[{"ChangeType":"CreateProduct","ChangeName":"A","Entity":{"Type":"AmiProduct"},"Details":"{\"ProductTitle\":\"one\"}"},{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct","Identifier":"$A.Entity.Identifier"},"Details":"{\"ProductTitle\":\"one\"}"}]} — observe 422 ValidationException (duplicate change type against the same entity), as StartChangeSet's doc requires.
2. Repeat the identical request with "Intent":"VALIDATE" added — observe 200 with a ChangeSetId: the refusal is gone, although nothing about the change set's content changed.
3. POST /StartChangeSet with two changes of DIFFERENT types that resolve to one entity and contradict each other: [{"ChangeType":"CreateProduct","ChangeName":"A","Entity":{"Type":"AmiProduct"},"Details":"{\"ProductTitle\":\"one\"}"},{"ChangeType":"AddRevisions","Entity":{"Type":"AmiProduct","Identifier":"$A.Entity.Identifier"},"Details":"{\"ProductTitle\":\"two\"}"}] with no Intent (APPLY), sleep past CHANGE_SET_PREPARING + CHANGE_SET_APPLYING, GET /DescribeChangeSet — observe Status FAILED with FailureCode CLIENT_ERROR and a FailureDescription naming both values.
4. Repeat that request with "Intent":"VALIDATE", sleep the same span, GET /DescribeChangeSet — observe Status SUCCEEDED with no FailureCode and no FailureDescription, so the validation run reports the request as sound while the apply run rejects it.
