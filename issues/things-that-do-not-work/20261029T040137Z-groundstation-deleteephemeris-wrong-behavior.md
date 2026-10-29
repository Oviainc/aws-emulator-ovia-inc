Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c22a521c3662ecce02874628fef1ed26c367e273

# The satellite coupling admits EVERY enabled ephemeris as "in force", so a superseded one cannot be deleted while a non-terminal contact flies its satellite

_Filed: 2026-10-29T04:01:37Z_
_Operation: DeleteEphemeris | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c22a521c3662ecce02874628fef1ed26c367e273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate answers "which ephemeris is in force for a satellite?" in two places with two different rules, and the one the delete gate reaches ignores precedence entirely.

`refresh_current_ephemerides` (the rule `GetSatellite`, `ListSatellites` and `remove_ephemeris` use) states and implements that ONE ephemeris is in force per satellite: of the ENABLED ones held for it, the one of highest precedence (`ephemeris_precedence` — highest `priority`, then most recently created, then larger id) takes the satellite and becomes its `currentEphemeris`.

`contacts_pointed_by_ephemeris` — the rule `DeleteEphemeris` reaches through `contacts_using_ephemeris`, and the rule `ListContacts`' `ephemeris` filter reaches — states the SAME coupling in its own doc ("the ephemeris in force for that satellite is the trajectory its pass is flown on") but implements a different test: it admits the ephemeris whenever `ephemeris.status == EPHEMERIS_ENABLED`, never asking whether this is the ephemeris in force for that satellite. `ephemeris_precedence` is never called on this path.

So with two ENABLED ephemerides held for one satellite and one non-terminal contact flown against it, BOTH are reported as pointing that contact. `DeleteEphemeris` then refuses to delete the one that is NOT in force with `ResourceInUseException`, naming a contact whose trajectory — by this crate's own satellite rule, and by what `GetSatellite` reports as `currentEphemeris` — is computed from the OTHER ephemeris. The caller is told a superseded ephemeris is in use by a contact that is not flown on it, and can never delete it while that contact stands.

The secondary symptom of the same root cause: `ListContacts` with an `ephemeris` filter naming the not-in-force ephemeris selects contacts whose pass is flown on a different one. (That is a distinct defect from the already-open ListContacts tie-break issue 20261029T030700Z, which is about two DIFFERENT tie-breaks between `ephemeris_precedence` and `pointing_passes`; this one is about the precedence question not being asked at all on the pointing path.)

## Evidence

The crate's single-ephemeris-in-force rule, and the helper that answers it — `services/service-com-amazonaws-groundstation-groundstation/src/state.rs:5009-5014` and `:5031-5036`:
```
    /// An ephemeris is in force for the satellite it was stored against once it is
    /// [`EPHEMERIS_ENABLED`] — the status of one whose data validated and which the
    /// account asked to be used, where every other status an ephemeris can hold is one no
    /// trajectory is computed from (still validating, kept disabled, failed validation, or
    /// expired) — and the one of highest precedence takes the satellite
    /// ([`ephemeris_precedence`]).
```
```
            let takes_over = in_force.get(satellite_id).is_none_or(|held| {
                ephemeris_precedence(ephemeris, held) == ::std::cmp::Ordering::Greater
            });
            if takes_over {
                in_force.insert(satellite_id, ephemeris);
            }
```
(`ephemeris_precedence` itself at `src/state.rs:3345-3364`: "The ephemeris of two held for the same satellite that its trajectory is computed from".)

The pointing rule, whose doc states the SAME coupling — `src/state.rs:4903-4909`:
```
    /// An ephemeris is what a contact's antenna is pointed by, in either of the two ways a
    /// contact names one: through the satellite the ephemeris applies to — a contact names
    /// its satellite by ARN ([`Contact::flies_satellite`]), and the ephemeris in force for
    /// that satellite is the trajectory its pass is flown on — or by naming the ephemeris
    /// itself in the program track of its tracking overrides
```
and whose code asks only about the STATUS, never about precedence — `src/state.rs:4933-4944`:
```
        // The satellite whose passes are flown on this ephemeris, when it is in force at
        // all and applies to a satellite this account is on-boarded to: an ephemeris that
        // applies to no satellite is the trajectory of nothing.
        let flown_satellite_arn = if ephemeris.status == EPHEMERIS_ENABLED {
            ephemeris
                .satellite_id
                .as_deref()
                .and_then(|satellite_id| self.satellites.get(satellite_id))
                .map(|satellite| satellite.satellite_arn.clone())
        } else {
            None
        };
```

The in-use narrowing that inherits it — `src/state.rs:4972-4981`:
```
    pub fn contacts_using_ephemeris(&self, ephemeris_id: &str) -> Vec<String> {
        self.contacts_pointed_by_ephemeris(ephemeris_id)
            .into_iter()
            .filter(|contact_id| {
                self.contacts
                    .get(contact_id)
                    .is_some_and(Contact::is_non_terminal)
            })
            .collect()
    }
```

The refusal it produces — `src/ops/delete_ephemeris.rs:185-199`:
```
        // An ephemeris a contact still to be flown depends on is not removed: dropping it
        // would take the trajectory out from under a pass that is still to happen. So the
        // holding contacts are counted first and, when there are any, the delete is
        // refused as the modelled in-use failure naming them
        let holding = guard.contacts_using_ephemeris(&input.ephemeris_id);
        if !holding.is_empty() {
            return Err(DeleteEphemerisErrorModeled::ResourceInUseException(
```

Test coverage: the delete suite states the rule as the IN-FORCE one and only varies the ephemeris STATUS, never holding two ENABLED ephemerides for one satellite — `tests/all/ephemeris_deletion.rs:6-12` and `:26-29`:
```
//! The RULE under test: a delete REMOVES the ephemeris this account holds under the
//! `{ephemerisId}` it is addressed by ... EXCEPT while a contact still to be flown
//! depends on it, which is the ephemeris in force for that contact's satellite: then the
//! delete is refused with the modelled `ResourceInUseException` (HTTP 409)
```
```
//! * the boundary of "depends on": a table of ephemerides a non-terminal contact does NOT
//!   hold — one of another satellite, one that applies to no satellite, and one of the
//!   contact's own satellite in each `EphemerisStatus` that is not in force — every one of
//!   which deletes while that contact stands;
```
The satellite side's single-winner rule IS pinned, with its reason — `tests/all/satellite_current_ephemeris.rs:139-148` ("two in force at the same priority" → exactly one `in_force`).

## Steps to Reproduce

1. Onboard one satellite to the account (where `state::SATELLITES_ENV` lands it) and note its `satelliteId` / `satelliteArn`.
2. POST /ephemeris twice against that `satelliteId`, `enabled: true`, with different `priority` values (say 1 and 9). Note both `ephemerisId`s.
3. Wait for both to leave VALIDATING (GET /ephemeris/{id} reports ENABLED for each).
4. GET /satellite/{satelliteId}: `currentEphemeris.ephemerisId` is the priority-9 one — the ONLY ephemeris this crate says the satellite's trajectory is computed from.
5. POST /missionprofile, then POST /contact (ReserveContact) with that profile and `satelliteArn`, for a future window. The contact is non-terminal.
6. DELETE /ephemeris/{the priority-1 id}: answered 409 ResourceInUseException naming that contact, although that ephemeris is not the one in force for the satellite and nothing is flown on it.
7. Same root cause on the read side: POST /contacts with `ephemeris: {azEl: {id: <the priority-1 id>}}` selects the contact, although its pass is flown on the priority-9 ephemeris.
