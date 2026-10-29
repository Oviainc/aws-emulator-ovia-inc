Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f10a9c1d4031e9ac500bbdeee80ff5cb5da13a14

# An entityList carrying two entities with the same identifier is refused 400 instead of processing the last one

_Filed: 2026-10-29T00:05:40Z_
_Operation: IsAuthorized | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f10a9c1d4031e9ac500bbdeee80ff5cb5da13a14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`EntitiesDefinition$entityList` documents what happens when a caller repeats an entity: "If you include multiple entities with the same `identifier`, only the last one is processed in the request." That is a documented SUCCESS path — the slice is accepted and the later entity wins.

The crate never applies that rule. `crate::state::cedar::entities_to_cedar_json` walks the `entityList` and pushes every item into the Cedar JSON array verbatim, with no de-duplication on `identifier`. The array then goes to `build_entities`, which hands it to `cedar_policy::Entities::from_json_value`; the crate's own doc comment on that function states that a duplicate uid is exactly what is refused there ("Only a payload that is no Cedar entity store at all (a duplicate uid, an unparseable entity type) is refused as a bad request"), and the refusal is turned into the modelled `ValidationException` naming `/entities`.

So `IsAuthorized` answers HTTP 400 `ValidationException` ("the entities are not a valid Cedar entity store: …") for a request the model says must be evaluated with the LAST of the duplicated entities. A caller that assembles its entity slice programmatically — concatenating a principal's groups and a resource's parents, for instance — and happens to emit one entity twice gets no authorization decision at all instead of the documented one.

The same helper pair is reached by `IsAuthorizedWithToken` (is_authorized_with_token.rs:243) and `BatchIsAuthorized` (batch_is_authorized.rs:244,256), so all three authorization calls behave the same way; `IsAuthorized` is the single-request handler that reaches it first. The `cedarJson` arm is affected identically, since `parse_cedar_json` only bounds size and depth before the same `build_entities` call.

## Evidence

The model sentence — `smithy_query(query="shape", shape="EntitiesDefinition")`, member `entityList`:
```
<p>An array of entities that are needed to successfully evaluate an authorization request. Each entity in this array must include an identifier for the entity, the attributes of the entity, and a list of any parent entities.</p> <note> <p>If you include multiple entities with the same <code>identifier</code>, only the last one is processed in the request.</p> </note>
```

The conversion that keeps every duplicate — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state/cedar.rs:1332-1341:
```
        Some(EntitiesDefinition::entityList(items)) => {
            let mut out = Vec::with_capacity(items.len().min(MAX_ENTITIES_PER_REQUEST + 1));
            for item in items.iter().take(MAX_ENTITIES_PER_REQUEST + 1) {
                out.push(entity_item_to_cedar_json(item)?);
            }
            if items.len() > MAX_ENTITIES_PER_REQUEST {
                return Err(too_many(items.len()));
            }
            serde_json::Value::Array(out)
        }
```
(no key is computed from `item.identifier`; `entity_item_to_cedar_json` at cedar.rs:1354-1362 just writes `uid` from `item.identifier` and returns.)

The crate's own statement that a duplicate uid is REFUSED — src/state/cedar.rs:914-918:
```
/// values an authorization request carries. So a slice the schema cannot type is
/// still parsed, untyped, and evaluated — whatever the evaluation then raises
/// about it is reported in-band in the response's `errors`. Only a payload that
/// is no Cedar entity store at all (a duplicate uid, an unparseable entity type)
/// is refused as a bad request.
```
and the refusal it builds — src/state/cedar.rs:930-935:
```
    cedar_policy::Entities::from_json_value(entities, None).map_err(|e| {
        InvalidInput::new(
            paths::ENTITIES,
            format!("the entities are not a valid Cedar entity store: {e}"),
        )
    })
```

How that reaches the caller as a 400 — src/ops/is_authorized.rs:188-189 and 202-218:
```
        let entities = crate::state::cedar::entities_to_cedar_json(input.entities.as_ref())
            .map_err(invalid)?;
```
```
        let verdict = crate::state::cedar::authorize(
            store,
            &principal,
            &action,
            &resource,
            context,
            entities,
        )
        .map_err(|refusal| match refusal {
            …
            crate::state::cedar::Refusal::InvalidInput(field) => invalid(field),
```
with `invalid` building the modelled `ValidationException` — src/ops/is_authorized.rs:262-264:
```
fn invalid(invalid: crate::ops::InvalidInput) -> IsAuthorizedError {
    IsAuthorizedErrorModeled::ValidationException(crate::ops::validation_exception([invalid])).into()
}
```
(`authorize` calls `build_entities` at src/state/cedar.rs:1069.)

Test coverage: nothing pins this. `grep -n 'duplicate|same identifier|last one' tests/` returns only unrelated hits (a duplicate POLICY name in tests/all/policy_templates.rs:294 and tests/all/error_messages.rs:836, and "that page is the last one" in the pagination suites). The entity-slice suites (tests/all/policy_store_authorization.rs:1606, tests/all/batch_is_authorized.rs:369) each send a single-entity slice.

## Steps to Reproduce

1. CreatePolicyStore { validationSettings: { mode: "OFF" } } -> note policyStoreId PS.
2. CreatePolicy { policyStoreId: PS, definition: { static: { statement: "permit(principal, action, resource) when { principal.level == \"admin\" };" } } }.
3. IsAuthorized { policyStoreId: PS, principal: { entityType: "User", entityId: "alice" }, action: { actionType: "Action", actionId: "view" }, resource: { entityType: "Photo", entityId: "p" }, entities: { entityList: [ { identifier: { entityType: "User", entityId: "alice" }, attributes: { level: { string: "guest" } } }, { identifier: { entityType: "User", entityId: "alice" }, attributes: { level: { string: "admin" } } } ] } }.
4. Observed: HTTP 400 ValidationException with fieldList[0].path = "/entities" and message "the entities are not a valid Cedar entity store: …" — no decision at all.
5. Expected per `EntitiesDefinition$entityList` ("If you include multiple entities with the same identifier, only the last one is processed in the request"): HTTP 200 with the decision computed from the SECOND `User::"alice"` entity (level = "admin"), i.e. ALLOW.
6. The same request against IsAuthorizedWithToken and against BatchIsAuthorized is refused the same way (both reach src/state/cedar.rs:919 through the same helper).
