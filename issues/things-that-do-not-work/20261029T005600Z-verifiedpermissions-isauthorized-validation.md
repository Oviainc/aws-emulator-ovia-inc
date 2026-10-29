Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d0484a4a393e00c66ab2f2d9da27b0764a19da5b

# An `entities` slice carrying action entities is accepted and evaluated, although the member's own documentation forbids actions

_Filed: 2026-10-29T00:56:00Z_
_Operation: IsAuthorized | Class: validation_

> _Note: this issue was identified against the code base at commit `d0484a4a393e00c66ab2f2d9da27b0764a19da5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`IsAuthorizedInput$entities` is documented to carry principal and resource entities ONLY — "you can't include actions" — but nothing on this operation's path ever looks at the entity TYPES a caller supplies. `entities_to_cedar_json` applies only a count bound, and `entity_item_to_cedar_json` copies each item's identifier, attributes, parents and tags into Cedar's entity JSON without inspecting what kind of entity it is; `build_entities` then falls back to an UNTYPED parse when the store's schema refuses the slice, so even a schema that declares the action set cannot stop it. The forbidden entity therefore reaches the authorizer and participates in the decision: an item whose identifier is the request's own action (e.g. `Action::"view"`) carrying `parents: [ActionGroup::"readers"]` puts the action into an action group the caller invented, so a policy written `action in ActionGroup::"readers"` can be satisfied by caller-supplied data instead of by the store's schema ("You must specify actions in the schema").

The crate already owns the mechanism for exactly this kind of refusal: `IsAuthorizedWithToken` scans the supplied slice's `/uid/type` and refuses an entity of a type the request may not supply, citing its own `entities` documentation. `IsAuthorized` (and `BatchIsAuthorized`, which shares `entities_to_cedar_json`/`build_entities`) applies no such check at all.

## Evidence

Model, `IsAuthorizedInput$entities` (smithy_query(query="doc", shape="IsAuthorizedInput", member="entities")): "(Optional) Specifies the list of resources and principals and their associated attributes ... You can include only principal and resource entities in this parameter; you can't include actions. You must specify actions in the schema."

Handler path, no type check anywhere:
- src/ops/is_authorized.rs:188-189 — `let entities = crate::state::cedar::entities_to_cedar_json(input.entities.as_ref()).map_err(invalid)?;` (the only thing done with `entities` before `authorize`).
- src/state/cedar.rs:1330-1341 — the `entityList` arm only counts items: `for item in items.iter().take(MAX_ENTITIES_PER_REQUEST + 1) { out.push(entity_item_to_cedar_json(item)?); }`.
- src/state/cedar.rs:1354-1396 — `entity_item_to_cedar_json` writes `"uid": {"type": item.identifier.entity_type, "id": item.identifier.entity_id}`, then `attrs`, `parents` and `tags`, with no inspection of `entity_type`.
- src/state/cedar.rs:923-935 — `build_entities`: "if let Some(schema) = schema { if let Ok(typed) = ...from_json_value(entities.clone(), Some(schema)) { return Ok(typed); } } cedar_policy::Entities::from_json_value(entities, None)" — a slice the schema rejects is accepted untyped.

Sibling that DOES enforce the analogous `entities` restriction, in this same crate:
- src/ops/is_authorized_with_token.rs:299-319 — `.filter_map(|entity| entity.pointer("/uid/type").or_else(|| entity.pointer("/uid/__entity/type"))...)` then `return Err(invalid(... format!("it carries an entity of type {entity_type}, which is an entity type the identity source that issued the token generates")))`.

Test coverage: no test in tests/all supplies an `entities` item whose `entityType` is an action — the bound cases in tests/all/policy_store_authorization.rs:1774-1798 only vary the entity COUNT (`entity_at` builds `{"identifier": {"entityType": "User", ...}}`), so nothing pins the current acceptance.

## Steps to Reproduce

1. CreatePolicyStore with `validationSettings.mode = OFF`; note the returned policyStoreId.
2. CreatePolicy with `definition.static.statement = permit(principal == User::"alice", action in ActionGroup::"readers", resource == Photo::"p");`
3. Call IsAuthorized with `{"policyStoreId": <id>, "principal": {"entityType":"User","entityId":"alice"}, "action": {"actionType":"Action","actionId":"view"}, "resource": {"entityType":"Photo","entityId":"p"}, "entities": {"entityList": [{"identifier": {"entityType":"Action","entityId":"view"}, "parents": [{"entityType":"ActionGroup","entityId":"readers"}]}]}}`.
4. Expected: the request is refused, because the member documents "you can't include actions" (the shape of refusal this crate already uses for a forbidden entity type in is_authorized_with_token.rs:311-319).
5. Actual: HTTP 200. The action entity is loaded into the Cedar entity store and the caller-supplied action hierarchy decides the request.
