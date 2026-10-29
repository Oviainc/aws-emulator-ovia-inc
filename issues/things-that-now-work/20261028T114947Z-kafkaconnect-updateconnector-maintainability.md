Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# De-duplicate the "connector is not RUNNING" refusal shared by UpdateConnector and RestartConnector

_Filed: 2026-10-28T11:49:47Z_
_Operation: UpdateConnector | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a call that only a RUNNING connector can serve is refused with BadRequestException, blamed on connectorArn, carrying crate::state::connector_is_not_running()" is implemented TWICE in this crate, once per handler that starts a connector operation. The two blocks are byte-for-byte identical apart from the per-operation error enum name, and both handlers' own comments say they are the same rule (update_connector.rs:269 "the same refusal a restart answers with").

The crate has already centralised the OTHER two rules these same two handlers share: the capacity rule lives in crate::state::capacity_refusal and the operation-minting/metering rule in crate::state::start_connector_operation, both of which return the message (Option<String> / Result<_, String>) and leave only the modelled error VARIANT with the handler — exactly the shape this gate needs. start_connector_operation is even called immediately after this gate in both handlers, so the one rule they still duplicate sits adjacent to the one they already share.

Because the state comparison and the blamed member name are written out twice, the copies can drift: a change to which ConnectorState accepts such a call (or to the member the refusal blames) has to be made in two files, and a change made in one silently leaves the other answering differently for the same rule. The fix is to express the gate once (e.g. a ConnectorRecord/state helper returning Option<String> alongside capacity_refusal, or folded into start_connector_operation) and keep the modelled variant in each handler. No response changes: the status, the error code and the exact message text stay as they are today.

## Evidence

Copy 1 — services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/restart_connector.rs:209-217:
```
        if connector.state != crate::state::CONNECTOR_RUNNING {
            return Err(RestartConnectorErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::invalid_parameter_because(
                    "connectorArn",
                    &crate::state::connector_is_not_running(),
                )),
            })
            .into());
        }
```

Copy 2 — services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/update_connector.rs:270-278:
```
        if connector.state != crate::state::CONNECTOR_RUNNING {
            return Err(UpdateConnectorErrorModeled::BadRequestException(BadRequestException {
                message: Some(crate::state::invalid_parameter_because(
                    "connectorArn",
                    &crate::state::connector_is_not_running(),
                )),
            })
            .into());
        }
```
The crate itself calls it one rule — update_connector.rs:267-269: "// Only a running connector has workers to move onto new settings; one // still being created, already updating or restarting, or being // deleted, has none — the same refusal a restart answers with."

The established de-duplication shape in this same crate, for the two rules the SAME pair of handlers share:
- src/state.rs:277-291: "The ONE capacity check every operation that takes a capacity runs: a create on the `Capacity` its request carried, an update on the `Capacity` its `CapacityUpdate` converts to ... so neither can accept a capacity the other refuses." (`pub fn capacity_refusal(capacity: &Capacity) -> Option<String>`)
- src/state.rs:1573-1578: "The ONE way an operation against a connector is started, run by every handler that starts one (a restart and an update), so neither can meter the store, word that refusal, or mint an operation's ARN differently from the other. Only the modelled error VARIANT and the stored record stay with the handler" (`pub fn start_connector_operation(..) -> Result<String, String>`) — called at restart_connector.rs:226 and update_connector.rs:298, i.e. directly after each copy of the duplicated gate.

Test coverage that must stay green and unchanged (both pin the exact wire message, so the extraction is verifiable):
- tests/all/connector_update.rs:988-996 — `assert_eq!(status, 400, "a connector that is already updating takes no update...")` and `assert_eq!(text(&refused, "message"), "Invalid parameter connectorArn: The connector is not RUNNING.")`; the same file also pins the still-CREATING case at lines 1010-1019.
- tests/all/connector_operations.rs:763 — "case {case:?}: a connector that is not RUNNING cannot be restarted, got ...".
- tests/all/connector_operation_window.rs:448-463 — the 400 branch asserting BadRequestException whose message contains "connectorArn", and that a refused call records no second operation, for both restart and update rows.

## Steps to Reproduce

1. Open services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/restart_connector.rs and read lines 209-217.
2. Open services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/update_connector.rs and read lines 270-278: the same condition (`connector.state != crate::state::CONNECTOR_RUNNING`), the same blamed member ("connectorArn") and the same reason helper (`crate::state::connector_is_not_running()`), differing only in the error enum name.
3. Compare with src/state.rs:287 (`capacity_refusal`) and src/state.rs:1582 (`start_connector_operation`), the two rules this same pair of handlers already shares through the state module — and note that `start_connector_operation` is invoked immediately after each duplicated gate (restart_connector.rs:226, update_connector.rs:298).
4. Change the gate in one handler only (for example, to also accept a connector in another state): the other handler keeps the old rule, and the two operations then answer differently for the one documented rule while tests/all/connector_update.rs:992 and tests/all/connector_operations.rs:763 each only cover one side.
