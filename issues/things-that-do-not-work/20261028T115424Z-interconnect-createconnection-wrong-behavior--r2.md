Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# Tag-on-create stores an explicitly empty tag map verbatim, breaking the one-representation-for-"no tags" rule TagResource and UntagResource both maintain

_Filed: 2026-10-28T11:54:24Z_
_Operation: CreateConnection | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states — and two of its three tag writers go out of their way to enforce — that "no tags" has exactly ONE representation in the store: `None`. `TagResource` guards its merge with `if !input.tags.is_empty()` precisely so that a request carrying no tag "leaves that representation alone instead of turning it into an empty map", and `UntagResource` collapses an emptied map back to `None` once the last key goes.

`CreateConnection` skips that normalisation entirely: it copies the request's `tags` member into the record with `tags: input.tags.clone()`. A create carrying `"tags": {}` therefore persists `Some(<empty map>)`, which is a second, different representation of the same "this connection has no tags" state. Every later read of that record reports it differently from an otherwise identical connection: the `Connection$tags` member is present-and-empty for the connection created with `"tags": {}` and absent for one created without `tags` at all — and absent again for the first connection as soon as a TagResource/UntagResource round trip rewrites its tag set, because those paths DO normalise. The same logical state thus has two wire forms depending only on how it was reached, which is the exact outcome the rule quoted in `tag_resource.rs` exists to prevent.

The fix is the normalisation the siblings already perform (store `None` for an empty incoming map), applied on the one write path that omits it.

## Evidence

The rule, stated in this crate and enforced by the tagging writer — services/service-com-amazonaws-interconnect-interconnect/src/ops/tag_resource.rs:234-252:
```
        // Merge, never replace: each key the request carries is written over
        // whatever the resource held for it, and every key the request does not name
        // keeps the value it had. "No tags" has ONE representation in the store
        // (`None` — how a connection created without tags is held, and what
        // `UntagResource` leaves once the last key goes), so a request carrying no
        // tag leaves that representation alone instead of turning it into an empty
        // map.
        if !input.tags.is_empty() {
            stored
                .connection
                .tags
                .get_or_insert_with(::std::collections::HashMap::new)
```
The other sibling restoring that representation — src/ops/untag_resource.rs:219-230:
```
        // Only the listed keys go; the rest of the map is untouched. A resource left
        // with no tags holds `None`, which is how a connection created without tags
        // is stored, so "no tags" has ONE representation in the store however it was
        // reached.
        if let Some(tags) = stored.connection.tags.as_mut() {
            for key in &input.tag_keys {
                tags.remove(key);
            }
            if tags.is_empty() {
                stored.connection.tags = None;
            }
        }
```
The path that breaks it — src/ops/create_connection.rs:483 (inside the `Connection` literal, no normalisation anywhere around it):
```
            tags: input.tags.clone(),
```
and the crate's own single-rule principle for the other half of the same tag-write rule — src/state.rs:988-993 (`tag_write_violation` doc):
```
/// Both of this service's tag writers (tag-on-create and the tagging operation) judge
/// their input here, so one rule decides what a stored tag set can be however it was
/// reached
```

Test coverage: nothing pins the create-side representation. The harness never sends an empty map on create — tests/all/emulator.rs:441 `if !tags.is_empty() { request.insert("tags".to_string(), json!(tags)); }` — and the shared reader deliberately tolerates both forms, tests/all/emulator.rs:450-452: "The `tags` member of a `Connection` or of a `ListTagsForResource` response, with absent and empty treated as the same \"no tags\"". The assertions in tests/all/tag_resource.rs:285-287 and tests/all/resource_tags.rs:257-259 likewise accept `after["tags"].is_null() || after["tags"] == json!({})`, so neither pins which form a create produces.

## Steps to Reproduce

1. Boot the emulator, create a ready Direct Connect gateway, and call ListEnvironments to pick a creatable environmentId plus one of its available bandwidths.
2. Call CreateConnection with that placement, attachPoint={"directConnectGateway": <gateway>}, the Environment's required remoteAccount, and `"tags": {}` — note the returned id (call it A).
3. Call CreateConnection again with the same placement but NO `tags` member at all — note the returned id (call it B).
4. Call GetConnection(identifier=A): the response body carries `"tags": {}`. Call GetConnection(identifier=B): the response body has no `tags` member, although both connections hold no tags.
5. Call TagResource(arn=<A's arn>, tags={"k":"v"}) then UntagResource(arn=<A's arn>, tagKeys=["k"]), then GetConnection(identifier=A) again: the member is now ABSENT, because those two handlers normalise to `None`. The same "no tags" state therefore has two representations, which src/ops/tag_resource.rs:236-240 says it must not.
