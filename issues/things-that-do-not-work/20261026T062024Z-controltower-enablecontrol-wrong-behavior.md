Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: adb8f986b66b684ed38690a8bd355d4f46d0ebf5

# EnableControl's one-enablement-per-target rule compares raw controlIdentifier strings, so the same control can be enabled twice on one target

_Filed: 2026-10-26T06:20:24Z_
_Operation: EnableControl | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `adb8f986b66b684ed38690a8bd355d4f46d0ebf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states in two places that a control is enabled at most once per control/target pair, and it resolves a `controlIdentifier` to its catalog entry in a way that deliberately IGNORES everything before the resource segment and accepts TWO different ARN forms for the same control (the Control Tower `control/<NAME>` form and the Control Catalog `control/<catalog_id>` form). But the duplicate check `enabled_control_on_target` compares the caller's raw `controlIdentifier` string with `==`, and `EnableControl` stores that raw string.

Consequence: two `EnableControl` calls naming the SAME catalog control on the SAME target with two identifiers that `resolve_control` maps to one entry — e.g. `arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES` and `arn:aws:controlcatalog:::control/7dcf0fqkdmxt34bq8sm0lyvke5`, or the same name under two region segments (`us-west-2` vs `us-east-1`) — both succeed and leave TWO `EnabledControl` resources describing one control on one target. That is exactly the state the handler's own comment says must not exist, and both the per-target quota accounting (`MAX_ENABLED_CONTROLS`) and the control operation log then carry two independent lifecycles for one deployed control.

The same raw-string comparison also makes `ListEnabledControls`'s `filter.controlIdentifiers` and `ListControlOperations`'s `filter.controlIdentifiers`/`filter.targetIdentifiers` miss an enablement that was created with the other equivalent form, because the stored identifier is never normalised to the catalog entry it resolved to.

The fix is to key the uniqueness check (and ideally the stored identifier) on the resolved catalog entry — which the handler already has in hand as `control` — rather than on the request string.

## Evidence

The crate says the segments and the ARN form carry no identity:

- `services/service-com-amazonaws-controltower-awscontroltowerapis/src/state.rs:2319-2325` (doc of `resolve_control`): "A control is named by either of the two ARN forms AWS publishes for it — the AWS Control Tower form, whose resource segment is the control's NAME, and the AWS Control Catalog form, whose resource segment is the control's opaque id — so both resolve to the same entry here. The same catalog is published in every partition, region and account, so the segments before the resource select nothing and are not compared beyond being the ones a control ARN carries"
- `src/state.rs:2326-2333`: `resolve_control` matches `ControlReference::Name(name) => ... control.name == name` OR `ControlReference::CatalogId(id) => ... control.catalog_id == id` — one catalog entry, two identifier forms.

The crate says the pair must be unique:

- `src/state.rs:1167-1169` (doc of `enabled_control_on_target`): "A control is enabled at most once per target, so this is what `EnableControl` reads to refuse a repeat."
- `src/ops/enable_control.rs:279-282`: "// A control is enabled at most once per target. A repeat while the first // enablement is still deploying would race it, which is the inconsistent state // `ConflictException` reports"

But the check is raw string equality, and the raw string is what gets stored:

- `src/state.rs:1177-1180`: `.filter(|control| { control.control_identifier == control_identifier && control.target_identifier == target_identifier })`
- `src/ops/enable_control.rs:283-284`: `if let Some(existing) = store.enabled_control_on_target(&input.control_identifier, &input.target_identifier)`
- `src/ops/enable_control.rs:344`: `control_identifier: input.control_identifier,` — the request string, not `control.name`/`control.catalog_id`.

Test coverage does not reach it: `tests/all/enabled_control_lifecycle.rs:780-840` (`a_control_is_enabled_at_most_once_per_target`) repeats the request with the SAME string produced by the single helper `tests/all/enabled_control_lifecycle.rs:45-47` — `format!("arn:aws:controltower:us-west-2::control/{name}")` — so a repeat through the Control Catalog form or another region segment is asserted nowhere. `tests/all/enabled_control_reset.rs:55-57` shows the catalog form is a form this emulator accepts: `format!("arn:aws:controlcatalog:::control/{catalog_id}")`.

## Steps to Reproduce

1. POST /enable-control with {"controlIdentifier":"arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES","targetIdentifier":"arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-8888"} — 200, note the returned enabled control ARN.
2. Wait past the settle span (OPERATION_SETTLE_AFTER, 500ms) so the first enablement is SUCCEEDED.
3. POST /enable-control again with {"controlIdentifier":"arn:aws:controlcatalog:::control/7dcf0fqkdmxt34bq8sm0lyvke5","targetIdentifier":"arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-8888"} — the Control Catalog ARN of the SAME catalog entry (state.rs:2216-2219).
4. Expected per the handler's own rule: a refusal (the ValidationException/ConflictException the repeat path returns). Actual: 200 with a SECOND, different enabled control ARN.
5. POST /list-enabled-controls with {"targetIdentifier":"arn:aws:organizations::000000000000:ou/o-emulatororg/ou-abcd-8888"} — two entries are returned for one control on one target.
6. Repeat with {"controlIdentifier":"arn:aws:controltower:us-east-1::control/AWS-GR_ENCRYPTED_VOLUMES"} (same name, different region segment) for a third duplicate.
7. POST /list-enabled-controls with {"filter":{"controlIdentifiers":["arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES"]}} — the duplicates created through the other forms are not selected.
