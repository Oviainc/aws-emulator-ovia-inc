Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# A contact is reserved against a satelliteArn that names no satellite of this account, which the crate's own in-use and pointing rules then cannot resolve

_Filed: 2026-10-28T18:06:50Z_
_Operation: ReserveContact | Class: validation_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ReserveContact` resolves every other reference its request carries before it writes anything — the `missionProfileArn` against `mission_profile_by_arn` (434) and the `trackingOverrides` program-track `ephemerisId` against `program_track_fault` (434/431) — but stores `satelliteArn` verbatim into the contact's first version, resolving nothing. `UpdateContact` appends it by the same unresolved route, so both writers of a contact version are affected (this is the exact gap the fixed issue `20261026T065303Z-groundstation-updatecontact-validation` left behind: it named "`CreateEphemeris` and `ListEphemerides` refuse a `satelliteId` this account is not on-boarded to" as the sibling rule and fixed only the `ephemerisId` half).

The consequences are not cosmetic, because the crate's own rules key on that ARN resolving to a stored satellite:

* `contacts_using_ephemeris` derives the satellite coupling as `ephemeris.satellite_id -> self.satellites.get(..) -> satellite.satellite_arn` and then asks `contact.flies_satellite(arn)`. A contact whose `satelliteArn` resolves to no on-boarded satellite therefore can never match, so `DeleteEphemeris`'s `ResourceInUseException` guard (and `ListContacts`' ephemeris filter, which uses the same helper) silently misses it: the ephemeris is deleted out from under a live contact, which is precisely what that guard exists to prevent.
* `pointing_passes` selects pointing data by `satellite.satellite_arn == satellite_arn`, so an `AVAILABLE` search for that pairing can never describe the pass the contact holds — the discovery-then-reserve loop and the reservation disagree about which satellite exists.
* `DescribeContact` / `DescribeContactVersion` / `ListContacts` then report a `satelliteArn` that `GetSatellite` answers 434 for.

The crate already holds everything the gate needs: the `satellites` store, each satellite's minted `satellite_arn`, and the shared not-found text for a satellite this account is not on-boarded to.

## Evidence

The same handler resolves its other two references — `src/ops/reserve_contact.rs:164-173`:
```
        let profile = guard
            .mission_profile_by_arn(&input.mission_profile_arn)
            .ok_or_else(|| {
                ReserveContactErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                    message: Some(crate::state::MISSION_PROFILE_NOT_FOUND.to_string()),
```
and `src/ops/reserve_contact.rs:238-251`:
```
        // What the reservation points the antenna at is resolved before anything is
        // stored: a program track is flown from an ephemeris of this account, so one
        // naming an ephemeris this account does not hold — or one of another form than
        // the member naming it requires — is refused rather than reserved, exactly as
        // `UpdateContact` refuses it for a later version of a contact.
        match guard.program_track_fault(input.tracking_overrides.as_ref()) {
```
while the satellite goes in unchecked — `src/ops/reserve_contact.rs:305-311`:
```
            // The satellite and the tracking overrides the contact is reserved with are
            // its FIRST version: `UpdateContact` appends further ones, and the contact
            // is flown with whichever is active.
            versions: vec![crate::state::ContactVersionRecord::initial(
                now,
                input.satellite_arn,
                input.tracking_overrides,
            )],
```
and the same for the second writer — `src/ops/update_contact.rs:269-274`:
```
        let version_id = contact.append_version(
            input.satellite_arn,
            input.tracking_overrides,
            input.client_token,
            crate::state::now_timestamp(),
        );
```

The crate rule that cannot see such a contact — `src/state.rs:4725-4742`:
```
        let flown_satellite_arn = if ephemeris.status == EPHEMERIS_ENABLED {
            ephemeris
                .satellite_id
                .as_deref()
                .and_then(|satellite_id| self.satellites.get(satellite_id))
                .map(|satellite| satellite.satellite_arn.clone())
        } else {
            None
        };
...
                    && (contact.program_tracks_ephemeris(ephemeris_id)
                        || flown_satellite_arn
                            .as_deref()
                            .is_some_and(|arn| contact.flies_satellite(arn)))
```
and the availability search, which resolves the ARN the reservation did not — `src/state.rs:5069-5074`:
```
            .filter(|ephemeris| {
                ephemeris
                    .satellite_id
                    .as_deref()
                    .and_then(|satellite_id| self.satellites.get(satellite_id))
                    .is_some_and(|satellite| satellite.satellite_arn == satellite_arn)
            })
```

The sibling handlers that DO refuse an unknown satellite — `src/ops/create_ephemeris.rs:199-211`:
```
        // An ephemeris applies to a satellite of this account, so a request naming one
        // this account is not on-boarded to references nothing: that is a missing
        // resource, not a bad value.
        if let Some(satellite_id) = input.satellite_id.as_deref() {
            if !guard.satellites.contains_key(satellite_id) {
                return Err(CreateEphemerisErrorModeled::ResourceNotFoundException(
```
(same shape in `src/ops/list_ephemerides.rs:214-227` and `src/ops/list_ground_stations.rs:203-220`), over the text the crate keeps for it — `src/state.rs:3631-3638`:
```
/// What this service answers when a `satelliteId` names no satellite this account is
/// on-boarded to.
///
/// Shared by every operation that takes one, so a create that references an unknown
/// satellite and a listing filtered on one answer the same text.
pub fn satellite_not_found_message(satellite_id: &str) -> String {
```
`ReserveContact` and `UpdateContact` both declare `ResourceNotFoundException (434)`, so the answer is available (`smithy_query(query="operation", operation="ReserveContact,UpdateContact")`).

Test coverage: `tests/all/contact_versions.rs:44-52` and `tests/all/contact_version_description.rs:91` build hand-written ARNs (`satellite_arn("11111111-1111-4111-8111-111111111111")`) for satellites no onboarding minted and only assert the round-trip; they state no reason for the reference going unresolved. By contrast `tests/all/available_contacts.rs:152-166` reserves with `account.satellite_arn`, the ARN of a genuinely on-boarded satellite.

## Steps to Reproduce

1. Seed one on-boarded satellite (`GROUNDSTATION_SATELLITES`) and `CreateEphemeris` an enabled az/el ephemeris for it.
2. `CreateConfig` + `CreateMissionProfile`, then `POST /contact` (ReserveContact) with `satelliteArn` = `arn:aws:groundstation:us-east-1:<account>:satellite/11111111-1111-4111-8111-111111111111` — a well-formed ARN for a satellite this account is not on-boarded to.
3. The call answers 200; `GET /contact/{contactId}` reports that `satelliteArn` back, while `GET /satellite/11111111-1111-4111-8111-111111111111` answers 434.
4. `DELETE /ephemeris/{ephemerisId}` now succeeds even though a non-terminal contact is flown against that satellite: `contacts_using_ephemeris` found no holder because the contact's ARN resolves to no stored satellite.
5. Compare with the same request carrying an `uplinkEchoConfig`/`programTrackSettings` reference that resolves to nothing: those are refused 434 before anything is stored.
