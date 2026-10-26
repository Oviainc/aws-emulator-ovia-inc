Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f2163892bf40cea6da7d1662f28c14ec75c797c6

# DeleteMissionProfile has no in-use guard, so a live contact is left pointing at a profile ARN that no longer resolves

_Filed: 2026-10-26T23:22:45Z_
_Operation: DeleteMissionProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `f2163892bf40cea6da7d1662f28c14ec75c797c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteMissionProfile` removes the profile and its ARN-index entry unconditionally: it takes the lock and calls `remove_mission_profile` as its very first act, with no scan of `guard.contacts` for contacts that still name the profile by `missionProfileArn` (and without `settle_due_contacts()`, so it could not classify them even if it looked).

This contradicts the rule the crate's two sibling deletes implement and state in their own comments, and it breaks a rule a state-module comment asserts:

1. `DeleteConfig` refuses a config a live mission profile references, for exactly this reason ("dropping one out from under it would leave it pointing at a reference that no longer resolves"), using `mission_profiles_referencing` + `InvalidParameterException`.
2. `DeleteEphemeris` refuses an ephemeris a non-terminal contact depends on, using `contacts_using_ephemeris` + `ResourceInUseException`.
3. `state::contact_dataflow_list`'s doc comment asserts that only a TERMINAL contact can be in the position of having a deleted profile — with no guard on the delete, a `SCHEDULING`/`SCHEDULED` contact reaches that path.

Observable damage to a contact that is still to be flown:
- `DescribeContact` / `DescribeContactVersion` stop reporting `dataflowList` entirely (it is derived from the profile through `contact_dataflow_list`, which returns `None` once `mission_profile_by_arn` resolves nothing), so a live reservation suddenly reports no dataflows.
- `DescribeContact`/`ListContacts` keep reporting `missionProfileArn` for an ARN every other read of this service now answers 434 for.
- `ListContacts` with `statusList` containing `AVAILABLE` for that same ARN answers `ResourceNotFoundException` / `MissionProfile not found`, i.e. the discovery half of the loop refuses the ARN the contact listing still hands back.

The contact's pre/post-pass widening was copied at reserve time, so the contact survives as a half-resolvable record rather than being refused or cleaned up.

## Evidence

No guard — `services/service-com-amazonaws-groundstation-groundstation/src/ops/delete_mission_profile.rs:143-155`:
```
        let mut guard = state.lock();
        // The removal drops the profile AND the ARN index entry that resolved to it, so
        // from here on a read of it, and any ARN-valued reference to it, answers as it
        // does for a profile that never existed.
        let removed = guard
            .remove_mission_profile(&input.mission_profile_id)
            .ok_or_else(|| { ... MISSION_PROFILE_NOT_FOUND ... })?;
```
(no read of `guard.contacts`, no `settle_due_contacts()`).

Sibling that DOES apply the rule — `src/ops/delete_config.rs:168-184`:
```
        // A config a live mission profile still depends on is NOT deleted: a mission
        // profile names its configs by ARN, and dropping one out from under it would
        // leave it pointing at a reference that no longer resolves. So the inbound
        // references are counted first and, when there are any, the delete is refused as
        // a bad parameter naming the profiles that hold them
        let referencing = guard.mission_profiles_referencing(&config_arn);
        if !referencing.is_empty() { return Err(... InvalidParameterException ...); }
```

Second sibling — `src/ops/delete_ephemeris.rs:185-199`:
```
        // An ephemeris a contact still to be flown depends on is not removed: dropping it
        // would take the trajectory out from under a pass that is still to happen.
        let holding = guard.contacts_using_ephemeris(&input.ephemeris_id);
        if !holding.is_empty() { return Err(... ResourceInUseException ...); }
```

The comment the code breaks — `src/state.rs:3639-3642`:
```
    /// Read from the profile rather than copied into the contact, so there is one record of
    /// each edge; a contact whose profile has since been DELETED (a terminal contact no
    /// longer holds its profile) reports NO dataflow list at all rather than an empty one,
    /// since what it was wired to is then no longer known.
```
and its body, `src/state.rs:3650-3651`: `let profile = self.mission_profile_by_arn(mission_profile_arn)?;` — i.e. `None` for the whole `dataflowList`.

The read that goes hollow — `src/ops/describe_contact.rs:197` and `:221`:
```
        let dataflow_list = guard.contact_dataflow_list(&contact.mission_profile_arn, &contact.region);
        ...
            dataflow_list,
```

The other half of the same ARN's fate — `src/ops/list_contacts.rs:359-366`:
```
            let profile = guard
                .mission_profile_by_arn(input.mission_profile_arn.as_deref().unwrap_or_default())
                .cloned()
                .ok_or_else(|| { ... ResourceNotFoundException { message: Some(crate::state::MISSION_PROFILE_NOT_FOUND.to_string()) } ... })?;
```

Test coverage: `DeleteMissionProfile` is exercised only in `tests/all/lifecycle_writes.rs:215-330` (`a_deleted_mission_profile_is_gone_from_every_read_and_every_reference`) and `tests/all/config_defaults_and_references.rs:491-508` / `:878-889`. In every one of those the profile being deleted has NO contact reserved against it — `lifecycle_writes.rs:303-330` only reserves AFTER the delete, to assert the reservation is refused 434 — so no existing assertion pins a profile being deletable while a live contact holds it, and adding the guard changes no current test outcome.

## Steps to Reproduce

1. CreateConfig a trackingConfig (`{"trackingConfig": {"autotrack": "REQUIRED"}}`) and keep its configArn.
2. CreateMissionProfile with `minimumViableContactDurationSeconds: 30`, `dataflowEdges: [[<downlink arn>, <endpoint arn>]]`, `trackingConfigArn: <tracking arn>`; keep `missionProfileId` and read `missionProfileArn` with GetMissionProfile.
3. ReserveContact with that `missionProfileArn`, `groundStation: "Ohio-1"`, `startTime`/`endTime` well in the future — 200 with a `contactId`.
4. GET /contact/{contactId} (DescribeContact): the response carries `dataflowList` with one entry per dataflow edge, and `contactStatus` SCHEDULING/SCHEDULED.
5. DELETE /missionprofile/{missionProfileId}: the emulator answers 200 and removes the profile, although a non-terminal contact still names it — where DeleteConfig and DeleteEphemeris both refuse the analogous delete.
6. GET /contact/{contactId} again: `dataflowList` is now absent entirely for a contact whose pass is still ahead, while `missionProfileArn` still reports the deleted ARN.
7. POST /contacts with `statusList: ["AVAILABLE"]`, the same `missionProfileArn`, `groundStation` and a `satelliteArn`: 434 `ResourceNotFoundException` "MissionProfile not found" for the very ARN the contact listing still hands back.
