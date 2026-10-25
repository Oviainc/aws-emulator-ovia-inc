Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# A created connection never leaves 'requested': the pending/available half of the ConnectionState lifecycle is unreachable

_Filed: 2026-10-24T13:51:55Z_
_Operation: CreateConnection | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: the Smithy model documents the connection lifecycle as `requested` ("the initial state of a connection. The state will remain here until the Connection is accepted on the Partner portal") -> `pending` ("the connection has been accepted and is being provisioned between AWS and the Partner") -> `available` ("the connection has been fully provisioned between AWS and the Partner"), with `down`/`failed` as the impaired outcomes. Against the real service a connection created by CreateConnection does progress out of `requested`, so a client that polls GetConnection until `available`, filters ListConnections by state=available/pending, or updates an `available` connection, eventually gets an answer.

Actual: CreateConnection stores `state: "requested"` with `pending_transition: None`, and NO code path in the crate ever writes `pending`, `available`, `down` or `failed` onto a connection. The only state writes are `deleting`/`deleted` (DeleteConnection) and `updating` (UpdateConnection, which settles back to the state the connection came from - i.e. `requested`). The emulator already models clock-driven settling for delete and update (`PendingTransition` + `materialise_due_transitions`), but a created connection is left in `requested` forever, and AcceptConnectionProposal - the only other write path - is a 501 scaffold (and models the inbound-proposal side, not activation of this connection).

Observable consequences: GetConnection/ListConnections report `requested` for the life of the process; `ListConnections(state="available")` and `(state="pending")` are permanently empty; the primary workflow "create connection -> wait until available -> use/update it" can never complete; and UpdateConnection's permitted-from-`available` branch, which the handler explicitly allows (UPDATABLE_CONNECTION_STATES = ["requested", "available"]), is dead code that no caller can reach.

## Evidence

Spec: smithy_query on com.amazonaws.interconnect#ConnectionSummary$state / Connection$state enumerates requested|pending|available|deleting|deleted|down|failed|updating with the documented progression quoted above (ConnectionState doc). Handler: src/ops/create_connection.rs lines 314-346 set `state: "requested"` and insert `StoredConnection { pending_transition: None, .. }`. grep over services/service-com-amazonaws-interconnect-interconnect/src for state writes finds only src/ops/delete_connection.rs:168 ("deleting" + pending "deleted"), src/ops/update_connection.rs:235 ("updating", settling back via `settled_state`) and src/state.rs:217 (applying a pending transition) - no literal "pending"/"available" is ever assigned to a connection (the "available" occurrences in state.rs are Environment state and the bandwidth `available` set). AcceptConnectionProposal is still the scaffold stub (own issue filed).
Tests: tests/all/refusals_and_gateway_attach_points.rs lines 246-394 drive requested -> updating -> deleting -> deleted only; its UPDATABLE_STATES table never reaches `available`, and tests/all/connection_lifecycle.rs asserts a fresh connection reports `requested`. No test asserts any progression past `requested`, so nothing covers this.
Not covered by existing issues: the open wire-fidelity issue (20261023T095130Z) covers the environment/attach-point seed, the not-found status and the validation-message form; the open error-semantics issue (20261022T145736Z) covers error codes/messages and stubs; neither mentions connection state progression.

## Steps to Reproduce

1. Boot the emulator and call ListEnvironments; pick a seeded environmentId and one of its available bandwidths.
2. CreateConnection(environmentId=<that id>, bandwidth=<that bandwidth>, attachPoint={directConnectGateway: <uuid>}) -> note connection.id and connection.state == "requested".
3. Poll GetConnection(identifier=<id>) repeatedly for several seconds: observe state stays "requested" indefinitely; expect it to progress to "pending" and then "available" as the circuit is provisioned.
4. Call ListConnections(state="available") and ListConnections(state="pending") -> both always return an empty connections list, so no filter value other than requested/updating/deleting/deleted can ever match.
5. Note that UpdateConnection's documented permitted state "available" (per its own refusal message "only connections in 'requested' or 'available' state can be updated") can never be exercised.
