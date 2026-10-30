Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 86a1fe66fd012b6ef331ae9dc1d2d842007fb4fe

# UpdateEphemeris disables an ephemeris a non-terminal contact is flown on, taking the trajectory out from under the pass DeleteEphemeris refuses to touch — and clearing the delete's own in-use guard

_Filed: 2026-10-28T18:34:10Z_
_Operation: UpdateEphemeris | Class: validation_

> _Note: this issue was identified against the code base at commit `86a1fe66fd012b6ef331ae9dc1d2d842007fb4fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteEphemeris` and `UpdateEphemeris` are the two writers of a stored ephemeris in this crate, and only one of them consults the crate's in-use rule.

`DeleteEphemeris` calls `crate::state::contacts_using_ephemeris` and, when any non-terminal contact depends on the ephemeris, refuses the call outright, on the stated grounds that dropping it "would take the trajectory out from under a pass that is still to happen".

`UpdateEphemeris` never calls that helper. Its required `enabled` member is written straight onto the stored record and, when the ephemeris has already validated, its status is flipped `ENABLED` -> `DISABLED` in the same call. By the crate's own definition that removes the trajectory from the live pass just as surely as a delete would:

* `contacts_using_ephemeris` only couples an ephemeris to its satellite's contacts while `status == EPHEMERIS_ENABLED`, documented as "one still validating, disabled, invalid or expired is not the trajectory anything is flown on";
* `refresh_current_ephemerides` skips any ephemeris whose status is not `EPHEMERIS_ENABLED`, so the satellite's `currentEphemeris` reference is dropped on the next `GetSatellite`/`ListSatellites`;
* `pointing_passes` reads pointing data only from an `EPHEMERIS_ENABLED` ephemeris, so the windows `ListContacts` computes for that pairing disappear too.

The second-order effect is that the guard `DeleteEphemeris` applies becomes a two-call bypass: `PUT /ephemeris/{id}` with `enabled: false` makes `contacts_using_ephemeris` answer empty for the satellite coupling, and the `DELETE /ephemeris/{id}` that was refused a moment earlier then succeeds and removes the ephemeris permanently — exactly the state the delete's comment says it exists to prevent. (A contact that program-tracks the ephemeris OUTRIGHT is still protected, because `program_tracks_ephemeris` holds "whatever its status"; the satellite-coupled contacts are not.)

Nothing in the crate records a reason for the asymmetry: the `UpdateEphemeris` suite never puts a contact in the store at all, so the acceptance is unpinned.

## Evidence

The sibling writer applies the guard — `services/service-com-amazonaws-groundstation-groundstation/src/ops/delete_ephemeris.rs:185-199`:
```
        // An ephemeris a contact still to be flown depends on is not removed: dropping it
        // would take the trajectory out from under a pass that is still to happen. So the
        // holding contacts are counted first and, when there are any, the delete is
        // refused as the modelled in-use failure naming them — the caller flies or
        // releases those first and retries. Nothing is written on this path, so the
        // ephemeris and every contact are left exactly as they were.
        let holding = guard.contacts_using_ephemeris(&input.ephemeris_id);
        if !holding.is_empty() {
            return Err(DeleteEphemerisErrorModeled::ResourceInUseException(
```

The update writer does not — its ENTIRE write path, `src/ops/update_ephemeris.rs:203-227`:
```
        // `enabled` is required, so every update states whether the ephemeris is to be
        // used; `name` and `priority` are optional, and one the request omits leaves the
        // stored value alone.
        stored.enabled = input.enabled;
...
        if matches!(
            stored.status.as_str(),
            crate::state::EPHEMERIS_ENABLED | crate::state::EPHEMERIS_DISABLED
        ) {
            stored.status = if stored.enabled {
                crate::state::EPHEMERIS_ENABLED.to_string()
            } else {
                crate::state::EPHEMERIS_DISABLED.to_string()
            };
        }
```
(`contacts_using_ephemeris` appears nowhere in this handler; `grep -n 'contacts_using_ephemeris' src/ops` matches only `delete_ephemeris.rs:191` and `list_contacts.rs:300`.)

The crate's own statement that a DISABLED ephemeris is no longer the trajectory — `src/state.rs:4706-4714`:
```
    /// For the SATELLITE coupling, only an ephemeris whose status is [`EPHEMERIS_ENABLED`]
    /// is in force: that status is what "whether or not the ephemeris is enabled"
    /// (`smithy_query(query="operation", operation="DescribeEphemeris")`) reports, and one
    /// still validating, disabled, invalid or expired is not the trajectory anything is
    /// flown on. A contact that names an ephemeris OUTRIGHT holds it whatever its status:
```
and the code it documents, `src/state.rs:4725`:
```
        let flown_satellite_arn = if ephemeris.status == EPHEMERIS_ENABLED {
```

The two readers that then stop seeing it — `src/state.rs:4795`:
```
            if ephemeris.status != EPHEMERIS_ENABLED {
                continue;
            }
```
(in `refresh_current_ephemerides`, which `GetSatellite` calls at `src/ops/get_satellite.rs:183`), and `src/state.rs:5028-5030`:
```
    /// Pointing data is in force when it belongs to an ENABLED ephemeris
    /// ([`EPHEMERIS_ENABLED`] — the same reading of "in force" as
    /// [`Self::contacts_using_ephemeris`]) that applies to that satellite
```

Test coverage: `tests/all/ephemeris_deletion.rs:622-632` + `678-684` pin that an ephemeris CREATED disabled for the flown satellite is deletable ("the flown satellite's, but disabled" ... "no contact depends on this ephemeris, so the delete is answered") — which is the same status-keyed rule, and is what makes the two-call bypass land on 200. No row anywhere disables an ephemeris that a live contact is flown on: `tests/all/ephemeris_updates.rs:1-31` lists the whole suite's scope (flag in both directions, name, priority, refusals) and it never reserves a contact, so nothing pins the acceptance.

## Steps to Reproduce

1. Set `GROUNDSTATION_SATELLITES=25994=Ohio 1` so the account is on-boarded to one satellite reachable from ground station `Ohio 1`.
2. `POST /ephemeris` with `satelliteId` = that satellite, `enabled: true`, a forward-running `tle` validity range and `expirationTime` in the future; wait out the validation span so `GET /ephemeris/{id}` reports `ENABLED`.
3. `POST /config` + `POST /missionprofile`, then `POST /contact` (ReserveContact) against that profile, `groundStation: "Ohio 1"` and the satellite's ARN, with a future window — the contact is non-terminal.
4. `DELETE /ephemeris/{id}` -> 409 `ResourceInUseException`, naming that contact: the service says the contact depends on this trajectory.
5. `PUT /ephemeris/{id}` with `{"enabled": false}` -> 200. `GET /ephemeris/{id}` now reports `DISABLED`.
6. `GET /satellite/{satelliteId}` no longer reports the ephemeris as `currentEphemeris`, and `POST /contacts` with `statusList: ["AVAILABLE"]` for the same pairing offers no window — the live contact's trajectory is gone, with no refusal anywhere.
7. `DELETE /ephemeris/{id}` -> 200: the ephemeris the service refused to delete in step 4 is now removed permanently, while the contact is still non-terminal.
