Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a399f03ccc4f5afa40c0af0ce737308e8fb96c4f

# A refusal about an entityList item's attributes, parents or tags names a path with no element position, so fieldList points at a member the request has not got

_Filed: 2026-10-29T16:54:51Z_
_Operation: IsAuthorized | Class: error-response_

> _Note: this issue was identified against the code base at commit `a399f03ccc4f5afa40c0af0ce737308e8fb96c4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate addresses every refused request member with a JSON pointer in `ValidationException$fieldList`, and it states — and its own suite pins — that a member inside a collection is addressed with that element's position (`/requests/1/principal/entityId`). The checks `entity_item_to_cedar_json` runs over one `entities.entityList` ITEM break that rule: they report at the fixed constants `/entities/entityList/attributes`, `/entities/entityList/parents` and `/entities/entityList/tags`, which carry no element position at all. `entityList` is a LIST on the wire, so `/entities/entityList/attributes` addresses nothing: a request carrying twenty entities is told only that "an attributes map" was too long or nested too deep, with no way to learn WHICH entity was at fault, and the pointer a client resolves against its own request body does not resolve.

The index is available at the call site and simply not used: `entities_to_cedar_json` walks `items.iter().take(..)` without `enumerate`, while the crate already owns the exact mechanism for this — `InvalidInput::within_batch_request(index)`, which exists so that a check written for one request can say which element of `/requests` it refused. Nothing equivalent is applied to `entityList`.

The result is that the SAME member is addressed two different ways in one operation: a boundary-constraint violation inside the second `entityList` item is reported by the service's own `json_pointer` with the element's 0-based position, while this handler-side check on the same item reports a positionless path.

This is a different defect from the already-FIXED `IsAuthorized`/error-response issue (20261029T005600Z), which was about payload refusals outranking store resolution; this one is about the pointer a payload refusal carries once it is correctly ordered. The shared helper is reached from `IsAuthorized`, `BatchIsAuthorized` and `IsAuthorizedWithToken`; `IsAuthorized` is named here as the first operation whose handler calls it.

## Evidence

The rule, from the model — smithy_query(query="doc", shape="ValidationExceptionField", member="path"): "The path to the specific element that Verified Permissions found to be not valid."

The rule as this crate states it — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/mod.rs:61-69:
```
    /// This member, re-rooted under one request of a batch.
    ///
    /// A check that evaluates ONE authorization request names the member as that
    /// request carries it (`/principal`); inside a batch the same member lives at
    /// `/requests/1/principal`, so the check keeps one implementation and the
    /// batch says which of its requests it was.
    pub fn within_batch_request(self, index: usize) -> Self {
        self.within(&format!("{}/{index}", paths::REQUESTS))
    }
```

The same rule pinned over the wire — tests/all/validation_exception_contents.rs:167-193:
```
        (
            // A member inside the SECOND element of a list: the pointer names
            // the element's own 0-based position, with no `member` step.
            "a length minimum broken inside the second element of a list",
            "BatchIsAuthorized",
...
                clause(
                    "/requests/1/principal/entityId",
                    "Member must have length greater than or equal to 1",
                )
```

The path that breaks it — src/state/cedar.rs:1411-1451 (`entity_item_to_cedar_json`):
```
fn entity_item_to_cedar_json(item: &EntityItem) -> Result<serde_json::Value, InvalidInput> {
...
    let mut attrs = serde_json::Map::new();
    if let Some(given) = item.attributes.as_ref() {
        let path = paths::ENTITY_LIST_ATTRIBUTES;
        check_len(given.len(), path)?;
        for (key, value) in given {
            attrs.insert(key.clone(), attribute_value_to_cedar_json(value, 1, path)?);
        }
    }
...
    if let Some(given) = item.parents.as_ref() {
        check_len(given.len(), paths::ENTITY_LIST_PARENTS)?;
...
    if let Some(given) = item.tags.as_ref() {
        let path = paths::ENTITY_LIST_TAGS;
        check_len(given.len(), path)?;
```
with the positionless constants at src/ops/mod.rs:200-205:
```
    /// An `entityList` item's attribute map.
    pub const ENTITY_LIST_ATTRIBUTES: &str = "/entities/entityList/attributes";
    /// An `entityList` item's parent list.
    pub const ENTITY_LIST_PARENTS: &str = "/entities/entityList/parents";
    /// An `entityList` item's tag map.
    pub const ENTITY_LIST_TAGS: &str = "/entities/entityList/tags";
```

The index is at hand and dropped — src/state/cedar.rs:1389-1398:
```
        Some(EntitiesDefinition::entityList(items)) => {
            let mut out = Vec::with_capacity(items.len().min(MAX_ENTITIES_PER_REQUEST + 1));
            for item in items.iter().take(MAX_ENTITIES_PER_REQUEST + 1) {
                out.push(entity_item_to_cedar_json(item)?);
            }
```

The refusals that reach those paths — src/state/cedar.rs:1174-1193:
```
fn check_depth(depth: usize, path: &str) -> Result<(), InvalidInput> {
    if depth > MAX_VALUE_DEPTH {
...
fn check_len(len: usize, path: &str) -> Result<(), InvalidInput> {
    if len > MAX_COLLECTION_LEN {
        return Err(InvalidInput::new(
            path,
            format!("it carries {len} elements, more than the {MAX_COLLECTION_LEN} allowed"),
```
(`MAX_COLLECTION_LEN = 1_000`, `MAX_VALUE_DEPTH = 32`, src/state/cedar.rs:99-102.)

The service's own pointer convention for a list element — src/service.rs:63-68:
```
/// Each dotted segment becomes one pointer segment. A step from a collection into
/// one of its elements arrives as `<position-or-key>.member`, which a pointer
/// spells as the position alone: an element's position is renumbered from the
/// framework's 1-based count to the 0-based index RFC 6901 gives an array
/// element,
```

Test coverage: no test in tests/all asserts any `/entities/entityList/...` path (grep for `entityList/` in tests/ returns no matches), so nothing pins or justifies the positionless form.

## Steps to Reproduce

1. Boot the emulator and point a Verified Permissions client at its data listener.
2. store = CreatePolicyStore(validationSettings={"mode":"OFF"}).
3. Call IsAuthorized with policyStoreId=store, principal={"entityType":"PhotoApp::User","entityId":"alice"}, action={"actionType":"PhotoApp::Action","actionId":"viewPhoto"}, resource={"entityType":"PhotoApp::Photo","entityId":"vacation"}, and entities={"entityList":[{"identifier":{"entityType":"PhotoApp::Photo","entityId":"vacation"}},{"identifier":{"entityType":"PhotoApp::Photo","entityId":"holiday"},"attributes":{"owners":{"set":[ ...1001 {"string":"x"} entries... ]}}}]}.
4. Observed: HTTP 400 ValidationException whose fieldList entry has path "/entities/entityList/attributes" — a member the request body has not got, and which does not say that the SECOND entity was at fault.
5. Expected per the rule this crate states (ops/mod.rs:61-69) and pins (tests/all/validation_exception_contents.rs:167-193): the path names the offending element's 0-based position, e.g. "/entities/entityList/1/attributes" — the spelling a boundary-constraint violation on the same item already gets from src/service.rs's json_pointer.
6. Contrast: send the same over-long attribute set as the second request of a BatchIsAuthorized and the refusal does name the request index ("/requests/1/..."), so which spelling a client sees depends only on which collection the member sits in.
