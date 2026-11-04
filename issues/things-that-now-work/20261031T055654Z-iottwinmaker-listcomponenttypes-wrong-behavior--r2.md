Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f0375ffd11dec5632b9c9c0b7c49494ca23050d

# The extendsFrom filter matches only DIRECT parents, justified by a claim about this crate that GetComponentType and the state module contradict

_Filed: 2026-10-31T05:56:54Z_
_Operation: ListComponentTypes | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f0375ffd11dec5632b9c9c0b7c49494ca23050d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListComponentTypes`' `extendsFrom` filter selects a component type only when the id the filter names appears literally in that type's own stored `extendsFrom` list, so a GRANDCHILD is never selected by its grandparent's id.

The handler does not present that as an arbitrary reading: its module doc states the reason for it, and the reason is a factual claim about this crate — "this service resolves no inheritance chain beyond them (`GetComponentType` reports every member `isInherited: false` for the same reason)". That claim is false in the crate as it stands. `GetComponentType`'s own module doc says the opposite in as many words ("Inheritance IS resolved"), and it resolves the whole `extendsFrom` chain through `Workspace::inherited_component_types`, marking every inherited declaration `isInherited: true`. The state module likewise defines the "extends" relation transitively for every other reader: `Workspace::component_type_instance_ids` resolves "the id itself plus every stored component type that extends it, directly or through another", and that is the scope `GetPropertyValue`, `GetPropertyValueHistory` and the component-type instance scan use.

So one crate now holds two incompatible notions of what "X extends Y" means: for inherited property definitions, for `definedIn`, for abstractness (`component_type_is_abstract` folds the whole chain) and for the instance scope of a component-type-addressed query it is TRANSITIVE, while this one filter reads it as direct-only on the strength of a sentence that describes a version of the crate that no longer exists. A caller that asks "which component types extend this base type" is answered a different set from the one every other operation of the same crate treats as extending it.

The test that pins the current filter result carries the same stale sentence as its stated reason, so that expectation moves with the fix rather than justifying it.

## Evidence

The filter's stated justification, in the handler's own module doc — `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_component_types.rs:19-26`:
```
//! * `extendsFrom` — the component types whose reported `extendsFrom` names that
//!   id. Membership is tested against the parents the create declared, which are
//!   exactly what `GetComponentType` reports, so the filter and the read agree on
//!   what one component type extends. DIRECT extension only, for that reason: a
//!   component type's stored parents are the ones its create named, and this
//!   service resolves no inheritance chain beyond them (`GetComponentType`
//!   reports every member `isInherited: false` for the same reason), so a
//!   grandchild is not selected by its grandparent's id.
```
and the code under it — `src/ops/list_component_types.rs:371-374`:
```
        Some(ListComponentTypesFilter::extendsFrom(parent)) => component_type
            .extends_from
            .iter()
            .any(|declared| declared == parent),
```

The sibling handler that sentence appeals to says the opposite — `src/ops/get_component_type.rs:17-22`:
```
//! Inheritance IS resolved: `extendsFrom` is reported as stored, and the
//! property definitions, functions, property groups and composite component
//! types reported are the ones the component type EFFECTIVELY has — its own
//! declarations folded over the ones its `extendsFrom` chain contributes
//! ([`crate::state::Workspace::inherited_component_types`]), each marked
//! `isInherited` exactly when the chain declares it.
```
and the walk it uses is transitive — `src/state.rs:1621-1633`:
```
    /// The component types one component type INHERITS from: the `extendsFrom`
    /// chain behind it, nearest first, without the component type itself.
...
    pub fn inherited_component_types(&self, component_type: &ComponentType) -> Vec<&ComponentType> {
        let (chain, _, _) = self.component_type_closure(component_type.extends_from.iter().cloned());
        chain
    }
```

The crate's own definition of the "extends" relation for every other reader — `src/state.rs:1509-1512`:
```
    /// Every component type an instance of `component_type_id` may name: the id
    /// itself plus every stored component type that extends it, directly or
    /// through another.
```

Test coverage: `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/tests/all/component_type_listing.rs:660-664` pins the current result, with the same stale sentence as its reason, so it moves with the fix:
```
            "extendsFromWithTwoChildren",
            json!({"extendsFrom": "com.example.base"}),
            // The grandchild is NOT here: this service resolves no inheritance
            // chain beyond the parents a create declared.
            ids(&["com.example.sensors.pump", "com.example.sensors.valve"]),
```
(The suite's rule header at `:583-596` repeats it: "a GRANDparent (whose grandchild is not selected, since a component type's stored parents are the ones its create declared)".)

## Steps to Reproduce

1. CreateWorkspace `w`.
2. CreateComponentType `com.example.base` in `w` with one property definition.
3. CreateComponentType `com.example.child` with `extendsFrom: ["com.example.base"]`.
4. CreateComponentType `com.example.grandchild` with `extendsFrom: ["com.example.child"]`.
5. GetComponentType `com.example.grandchild`: the base type's property definition IS reported, marked `isInherited: true` — this crate resolves the chain through `Workspace::inherited_component_types`, so the grandchild extends `com.example.base` as far as every read is concerned.
6. ListComponentTypes on `w` with `filters: [{"extendsFrom": "com.example.base"}]`: only `com.example.child` comes back; `com.example.grandchild` is absent, on the strength of a module-doc sentence (`src/ops/list_component_types.rs:22-26`) that says this crate resolves no inheritance chain — which step 5, `src/ops/get_component_type.rs:17-22` and `src/state.rs:1509-1512` each disprove.
