Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8e0ac146ffee686dd3a46765dec581c593b08a30

# RecommendedAction$context is projected into a HashMap, so the response order the store promises is lost

_Filed: 2026-10-28T03:11:08Z_
_Operation: ListRecommendedActions | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8e0ac146ffee686dd3a46765dec581c593b08a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The store deliberately holds each record's `context` in a `BTreeMap` and states the reason: so that "the map a response carries is rendered in a stable order". The handler's projection then collects that `BTreeMap` into the generated wire type, whose `context` member is an `Option<std::collections::HashMap<String, String>>`, and the generated serializer writes a map by iterating the Rust map in place (`for (__k, __v) in #value_expr.iter()`). A fresh `HashMap` is built per request with a fresh `RandomState`, so the key order of `context` in the JSON body is arbitrary and can differ between two identical calls. That contradicts the stated rationale in `state.rs` and the handler's own claim that "two identical calls answer identically and a later caller sees exactly what an earlier one saw": the records are the same, but the bytes of each record's `context` object are not. No existing test pins the order (the one test that checks `context` compares it as a `serde_json::Map`, which is order-insensitive), so the promise in the comment is unenforced and already broken by the code under it.

## Evidence

Store side, state.rs:155-157:
```
/// `context` is a `BTreeMap` so the map a response carries is rendered in a
/// stable order, and `last_updated_timestamp` is the ISO-8601 instant the action's
/// status last changed
```
and `src/state.rs:172`: `pub context: BTreeMap<String, String>,`

Wire side, `src/ops/types/list_recommended_actions_types.rs:51`:
```
    pub context: Option<std::collections::HashMap<String, String>>,
```
Projection, `src/ops/list_recommended_actions.rs:529-535` (inside `to_wire`):
```
        context: Some(
            action
                .context
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
```
Serializer, `vendor/smithy-macros/src/codegen.rs:435-441`:
```
                #s.write_map(#member_expr, &|__sm| {
                    for (__k, __v) in #value_expr.iter() {
                        __sm.write_string(#member_expr, __k.as_ref())?;
```
(`vendor/smithy-schema-server/src/json.rs:411-427` `write_map` emits entries in exactly the order `write_entries` yields them — no sorting.)

Handler claim it breaks, `src/ops/list_recommended_actions.rs:14-16`:
```
//!   "returns a list of recommended actions"; it never writes, so two identical
//!   calls answer identically and a later caller sees exactly what an earlier one
//!   saw.
```
Test coverage: `tests/all/list_recommended_actions.rs:275-290` builds the expected `context` as a `serde_json::Map` and compares with `assert_eq!` on `Value`, which ignores key order — so nothing in the crate pins the ordering the comment promises.

## Steps to Reproduce

1. POST `/` with `X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions` and body `{}` against the emulator (the boot inventory already seeds a `context` entry; seed a record with several `context` keys through `RecommendedActionsStore::seed` to make the order visible).
2. Repeat the identical request several times (and across emulator restarts) and compare the raw response bytes rather than the parsed JSON.
3. Observe that the `context` object's key order varies between responses, even though `StoredAction::context` is a `BTreeMap` whose doc says the order a response carries is stable, and the handler doc says two identical calls answer identically.
