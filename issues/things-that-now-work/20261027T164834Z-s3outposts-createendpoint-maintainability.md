Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 03bb735b5164c6b554d75cb5873a72ff1e3f303a

# Factor the triplicated "resolve the Outpost in this Region or answer the modelled 404" rule into state, as list_cursor already is

_Filed: 2026-10-27T16:48:34Z_
_Operation: CreateEndpoint | Class: maintainability_

> _Note: this issue was identified against the code base at commit `03bb735b5164c6b554d75cb5873a72ff1e3f303a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "an `OutpostId` that names no Outpost of the inventory installed in the Region the request was signed for is the modelled `ResourceNotFoundException` carrying `state::not_found()`" is implemented three separate times, once per handler that takes an `OutpostId`: `CreateEndpoint`, `DeleteEndpoint` and `ListSharedEndpoints`. Each copy re-writes the same branch (call `state::outpost_in`, test for absence, lift `state::not_found()` into that operation's own error enum), so the three can drift: a change to how the Outpost is resolved (for instance also admitting an Outpost in a sibling Region, or carrying a different payload) has to be made in three places and nothing fails if one is missed.

This crate already has its own established shape for exactly this situation and uses it for the other rule shared by several operations: `state::list_cursor` owns the cursor refusal centrally and hands back the bare modelled payload for each caller to lift into its own error enum. The same treatment applies here — a `state`-level resolver returning `Result<&OutpostRecord, ResourceNotFoundException>` — leaving the one resolution rule in one place, every response byte-identical and every existing test unchanged (`CreateEndpoint`'s copy additionally needs the resolved record's `region`, which such a resolver returns).

## Evidence

Copy 1 — services/service-com-amazonaws-s3outposts-s3outposts/src/ops/create_endpoint.rs:204-211:
```
        let Some(home_region) = crate::state::outpost_in(&inventory, &region, &input.outpost_id)
            .map(|outpost| outpost.region.clone())
        else {
            return Err(CreateEndpointErrorModeled::ResourceNotFoundException(
                crate::state::not_found(),
            )
            .into());
        };
```

Copy 2 — services/service-com-amazonaws-s3outposts-s3outposts/src/ops/delete_endpoint.rs:191-196:
```
        if crate::state::outpost_in(&inventory, &region, &input.outpost_id).is_none() {
            return Err(DeleteEndpointErrorModeled::ResourceNotFoundException(
                crate::state::not_found(),
            )
            .into());
        }
```

Copy 3 — services/service-com-amazonaws-s3outposts-s3outposts/src/ops/list_shared_endpoints.rs:171-176:
```
        if crate::state::outpost_in(&inventory, &region, &input.outpost_id).is_none() {
            return Err(ListSharedEndpointsErrorModeled::ResourceNotFoundException(
                crate::state::not_found(),
            )
            .into());
        }
```

The crate's own precedent for centralising a rule shared by several handlers — services/service-com-amazonaws-s3outposts-s3outposts/src/state.rs:977-996:
```
/// `operation` is the listing's own name and `decode` its cursor's own form;
/// those are all that differ between the listings, so the refusal itself lives
/// here. The caller lifts the payload into its own error enum.
pub fn list_cursor<Key>(
    next_token: Option<&str>,
    operation: &str,
    decode: impl FnOnce(&str) -> Option<Key>,
) -> Result<Option<Key>, ValidationException> {
```

That the three copies are one rule is stated by the crate's own test — services/service-com-amazonaws-s3outposts-s3outposts/tests/all/control_plane_inventory.rs:654-671:
```
        // Every door into this service's endpoint lifecycle resolves the Outpost first,
        // so each one answers the service's single not-found text.
        let refusals = vec![
            ("CreateEndpoint", emulator.create_endpoint_in(REGION, &outpost_id, 1).await),
            ("ListSharedEndpoints", emulator.shared_endpoints_in(REGION, &outpost_id).await),
            ("DeleteEndpoint", emulator.delete_endpoint_in(REGION, &outpost_id, "0000000000000000001").await),
        ];
```

Existing coverage that must stay green and unchanged after the refactor: `tests/all/control_plane_inventory.rs::an_outpost_without_ordered_s3_capacity_is_reported_and_resolved_nowhere` (drives all three doors and asserts the same 404 + fixed text), `tests/all/not_found_messages.rs`, `tests/all/region_scoping.rs` (the out-of-Region 404 through each door), `tests/all/unprovisioned_account.rs` and `tests/all/endpoint_teardown.rs:220-236`.

## Steps to Reproduce

1. Open src/ops/create_endpoint.rs and read lines 204-211: `outpost_in(&inventory, &region, &input.outpost_id)` is absent -> `ResourceNotFoundException(state::not_found())`.
2. Open src/ops/delete_endpoint.rs lines 191-196 and src/ops/list_shared_endpoints.rs lines 171-176: the same branch, verbatim apart from the error-enum name.
3. Compare with src/state.rs:977-996, where the other rule shared by several handlers (the cursor refusal) is centralised in `list_cursor` and the handlers only lift its payload.
4. Observe there is no `state`-level resolver for the Outpost rule, so the three copies must be kept in step by hand.
