Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# DeleteConnector deletes synchronously: the DELETING status is never observable and a CREATING connector is refused instead of being deleted

_Filed: 2026-10-24T02:08:31Z_
_Operation: DeleteConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: DeleteConnector is asynchronous — it answers HTTP 202 ("accepted") and the connector moves into the modeled ConnectorStatus value DELETING, which GetConnector/ListConnectors report until the teardown completes and the record disappears (then 404). DELETING is one of only four ConnectorStatus values and, for this API, is reachable only through DeleteConnector; the emulator already models exactly this clock-driven pattern for CREATING -> ACTIVE (state::PendingTransition / TRANSITION_SPAN), so the machinery exists.

Actual: the handler removes the record from the store inside the request, so the connector jumps straight from ACTIVE to absent: a GetConnector immediately after the delete returns 404 and no caller can ever observe Status=DELETING. In addition, deleting a connector that is still CREATING is refused with ConflictException (409) and the connector is left in place, so a caller who creates and then deletes must poll for ACTIVE before the delete can be accepted — the real service accepts the delete and tears the connector down (DELETING), it does not require the resource to be settled first.

## Evidence

Smithy model: ConnectorStatus = [CREATING, ACTIVE, DELETING, FAILED] (smithy_query shape=ConnectorStatus); DeleteConnector http code 202 with an empty output (smithy_query operation=DeleteConnector), i.e. the call only accepts the work. Handler: src/ops/delete_connector.rs lines 184-204 — the ConflictException branch for `stored.is_in_transition()` and the immediate `store.connectors.remove(...)`; state.rs StoredConnector has no DELETING path (only PendingTransition to ACTIVE, lines 213-225). Tests assert the current synchronous behavior (tests/all/connector_lifecycle.rs lines 1206-1221 expect 404 right after the delete) and the conflict (`deleting_a_connector_mid_creation_conflicts`, line 1414); nothing covers Status=DELETING.

## Steps to Reproduce

1. CreateConnector; wait until GetConnector reports Status=ACTIVE.
2. Call DeleteConnector (202), then immediately call GetConnector with the same ARN.
3. Observe 404 ResourceNotFoundException; expected 200 with Connector.Status=DELETING for the teardown window, with the record disappearing only afterwards (and ListConnectors showing it as DELETING meanwhile).
4. Separately: CreateConnector and call DeleteConnector right away while Status=CREATING — observe 409 ConflictException and the connector still present; expected the delete to be accepted (202) and the connector to tear down.
