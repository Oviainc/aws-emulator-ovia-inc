Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: d7f2082fceb83802a8fa1cf49a5205bc74f19057

# De-duplicate the "exactly one capacity form" rule shared by CreateConnector and UpdateConnector

_Filed: 2026-10-25T00:22:19Z_
_Operation: CreateConnector | Class: maintainability_

> _Note: this issue was identified against the code base at commit `d7f2082fceb83802a8fa1cf49a5205bc74f19057`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a capacity names exactly one of `autoScaling` / `provisionedCapacity`" is implemented TWICE, open-coded in two handlers, even though every other capacity rule of this crate is centralised in `crate::state::capacity_refusal` / `CapacityCounts::refusal`.

`crate::state::capacity_refusal` is documented as "The ONE capacity check every operation that takes a capacity runs", and `CapacityCounts::of` deliberately answers `None` for a capacity that names neither form, pushing that one half of the rule back into each handler. The result is two identical copies of the same two `is_some()` reads, the same `auto_scaled == provisioned` comparison and the same `invalid_parameter_because("capacity", ONE_CAPACITY_FORM)` rendering — one in `create_connector.rs`, one in `update_connector.rs`. The copies can drift: a change to how "exactly one form" is decided (or to which member a refusal blames) applied in one handler and not the other would make the create and the update accept different capacities, which is precisely what the `capacity_refusal` doc says cannot happen.

The de-duplication is behaviour-preserving: `capacity_from_update` maps `auto_scaling`/`provisioned_capacity` with `Option::map`, so presence of each form is identical before and after the conversion. `UpdateConnector` can therefore convert first and run ONE shared check (the one-form test folded into `capacity_refusal`, or a `one_capacity_form_refusal(&Capacity)` beside it), producing the exact same status, code and message for every input. No response and no existing test changes.

## Evidence

Copy 1 — services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/create_connector.rs:237-249:
```
        // Capacity is "whether it is auto scaled or provisioned": exactly one of
        // the two forms, which the model expresses as two optional members.
        let auto_scaled = input.capacity.auto_scaling.is_some();
        let provisioned = input.capacity.provisioned_capacity.is_some();
        if auto_scaled == provisioned {
            return Err(CreateConnectorErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::invalid_parameter_because(
                    "capacity",
                    crate::state::ONE_CAPACITY_FORM,
                )),
```

Copy 2 — services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/update_connector.rs:188-203:
```
                // Capacity is "whether it is auto scaled or provisioned":
                // exactly one of the two forms, the same rule a create's
                // capacity answers to.
                let auto_scaled = capacity.auto_scaling.is_some();
                let provisioned = capacity.provisioned_capacity.is_some();
                if auto_scaled == provisioned {
                    return Err(UpdateConnectorErrorModeled::BadRequestException(
                        BadRequestException {
                            message: Some(crate::state::invalid_parameter_because(
                                "capacity",
                                crate::state::ONE_CAPACITY_FORM,
                            )),
```

The single-place intent this contradicts — src/state.rs:269-278:
```
/// The message a refusal of `capacity` carries, or `None` when every relation
/// its members document holds.
///
/// The ONE capacity check every operation that takes a capacity runs: a create
/// on the `Capacity` its request carried, an update on the `Capacity` its
/// `CapacityUpdate` converts to ([`capacity_from_update`]), so neither can
/// accept a capacity the other refuses.
pub fn capacity_refusal(capacity: &Capacity) -> Option<String> {
    CapacityCounts::of(capacity)?.refusal()
}
```
and src/state.rs:209-212, which records that the handlers carry the other half: "or `None` when it names neither form — which every operation that takes a capacity refuses on the `capacity` member itself ([`ONE_CAPACITY_FORM`]) before reading its counts."

Presence is preserved across the conversion — src/state.rs:604-623: `auto_scaling: update.auto_scaling.map(|scaling| AutoScaling { .. })`, `provisioned_capacity: update.provisioned_capacity.map(|fixed| ProvisionedCapacity { .. })`.

Test coverage that must stay green (both sides are already pinned, and neither asserts the message text, so the shared helper keeps every assertion true):
- tests/all/custom_plugin_and_connector_lifecycle.rs:838 ("a capacity naming neither form") and :849 ("a capacity naming both forms") in `a_create_connector_request_is_refused_on_its_name_its_plugins_and_its_capacity`.
- tests/all/validation_messages.rs:340 ("CreateConnector with a capacity naming neither form").
- tests/all/connector_update.rs:848 ("a capacity naming neither of its forms") and :852 ("a capacity naming both of its forms") in `an_update_that_names_no_single_valid_setting_is_refused_with_nothing_stored`, which asserts only HTTP 400 plus an unchanged connector (lines 897-905).

## Steps to Reproduce

1. Open services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/create_connector.rs at lines 237-249 and services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/update_connector.rs at lines 188-203: the same two `is_some()` reads, the same `auto_scaled == provisioned` comparison and the same `invalid_parameter_because("capacity", crate::state::ONE_CAPACITY_FORM)` rejection appear in both.
2. Open src/state.rs:269-278 and read the `capacity_refusal` doc claiming it is "The ONE capacity check every operation that takes a capacity runs", then src/state.rs:209-212 where `CapacityCounts::of` returns `None` for a capacity naming neither form and defers that half of the rule to each handler.
3. Edit only one of the two copies (e.g. change the blamed member or the comparison in update_connector.rs) and observe that nothing in the crate detects the divergence: the create and the update then accept/refuse different capacities while `capacity_refusal`'s doc still claims neither can.
4. Fix by folding the one-form test into the shared `crate::state` capacity check (converting an update's `CapacityUpdate` with `capacity_from_update` before the check), then run `cargo test -p service-com-amazonaws-kafkaconnect-kafkaconnect`: tests/all/custom_plugin_and_connector_lifecycle.rs, tests/all/validation_messages.rs and tests/all/connector_update.rs must pass unchanged.
