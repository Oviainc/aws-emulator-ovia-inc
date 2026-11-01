Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 15a7913299c0ebfe5f22c92412d933bd37150a6d

# Intent=VALIDATE skips the (entity, change type) uniqueness check for a change that resolves through a change reference

_Filed: 2026-10-29T16:15:23Z_
_Operation: StartChangeSet | Class: validation_

> _Note: this issue was identified against the code base at commit `15a7913299c0ebfe5f22c92412d933bd37150a6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartChangeSet enforces the documented rule "Within a single ChangeSet, you can't start the same change type against the same entity multiple times" in two passes: a first pass keyed on the identifier each change NAMES, and a second pass keyed on the entity each change RESOLVED to. The second pass is the only one that can see two changes reaching one entity when one of them names the other by `$<ChangeName>.Entity.Identifier`, and it is guarded by `if let Some(entity_id) = entity_id.as_deref()`. For `Intent=VALIDATE`, a change that names no identifier is deliberately left with `entity_id == None` (`None => None` instead of minting an id), so both the creating change and every change referencing it carry `None` and the second pass never runs for them. The result: a change set that repeats one change type against the entity one of its own changes creates is REFUSED with 422 `ValidationException` under `Intent=APPLY` and ACCEPTED with 200 under `Intent=VALIDATE` — i.e. the mode whose whole purpose is "to test your request before applying changes to your entities" is the one that fails to report this contradiction. The first pass cannot cover it: the creating change carries no identifier at all and is skipped there (`continue`).

## Evidence

Model sentence, reproduced in the handler header from the Smithy model — services/.../src/ops/start_change_set.rs:48-50: "//! Allows you to request changes for your entities. Within a single ChangeSet, you can't\n//! start the same change type against the same entity multiple times." (also quoted on the fault builder, src/state.rs:3246-3248: "`StartChangeSet` documents the rule (\"Within a single `ChangeSet`, you can't start the same change type against the same entity multiple times\")").

The code's own comment states the rule the code then skips — src/ops/start_change_set.rs:649-660:
```
            if let Some(entity_id) = entity_id.as_deref() {
                // "Within a single ChangeSet, you can't start the same change
                // type against the same entity multiple times" — judged here on
                // the entity the changes RESOLVED to as well as above on the one
                // each named, because two changes reach one entity whenever one
                // of them names the other or the entity it creates.
                if !claimed_entities.insert((entity_id.to_string(), change.change_type.clone())) {
                    return Err(StartChangeSetErrorModeled::ValidationException(
                        crate::state::duplicate_change_pair(&change.change_type, entity_id),
```
and the resolution that makes `entity_id` `None` for VALIDATE — src/ops/start_change_set.rs:639-642:
```
                        None if intent == crate::state::INTENT_APPLY => {
                            Some(store.mint_entity_id())
                        }
                        None => None,
```
The first pass cannot catch it — src/ops/start_change_set.rs:320-323 skips a change that names no identifier: `let Some(identifier) = change.entity.identifier.as_deref() else { // A change naming no identifier creates an entity of its own, so\n                // it can take no pair with another change.\n                continue; };`

Test coverage: tests/all/change_pair_uniqueness.rs:363-419 covers the change-reference form only when BOTH copies name the reference (`change("AddRevisions", "SaaSProduct", Some(identifier))` twice with `identifier = "$Create.Entity.Identifier"`), which the FIRST pass catches; no row pairs a creating change with a change referencing it, and no row uses `Intent=VALIDATE`. tests/all/entity_id_list_contract.rs:304-390 exercises `Intent=VALIDATE` ("VALIDATE, two change types reaching one existing entity") but never a repeated pair, so nothing pins the behaviour described here.

## Steps to Reproduce

1. POST /StartChangeSet with body {"Catalog":"AWSMarketplace","Intent":"APPLY","ChangeSet":[{"ChangeType":"AddRevisions","ChangeName":"Create","Entity":{"Type":"AmiProduct"}},{"ChangeType":"AddRevisions","Entity":{"Type":"AmiProduct","Identifier":"$Create.Entity.Identifier"}}]} — the second pass resolves both changes to the minted entity id and answers 422 ValidationException "Requested changes contain multiple changes for the same change type 'AddRevisions' on entity '…'. Entity - change type pairs should be unique."
2. Repeat the identical request with "Intent":"VALIDATE" — expected the same 422 (the rule is about the request, and VALIDATE exists to test the request), actual 200 with a fresh ChangeSetId: the creating change gets `entity_id = None` (start_change_set.rs:642), so `claimed_entities` is never consulted for either change.
3. GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> — the stored change set holds both contradicting changes and reports SUCCEEDED, so the validation never happens at all.
