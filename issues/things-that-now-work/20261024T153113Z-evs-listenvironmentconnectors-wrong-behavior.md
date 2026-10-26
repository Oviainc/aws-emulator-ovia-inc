Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# An ACTIVE connector never reports connectivity health: status is stuck at UNKNOWN and checks is always absent

_Filed: 2026-10-24T15:31:13Z_
_Operation: ListEnvironmentConnectors | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Connector$checks` is "A list of checks that are run on the connector" (ConnectorCheck: "A check on a connector to identify connectivity health") and `Connector$status` is the CheckResult roll-up of those checks. The connector exists precisely so that "Amazon EVS requires an active connector to SDDC Manager or VCF Operations Manager to monitor environment health and license compliance" (CreateEnvironmentConnector doc), and the model declares the connector-scoped check types CONNECTOR_HEALTH, VCENTER_REACHABILITY, SDDC_MANAGER_REACHABILITY and OPERATIONS_MANAGER_REACHABILITY.

Expected: once a connector settles into ACTIVE — the state this emulator itself defines as the established connection to the appliance — a read reports at least the connector-health/reachability check the state decides, with `status` rolling up to PASSED. A caller polling connector health (the documented way to see whether EVS can monitor the environment) must eventually observe a decided result.

Actual: `StoredConnector` is created with `checks: Vec::new()` and nothing ever appends to it, so `to_wire` omits `checks` for the connector's whole life, and `StoredConnector::status()` returns UNKNOWN whenever `checks.is_empty()`. An ACTIVE VCENTER/SDDC_MANAGER/OPERATIONS_MANAGER connector therefore reports `status: "UNKNOWN"` and no `checks` forever, on CreateEnvironmentConnector, ListEnvironmentConnectors and UpdateEnvironmentConnector alike — connector health is permanently unobservable. This is the same shape of gap that was already filed and fixed for the environment (`Environment$checks` / `environmentStatus`, which are now derived from the stored record for the decidable check types); the connector's own state machine is equally decidable from the store (ACTIVE vs CREATING/UPDATING/CREATE_FAILED), so a derived CONNECTOR_HEALTH result would claim nothing the record does not already say.

## Evidence

Spec: `Connector$checks` + `ConnectorCheck` doc ("A check on a connector to identify connectivity health"), `Connector$status` (CheckResult), CheckType enum values CONNECTOR_HEALTH / VCENTER_REACHABILITY / SDDC_MANAGER_REACHABILITY / OPERATIONS_MANAGER_REACHABILITY (smithy_query shape=CheckType), CreateEnvironmentConnector doc ("Amazon EVS requires an active connector ... to monitor environment health and license compliance"). Handler/state: src/state.rs `StoredConnector::creating` sets `checks: Vec::new()` (line ~526) and nothing else writes the field; `StoredConnector::status()` (lines 554-566) returns CHECK_UNKNOWN while `checks.is_empty()`; `to_wire` (lines 570-584) emits `checks` only when non-empty. src/ops/list_environment_connectors.rs lines 224-244 reports those records verbatim after `materialise_due_transitions`, which recomputes ENVIRONMENT checks only (`recompute_environment_checks`) and never a connector's. Tests: tests/all/connectors.rs lines 270-286 pin status=UNKNOWN / checks=null for a freshly created connector that is still CREATING (where UNKNOWN is legitimate) and lines 305-308 only assert `state == ACTIVE` after the transition — no test asserts status/checks for a settled ACTIVE connector, so this rule is uncovered.

## Steps to Reproduce

1. Create an environment (VPC + service access subnet registered in the EC2 store) and wait for it to reach CREATED.
2. Call CreateEnvironmentConnector with type=VCENTER and a valid applianceFqdn/secretIdentifier.
3. Wait past the pending CREATING->ACTIVE transition, then call ListEnvironmentConnectors (or GetEnvironment's connector-dependent checks).
4. Observe the connector reports state=ACTIVE but status="UNKNOWN" and no `checks` member at all — no connectivity-health result is ever produced for any connector type or state.
