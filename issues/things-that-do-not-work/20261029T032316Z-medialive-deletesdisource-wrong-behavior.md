Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: c0c2239955f7987f5942ca86a100c431d5defb9f

# The SdiSource and monitor-template handlers never call `settle`, breaking the "call this at the top of every operation" rule the state module states and four sibling handlers restate

_Filed: 2026-10-29T03:23:16Z_
_Operation: DeleteSdiSource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c0c2239955f7987f5942ca86a100c431d5defb9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ComAmazonawsMedialiveMedialiveState::settle` is this crate's single mechanism for materialising clock-owed transitions, and its own documentation states the rule every handler is supposed to follow: "Call this at the top of every operation, reporting ones included". Four handlers restate that rule in their own bodies as the reason they call it first (`delete_reservation`, `update_reservation`, `purchase_offering`: "`settle` first, as every operation does"; `create_node_registration_script`, `delete_cluster`, `update_node_state`: "Clock-owed work first, as every operation of this service does").

Sixteen handlers take the state guard and never call it. They are exactly two resource families:

* the whole SdiSource family — `CreateSdiSource`, `UpdateSdiSource`, `DeleteSdiSource`, `DescribeSdiSource`, `ListSdiSources`;
* the monitor-template family's creates, updates and reads — `CreateCloudWatchAlarmTemplate(Group)`, `UpdateCloudWatchAlarmTemplate(Group)`, `GetCloudWatchAlarmTemplate(Group)`, `CreateEventBridgeRuleTemplate(Group)`, `UpdateEventBridgeRuleTemplate(Group)`, `GetEventBridgeRuleTemplate(Group)` — plus `ListVersions`.

The inconsistency is visible inside one family: `DeleteCloudWatchAlarmTemplate` and `DeleteEventBridgeRuleTemplate` DO settle at the top, while the create, the update and the get of the very same resource do not.

`DeleteSdiSource` is the sharpest case, and why this is named against it: it is a MUTATION whose documented precondition is judged against ANOTHER resource store. It refuses when `state.sdi_source_is_in_use(..)` — a derivation over `self.inputs` — yet it decides that against a store it has not brought up to date, where every other delete in this crate (`DeleteInput`, `DeleteChannel`, `DeleteMultiplex`, `DeleteNetwork`, `DeleteCluster`, `DeleteNode`, `DeleteSignalMap`, `DeleteInputSecurityGroup`, `DeleteSchedule`, `DeleteReservation`, `BatchDelete`) settles first and says in a comment that it does so precisely so "the checks below see the store a reader would".

To be honest about impact: today the two derivations these handlers read happen to be clock-insensitive — `inputs_using_sdi_source` filters on `InputRecord::is_delete_accepted`, which is `is_deleting() || is_deleted()` and therefore true both before and after `InputRecord::settle` converts `deletion_due` into `deleted`; and a template record carries no clock-owed member at all. So there is no observable divergence right now, which is why this is LOW. What is broken is the stated invariant: these sixteen handlers decide and report from a store that has not been settled, so the moment any member they read gains a clock-owed component (the node-side half of `DeleteSdiSource`'s own documented precondition is already an open issue, and nodes DO have clock-owed state via `settle_nodes`) they will silently answer from a stale store. The fix is additive and changes no response and no test: call `state.settle(now)` directly after the lock, as the other ~100 handlers of this crate do.

## Evidence

The rule, stated by the mechanism itself — services/service-com-amazonaws-medialive-medialive/src/state.rs:2443-2449:
```
    /// Materialise every transition the clock had already made due as of `now`,
    /// then answer from the result.
    ///
    /// Call this at the top of every operation, reporting ones included: it only
    /// applies transitions that were owed at that instant whether or not anybody
    /// called, so it never changes what a later caller would otherwise have seen.
    pub fn settle(&mut self, now: SystemTime) {
```

Sibling handlers restating it as the reason they call it first — src/ops/delete_reservation.rs:180-186:
```
        let mut state = state.lock();
        // `settle` first, as every operation does: it materialises only what the
        // clock had already made due, so it never changes what a later caller
        // would otherwise have seen. The reservation's own lease needs no write
        // at all — its `State` is derived from its `End` and this instant, and
        // that derived state is what this delete is judged against.
        state.settle(now);
```
and src/ops/create_node_registration_script.rs:180-184:
```
        let mut state = state.lock();
        // Clock-owed work first, as every operation of this service does, so a
        // node whose deregistration has come due is already in its terminal
        // state when a re-registration script is asked for it.
        state.settle(now);
```
and src/ops/delete_network.rs:171-176, which states what the call buys a precondition check:
```
        let mut state = state.lock();
        // `settle` materialises the transitions the clock had already made due —
        // ...
        // not this delete ran, so the checks below see the store a reader would.
        state.settle(now);
```

The handler that breaks it while judging a cross-store precondition — src/ops/delete_sdi_source.rs:160-184 (no `settle` anywhere in the file):
```
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(unavailable)?;

        let mut state = state.lock();
        if !state.sdi_sources.contains_key(&input.sdi_source_id) {
            ...
        }

        // "The SdiSource must not be part of any SdiSourceMapping and must not be
        // attached to any input": a source an input is using is a conflict, not a
        // deletion.
        if state.sdi_source_is_in_use(&input.sdi_source_id) {
```
That precondition is a derivation over the INPUT store — src/state.rs:2692-2697:
```
    /// Whether an input is attached to the SdiSource `id`, which is what
    /// `DeleteSdiSource` refuses on ("The SdiSource must ... not be attached to
    /// any input").
    pub fn sdi_source_is_in_use(&self, id: &str) -> bool {
        !self.inputs_using_sdi_source(id).is_empty()
    }
```
and the input store is one `settle` writes — src/state.rs:2466-2468 (`for input in self.inputs.values_mut() { input.settle(now); }`).

The same family, one handler settling and its siblings not — src/ops/delete_cloud_watch_alarm_template.rs:170-171:
```
        let mut state = state.lock();
        state.settle(now);
```
versus src/ops/create_cloud_watch_alarm_template.rs:226 (`let mut state = state.lock();`, no settle), src/ops/update_cloud_watch_alarm_template.rs:239, src/ops/get_cloud_watch_alarm_template.rs:157, src/ops/create_event_bridge_rule_template.rs:235, src/ops/update_event_bridge_rule_template.rs:235, src/ops/get_event_bridge_rule_template.rs:162, src/ops/create_event_bridge_rule_template_group.rs:192, src/ops/update_event_bridge_rule_template_group.rs:169, src/ops/create_cloud_watch_alarm_template_group.rs:175, src/ops/update_cloud_watch_alarm_template_group.rs:169, src/ops/get_cloud_watch_alarm_template_group.rs:156, src/ops/get_event_bridge_rule_template_group.rs:153, src/ops/list_versions.rs:162 — and the rest of the SdiSource family: src/ops/create_sdi_source.rs:178, src/ops/update_sdi_source.rs:164, src/ops/describe_sdi_source.rs:148, src/ops/list_sdi_sources.rs:156.

Why no response changes today (and why the severity is LOW) — src/state.rs:2682-2689:
```
        self.inputs
            .values()
            // An input whose deletion has been accepted has stopped using the
            // source, exactly as a deleted source has nothing using it.
            .filter(|input| !input.is_delete_accepted())
```
with src/state.rs:5534-5545 showing that predicate is clock-independent (`is_delete_accepted` = `is_deleting() || is_deleted()`, and `settle` only moves `deletion_due = None; deleted = true`).

Test coverage of the affected handlers, which a settle-first fix must leave green: tests/all/sdi_source_lifecycle.rs (create, partial update, idempotency, the `Inputs` derivation, the delete and its in-use refusal), tests/all/sdi_source_tags_and_statuses.rs, tests/all/cloud_watch_alarm_templates.rs, tests/all/cloud_watch_alarm_template_update.rs, tests/all/event_bridge_rule_templates.rs, tests/all/template_updates.rs, tests/all/engine_version_catalog.rs. None of them asserts that a handler leaves the store un-settled, so adding the call pins nothing new.

## Steps to Reproduce

1. Read src/state.rs:2443-2449 — the state module states the rule: `settle` must be called "at the top of every operation, reporting ones included", and never changes what a later caller would otherwise have seen.
2. Read src/ops/delete_reservation.rs:180-186, src/ops/create_node_registration_script.rs:180-184 and src/ops/delete_network.rs:171-176 — three handlers restate that rule in-line ("as every operation does" / "as every operation of this service does") as the reason they call it before any check.
3. grep -n 'settle' over services/service-com-amazonaws-medialive-medialive/src/ops/*.rs and compare against the handlers that take a lock: sixteen take `state.lock()` and never settle — the five SdiSource handlers, the ten monitor-template creates/updates/gets, and ListVersions.
4. Note the split inside one family: src/ops/delete_cloud_watch_alarm_template.rs:170-171 settles, while src/ops/create_cloud_watch_alarm_template.rs:226, src/ops/update_cloud_watch_alarm_template.rs:239 and src/ops/get_cloud_watch_alarm_template.rs:157 do not.
5. Note that DeleteSdiSource (src/ops/delete_sdi_source.rs:162-184) judges its documented precondition with `state.sdi_source_is_in_use` (src/state.rs:2695), a derivation over `self.inputs` — a store `settle` writes (src/state.rs:2466-2468) — without settling first, unlike every other delete in the crate.
6. Fix: add `state.settle(now)` immediately after the lock in those sixteen handlers. Confirm tests/all/sdi_source_lifecycle.rs, tests/all/cloud_watch_alarm_templates.rs, tests/all/template_updates.rs and tests/all/engine_version_catalog.rs still pass unchanged — the derivations those handlers read are clock-insensitive today (src/state.rs:5534-5545), so no response byte moves.
