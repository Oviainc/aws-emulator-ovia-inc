Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7432a15a84544829253538d508958ad90c210154

# DescribeContact never derives dataflowList and freezes contactStatus at SCHEDULED for the whole contact lifecycle

_Filed: 2026-10-23T23:53:16Z_
_Operation: DescribeContact | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7432a15a84544829253538d508958ad90c210154`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DescribeContact answers 200 but two parts of its response are not computed from the state the service already holds.

(a) `dataflowList` is hardcoded `None`. A contact is reserved against a mission profile that carries `dataflowEdges` (a list of `[sourceConfigArn, destinationConfigArn]` pairs), and `DescribeContactResponse$dataflowList` is modelled as `list<DataflowDetail>` where each `DataflowDetail` carries `source`/`destination` with `configType`, `configId` and `configDetails` — exactly the information the emulator can derive from the reserved contact's mission profile plus the config store (`state::config_arn_parts` already splits a `ConfigArn` into type + id). Real AWS returns one `DataflowDetail` per dataflow edge of the mission profile the contact was reserved against; the emulator returns the member absent/null for every contact, so a client that walks `dataflowList` to learn where a pass's data will land sees an empty contact. `visibilityStartTime`, `visibilityEndTime` and `maximumElevation` are likewise hardcoded `None`.

(b) The contact status never advances past `SCHEDULED`. `state::settle_due_contacts()` only materialises `SCHEDULING -> SCHEDULED`; no code moves a contact on to `PREPASS`, `PASS`, `POSTPASS` or `COMPLETED`. After a contact's `endTime` has elapsed, real AWS reports `contactStatus: COMPLETED` (and `PREPASS`/`PASS`/`POSTPASS` while the window is in progress, as the `ContactStatus` enum and the documented contact lifecycle describe). The emulator keeps reporting `SCHEDULED` forever, so a client polling DescribeContact (or ListContacts with `statusList: ["COMPLETED"]`) for a pass that is long over never sees it finish, and GetMinuteUsage classifies an elapsed contact as still-scheduled antenna time of a status that no longer applies. These are clock-owed transitions of the same kind the handler already materialises for `SCHEDULING`.

## Evidence

Spec: `smithy_query(query="operation", operation="DescribeContact")` models `dataflowList` (`DataflowList` of `DataflowDetail`), `visibilityStartTime`, `visibilityEndTime`, `maximumElevation` and `contactStatus` (`ContactStatus` enum = SCHEDULING, FAILED_TO_SCHEDULE, SCHEDULED, CANCELLED, AWS_CANCELLED, PREPASS, PASS, POSTPASS, COMPLETED, FAILED, AVAILABLE, CANCELLING, AWS_FAILED). `DataflowDetail`/`Source`/`Destination` expose `configType`/`configId`, which the stored mission profile's `dataflowEdges` ARNs already name.
Handler: `services/service-com-amazonaws-groundstation-groundstation/src/ops/describe_contact.rs` lines ~202-213 set `error_message: None, maximum_elevation: None, dataflow_list: None, visibility_start_time: None, visibility_end_time: None` unconditionally; `src/state.rs` `settle_due_contacts()` only performs the `SCHEDULING -> SCHEDULED` move (`scheduling_settles_at`), and `Contact` carries no field for any later lifecycle stage.
Tests: `grep -n 'dataflowList|dataflow_list' services/.../tests` returns no hit — no test asserts the dataflow list; `tests/all/scheduling_loop.rs` only asserts the `SCHEDULING -> SCHEDULED` settle and a cancel, so neither symptom is covered.
Not covered by the open issue `20261023T171627Z-groundstation-ground-station-config-missionprofile-fidelity-...`, which is about Config/MissionProfile validation and not-found texts.

## Steps to Reproduce

1. CreateConfig an antenna-downlink config and a dataflow-endpoint config, plus a tracking config.
2. CreateMissionProfile with dataflowEdges=[[antennaDownlinkConfigArn, dataflowEndpointConfigArn]] and trackingConfigArn=<tracking arn>, minimumViableContactDurationSeconds=60.
3. ReserveContact against that mission profile with a short window.
4. DescribeContact(contactId): observe `dataflowList` absent/null where AWS returns one DataflowDetail per mission-profile dataflow edge (source=antenna-downlink config, destination=dataflow-endpoint config).
5. Reserve a contact whose window is short and already elapsed (or wait past endTime), then DescribeContact again: the emulator still reports contactStatus SCHEDULED where AWS reports PREPASS/PASS/POSTPASS/COMPLETED as the window passes; ListContacts with statusList=["COMPLETED"] returns nothing for the same contact.
