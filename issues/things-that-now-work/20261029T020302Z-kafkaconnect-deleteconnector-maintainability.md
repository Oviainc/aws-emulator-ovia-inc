Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 5072f4ec107ec0dab088411d8137f3fc4cd946a3

# De-duplicate the "first delete starts the clock, a repeat does not postpone it" transition copied into all three delete handlers

_Filed: 2026-10-29T02:03:02Z_
_Operation: DeleteConnector | Class: maintainability_

> _Note: this issue was identified against the code base at commit `5072f4ec107ec0dab088411d8137f3fc4cd946a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three delete handlers implement one rule — the idempotent start of a removal: if the resource is not already in its DELETING state, move it there and owe the clock a `PendingKind::Remove` one `TRANSITION_SPAN` out; if it already is, change nothing so a retry never postpones the removal — as three separate copies of the same branch-and-two-assignments, differing only in the record and the `*_DELETING` constant. The rule itself is written down once, as a comment over the third copy only; the other two carry the code with no statement of the rule they implement, which is exactly how such copies drift (one copy gaining a state-dependent refinement, or one copy resetting the due instant on a repeat, would not be visible from the others).

This crate has already settled on the opposite pattern for every other rule two or more handlers share: `crate::state::capacity_refusal`, `crate::state::ComAmazonawsKafkaconnectKafkaconnectState::start_connector_operation` and `ConnectorRecord::not_running_refusal` each live in `state.rs` as the ONE seam every handler that needs them runs, with only the modelled error variant left per handler. The removal-start transition is the remaining shared rule that is not such a seam; folding it into one place in `state.rs` (the records already share the `pending: Option<PendingChange>` field and the `RegionalResource` trait) changes no response body and no status: each handler keeps reading `record.state` for its own output member after the call.

## Evidence

Copy 1 — services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/delete_connector.rs:212-217 (no statement of the rule):
```
        if connector.state != crate::state::CONNECTOR_DELETING {
            connector.state = crate::state::CONNECTOR_DELETING.to_string();
            connector.pending = Some(crate::state::PendingChange::in_a_moment(
                crate::state::PendingKind::Remove,
            ));
        }
```

Copy 2 — src/ops/delete_custom_plugin.rs:199-204 (no statement of the rule):
```
        if plugin.state != crate::state::CUSTOM_PLUGIN_DELETING {
            plugin.state = crate::state::CUSTOM_PLUGIN_DELETING.to_string();
            plugin.pending = Some(crate::state::PendingChange::in_a_moment(
                crate::state::PendingKind::Remove,
            ));
        }
```

Copy 3 — src/ops/delete_worker_configuration.rs:209-217, the only copy that states the rule all three implement:
```
        // The first delete starts the clock; a repeat while the configuration is
        // still DELETING answers the same state and leaves the due instant where
        // it was, so retrying the call never postpones the removal.
        if worker.state != crate::state::WORKER_CONFIGURATION_DELETING {
            worker.state = crate::state::WORKER_CONFIGURATION_DELETING.to_string();
            worker.pending = Some(crate::state::PendingChange::in_a_moment(
                crate::state::PendingKind::Remove,
            ));
        }
```

The crate's own stated pattern for a rule more than one handler runs — src/state.rs:784-794 (`ConnectorRecord::not_running_refusal`): "The ONE state gate every operation that starts work on a connector runs (a restart and an update), so neither can differ from the other on which `ConnectorState` admits such a call, on the member the refusal blames, or on how it is worded ... Only the modelled error VARIANT stays with the handler, since the error enum is per-operation — the same shape as [`capacity_refusal`] and [`ComAmazonawsKafkaconnectKafkaconnectState::start_connector_operation`], the other two rules these handlers share." The same three-handler doc rule is restated in each delete handler's header, e.g. src/ops/delete_custom_plugin.rs:8-9: "Idempotent: a repeat while the plugin is already `DELETING` answers the same state without moving the due instant."

Existing tests that pin the behaviour a de-duplication must leave unchanged:
- tests/all/custom_plugin_and_connector_lifecycle.rs:1182-1218 — "The real version is accepted, and the delete is idempotent." asserting `connectorState == "DELETING"` on each attempt, and :1255-1256 for the custom plugin ("a deleted plugin is DELETING").
- tests/all/worker_configuration_delete_and_tags.rs:239-317 — case "wc-delete-repeated", ":239 operation is `@idempotent`, so a repeat must answer the same thing", asserting `workerConfigurationState == "DELETING"` per attempt.
- tests/all/list_worker_configurations.rs:545-546 — a configuration whose delete is under way is listed DELETING.
- tests/all/connector_capacity_and_worker_configuration.rs:542-575 — a connector create against a DELETING worker configuration is a 400, which depends on the state this transition writes.

## Steps to Reproduce

1. Open src/ops/delete_connector.rs:212-217, src/ops/delete_custom_plugin.rs:199-204 and src/ops/delete_worker_configuration.rs:209-217 side by side: the same branch, the same two assignments and the same `PendingKind::Remove` deadline appear three times, with only the record and the `*_DELETING` constant differing.
2. Note that the rule those three copies implement is stated only once, in the comment at delete_worker_configuration.rs:209-211; the other two copies carry no statement of it.
3. Compare with the three shared rules this crate already keeps as one seam in src/state.rs (`capacity_refusal`, `start_connector_operation`, `ConnectorRecord::not_running_refusal`, cross-referenced at state.rs:784-794) — each run by every handler that needs it, with only the per-operation error variant left in the handler.
4. Confirm a de-duplication is behaviour-preserving: each handler reads `record.state`/`record.arn` for its own output member after the transition, so the three response shapes and the tests listed in the evidence stay as they are.
