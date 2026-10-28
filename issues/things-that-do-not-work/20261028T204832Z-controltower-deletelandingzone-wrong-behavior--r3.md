Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: fe93971ca235a36b2f32da1951aa5b5c55651609

# Decommissioning a landing zone leaves every EnabledControl and EnabledBaseline behind, still reported as deployed

_Filed: 2026-10-28T20:48:32Z_
_Operation: DeleteLandingZone | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `fe93971ca235a36b2f32da1951aa5b5c55651609`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteLandingZone` is modelled as the operation that decommissions the landing zone by deleting the Control Tower resources deployed in the managed accounts, and `EnableControl` is modelled as the operation that CREATED those resources on the target organizational unit and its accounts. The emulator's decommissioning removes exactly one thing: the landing zone record and its tag entry. Every `EnabledControl` and `EnabledBaseline` the account holds survives the decommissioning untouched, and `ListEnabledControls`/`GetEnabledControl`/`ListEnabledBaselines`/`GetEnabledBaseline` keep reporting them as `SUCCEEDED`, `IN_SYNC` resources of a landing zone that no longer exists.

The cascade machinery is already in the same function: `settle` removes an `EnabledControl` (and its tags) when its own teardown becomes due, and the same for an `EnabledBaseline`. Only the landing zone's own removal cascades to nothing beneath it, even though it is the resource the crate itself calls "Control Tower's primary resource" and the one whose governance an enabled control's reported `targetRegions` is derived from.

The survival is not inert: with the landing zone gone, `governed_regions()` is empty and `target_regions_of` falls back to the region the ENABLING request was signed for, so after a successful decommissioning `GetEnabledControl` reports the control as expected-to-be-deployed in a region no landing zone governs — a value built from a landing zone that was decommissioned rather than from governance.

## Evidence

The modelled contract for this operation, quoted in its own handler header, `src/ops/delete_landing_zone.rs:55-59` (identical to `smithy_query(query="doc", shape="com.amazonaws.controltower#DeleteLandingZone")`):
```
//! Decommissions a landing zone. This API call starts an asynchronous operation that
//! deletes Amazon Web Services Control Tower resources deployed in accounts managed by
//! Amazon Web Services Control Tower. Decommissioning a landing zone is a process with
//! significant consequences, and it cannot be undone. We strongly recommend that you
//! perform this decommissioning process only if you intend to stop using your landing zone.
```

What the resources that survive it ARE, from `EnableControl`'s modelled contract, `src/ops/enable_control.rs:61-64`:
```
//! This API call activates a control. It starts an asynchronous operation that creates
//! Amazon Web Services resources on the specified organizational unit and the accounts it
//! contains. The resources created will vary according to the control that you specify. For
//! usage examples, see the Controls Reference Guide .
```

The decommissioning as implemented — landing zone and its tags only, `src/state.rs:1189-1205`:
```
        let decommissioned: Vec<String> = self
            .landing_zones
            .iter()
            .filter(|(_, landing_zone)| {
                landing_zone
                    .pending_deletion
                    .as_ref()
                    .is_some_and(|pending| now >= pending.due_at)
            })
            .map(|(arn, _)| arn.clone())
            .collect();
        for arn in decommissioned {
            self.landing_zones.remove(&arn);
            // A tag entry never outlives the resource it tags, which is what keeps the tag
            // map bounded by the resource stores.
            self.tags.remove(&arn);
        }
```
Nothing in `settle` (or in `record_landing_zone_deletion`, `src/state.rs:1587-1601`) touches `enabled_controls` or `enabled_baselines`.

The cascade the crate already performs for the resource one level down, `src/state.rs:1272-1277`:
```
        for arn in torn_down {
            self.enabled_controls.remove(&arn);
            // A tag entry never outlives the resource it tags, which is what keeps the tag
            // map bounded by the resource stores.
            self.tags.remove(&arn);
        }
```
and for an enabled baseline, `src/state.rs:1372-1377`.

What a surviving control then reports, `src/state.rs:1531-1538`:
```
    pub fn target_regions_of(&self, control: &EnabledControl) -> Vec<String> {
        let governed = self.governed_regions();
        if governed.is_empty() {
            vec![control.enablement_region.clone()]
        } else {
            governed
        }
    }
```
against its own documented rule, `src/state.rs:1518-1522`: "A control is deployed into the regions the landing zone governs (`Region`: \"the expected Regions are based on the Regions that are governed by the landing zone\")".

Test coverage: `tests/all/landing_zone_decommission.rs` asserts only that the landing zone itself disappears (`a_landing_zone_stays_a_resource_until_its_delete_is_due`, line 139, and the 404 at line 259) and `tests/all/delete_landing_zone_idempotence.rs` only the repeated call; neither file, nor `tests/all/enabled_control_lifecycle.rs`, `tests/all/enabled_control_target_regions.rs` or `tests/all/enable_baseline_persistence.rs`, mentions an enabled control or enabled baseline after a decommissioning — so nothing pins the surviving enablements either way.

## Steps to Reproduce

1. POST /create-landingzone with `{"version":"3.3","manifest":{... "governedRegions":["us-west-2"] ...}}` -> 200, note `arn`.
2. POST /enable-control with an enablable catalog control and an organizational-unit `targetIdentifier` -> 200, note `arn`; wait past the settle span.
3. POST /get-enabled-control with that ARN -> 200: `statusSummary.status` is `SUCCEEDED` and `targetRegions` is `[{"name":"us-west-2"}]` (the governed set).
4. POST /delete-landingzone with the landing zone ARN -> 200; wait past the settle span. POST /get-landingzone -> 404: the landing zone is decommissioned.
5. POST /list-enabled-controls (and /get-enabled-control, /list-enabled-baselines) -> 200: the enablement is still listed as `SUCCEEDED`/`IN_SYNC`, and `targetRegions` has silently become the region the enabling request was signed for, because `governed_regions()` is now empty (`src/state.rs:1531-1538`).
6. Read `src/state.rs:1189-1205`: the decommissioning removes only `landing_zones` and the landing zone's `tags` entry, while the sibling teardowns at `src/state.rs:1272-1277` and `src/state.rs:1372-1377` do remove the resource they tear down and its tags.
