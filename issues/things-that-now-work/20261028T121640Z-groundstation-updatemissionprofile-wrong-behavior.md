Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# Updating a mission profile retroactively rewires the dataflows of contacts already reserved against it, which the operation's own model doc forbids

_Filed: 2026-10-28T12:16:40Z_
_Operation: UpdateMissionProfile | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states, for this operation, that an update must NOT reach contacts that are already scheduled. The handler nonetheless replaces the stored `dataflowEdges` in place, and a contact keeps only the profile's ARN: every read of a contact derives its `dataflowList` from the profile as it stands NOW (`crate::state::contact_dataflow_list`). So after `UpdateMissionProfile` replaces (or empties) `dataflowEdges`, a contact reserved BEFORE the update — one still in SCHEDULING/SCHEDULED, i.e. an "existing future contact" — reports the new wiring from `DescribeContact` and `DescribeContactVersion`, and is therefore flown with execution parameters it was not reserved with.

This is also inconsistent inside the handler pair: `ReserveContact` already FREEZES part of the profile onto the contact — the pre- and post-pass durations are read from the profile at reserve time and stored as `pre_pass_start_time`/`post_pass_end_time`, so a later update of those durations correctly does not move an existing contact's window — while the dataflow wiring from the same profile is left live. One half of the same profile is snapshotted and the other half is not.

The defect is the retroactive reach, not the storage layout: the pre-update wiring of every non-terminal contact that holds the profile has to survive the update. Note that `tests/all/mission_profile_deletion.rs` pins that a contact whose profile is GONE reports no `dataflowList` at all, so the pre-update wiring must only be captured when an update actually changes it, not snapshotted at reserve time.

## Evidence

Model (smithy_query(query="doc", shape="UpdateMissionProfile")): "Updates a mission profile." / "Updating a mission profile will not update the execution parameters for existing future contacts."

Handler replaces the edges in place — services/service-com-amazonaws-groundstation-groundstation/src/ops/update_mission_profile.rs:251-253:
```
        if let Some(edges) = input.dataflow_edges {
            stored.dataflow_edges = edges;
        }
```

A contact's dataflows are read LIVE out of that same profile — services/service-com-amazonaws-groundstation-groundstation/src/state.rs:4437-4440 and 4444-4456:
```
    /// Read from the profile rather than copied into the contact, so there is one record of
    /// each edge; a contact whose profile has since been DELETED (a terminal contact no
    /// longer holds its profile) reports NO dataflow list at all rather than an empty one,
...
    pub fn contact_dataflow_list(
...
        let profile = self.mission_profile_by_arn(mission_profile_arn)?;
        Some(
            profile
                .dataflow_edges
                .iter()
                .map(|edge| dataflow_detail(self, edge, contact_region))
```
and services/service-com-amazonaws-groundstation-groundstation/src/ops/describe_contact.rs:203:
```
        let dataflow_list = guard.contact_dataflow_list(&contact.mission_profile_arn, &contact.region);
```
(same call in src/ops/describe_contact_version.rs:262-263).

The SAME handler pair does freeze the other half of the profile — services/service-com-amazonaws-groundstation-groundstation/src/ops/reserve_contact.rs:288-297:
```
            // The pre/post-pass window is the contact window widened by the profile's
            // pre- and post-pass durations; an unset duration widens it by nothing.
            pre_pass_start_time: crate::state::shift_seconds(
                &input.start_time,
                -i64::from(profile.contact_pre_pass_duration_seconds.unwrap_or(0)),
            ),
```

Test coverage: tests/all/contact_dataflows_and_pass_stages.rs:326 asserts a contact's `dataflowList` is one `DataflowDetail` per edge of its profile; tests/all/mission_profile_deletion.rs:227 asserts a contact whose profile is gone reports no `dataflowList`. No test exercises a read of an already-reserved contact AFTER its profile was updated, so nothing pins today's retroactive behaviour.

## Steps to Reproduce

1. Create two tracking/dataflow-endpoint configs and a mission profile whose `dataflowEdges` wires edge A (CreateConfig, CreateMissionProfile).
2. ReserveContact against that profile for a window in the future; GET /contact/{contactId} and record `dataflowList` (one entry, describing edge A).
3. Call UpdateMissionProfile on the same profile with a different `dataflowEdges` (or `[]`).
4. GET /contact/{contactId} again for the SAME, still-future contact.
5. Observed: `dataflowList` now describes the NEW edges (or is empty) — the execution parameters of an existing future contact changed. Expected per the operation's own documentation: the contact keeps the wiring it was reserved with.
