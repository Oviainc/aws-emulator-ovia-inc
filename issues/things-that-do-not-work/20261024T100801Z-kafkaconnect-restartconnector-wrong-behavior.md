Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ca029f426fda2d51f4b9c87edd69d4539a13e33a

# Keep a restarted connector RESTARTING until its restart operation reaches a terminal state

_Filed: 2026-10-24T10:08:01Z_
_Operation: RestartConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ca029f426fda2d51f4b9c87edd69d4539a13e33a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A restart is tracked by TWO independent clocks that fall due at different instants, so the connector's state and the state of the connector operation that represents the restart disagree, and a second restart is accepted while the first is still in progress.

RestartConnector does two things: it flips the connector to RESTARTING with a pending `Settle` due one `TRANSITION_SPAN` (500 ms) later, and it records a `ConnectorOperationRecord::restart` whose progression is PENDING -> RESTART_IN_PROGRESS -> RESTART_COMPLETE, one `TRANSITION_SPAN` per step, i.e. terminal only after 2 x 500 ms.

Expected (real service / the emulator's own stated intent): the connector is RESTARTING for exactly as long as the restart it is running — its return to RUNNING is the SAME event as the restart operation reaching its terminal state (RESTART_COMPLETE). Consequently, while a restart operation is PENDING or RESTART_IN_PROGRESS, (a) DescribeConnector/ListConnectors report the connector as not RUNNING, and (b) a further RestartConnector against that connector is refused — operations against one connector are serialized by the service, which is precisely the refusal this handler implements ("only a connector that is running HAS tasks to restart", restart_connector.rs lines 183-200, and the test comment "RESTARTING: the restart already under way is the one in progress").

Actual (emulator): in the window between +500 ms and +1000 ms after the call, DescribeConnector reports `connectorState: RUNNING` while DescribeConnectorOperation / ListConnectorOperations report the same restart as `RESTART_IN_PROGRESS` with no `endTime`. A client polling DescribeConnector concludes the restart finished while the operation record says it has not. In that same window a second RestartConnector is ACCEPTED (the connector reads RUNNING), recording a second, overlapping RESTART_CONNECTOR operation against a connector that is still restarting — the exact state the handler means to refuse.

## Evidence

Spec/model grounding: `ConnectorState` models RESTARTING as the connector state while a restart runs, and `ConnectorOperationState` models the restart progression PENDING | RESTART_IN_PROGRESS | RESTART_COMPLETE for `ConnectorOperationType = RESTART_CONNECTOR` (smithy_query shape ConnectorState / ConnectorOperationState / ConnectorOperationType). The restart of a connector and the connector operation that reports it are one and the same piece of asynchronous work, so the connector cannot be RUNNING again while the operation reporting that work is still non-terminal; MSK Connect likewise serializes operations on a connector (a connector in a non-RUNNING/in-flight state is refused a further operation with BadRequestException), which this crate already encodes as its refusal rule.

Handler: services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/restart_connector.rs lines 232-246 — the connector is given `PendingChange::in_a_moment(PendingKind::Settle)` (one TRANSITION_SPAN) while `ConnectorOperationRecord::restart` (src/state.rs lines 471-488) sets `ends_at = creation_time + TRANSITION_SPAN * RESTART_PROGRESSION.len()` (two spans) and advances one state per span in `advance_due` (src/state.rs lines 509-521). `settle_due` (src/state.rs lines 632-651) applies the two independently, so the connector's Settle fires one span before the operation's terminal state.

Tests: tests/all/connector_operations.rs pins only the two ends of the window — `a_restart_records_an_operation_that_reads_back_from_state_and_ends_on_the_clock` asserts RESTARTING immediately after the call (line 345) and RUNNING after `past_the_whole_restart()` (lines 485-491), and `a_connector_that_is_not_running_is_refused_a_restart_and_records_nothing` refuses the second restart only immediately after the first (line 792, inside the first 500 ms). No test observes the connector or a second restart in the +500 ms..+1000 ms window, so nothing asserts the current divergence.

(Note: the open issue 20261024T023759Z-kafkaconnect-restartconnector-stub is stale — restart_connector.rs is fully implemented and carries no `//! SCAFFOLDED, NOT IMPLEMENTED:` marker; this is a behavioral gap in that implementation.)

## Steps to Reproduce

1. Create a custom plugin and wait for it to become ACTIVE, then POST /v1/connectors and wait for the connector to become RUNNING.
2. POST /v1/connectors/{connectorArn}/restart and note the returned connectorOperationArn.
3. Wait slightly longer than one transition span (just over 500 ms) but less than two (under 1000 ms).
4. GET /v1/connectors/{connectorArn} — the emulator reports connectorState RUNNING, while GET /v1/connectorOperations/{connectorOperationArn} still reports connectorOperationState RESTART_IN_PROGRESS with no endTime; the real service keeps the connector out of RUNNING until the restart operation is terminal.
5. In the same window POST /v1/connectors/{connectorArn}/restart again — the emulator answers 200 and GET /v1/connectors/{connectorArn}/operations then lists TWO overlapping RESTART_CONNECTOR operations; the real service refuses a restart while an operation against that connector is still in progress (400 BadRequestException naming connectorArn, the refusal this handler already renders for a connector that is not RUNNING).
