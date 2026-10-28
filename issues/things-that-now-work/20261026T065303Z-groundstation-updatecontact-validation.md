Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 71cff0481dbaf9b41262283aed7350a64aff149f

# A contact version stores a programTrackSettings ephemerisId that names no ephemeris, on the strength of a comment saying CreateEphemeris is still a scaffold

_Filed: 2026-10-26T06:53:03Z_
_Operation: UpdateContact | Class: validation_

> _Note: this issue was identified against the code base at commit `71cff0481dbaf9b41262283aed7350a64aff149f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateContact` appends a version carrying `trackingOverrides.programTrackSettings.{azEl|oem|tle}.ephemerisId` exactly as given, resolving nothing. The handler says why: "no ephemeris can exist in this store while `CreateEphemeris` is a scaffold (its own issue)". That premise is no longer true — `CreateEphemeris` is implemented and inserts into `guard.ephemerides` — so the one reason the reference was left unresolved has gone, and the contact can now be pointed at an `ephemerisId` this account does not hold (or at an ephemeris of the wrong form: an `azEl` program track naming a TLE ephemeris).

Every other ARN/id-valued reference in this crate is resolved before it is stored, which is what makes this one the odd path: `CreateConfig`/`UpdateConfig` refuse an `uplinkEchoConfig` whose `antennaUplinkConfigArn` resolves to nothing (`ResourceNotFoundException`) or to a config of another type (`InvalidParameterException`); `CreateMissionProfile`/`UpdateMissionProfile` refuse config ARNs that do not resolve; `CreateEphemeris` and `ListEphemerides` refuse a `satelliteId` this account is not on-boarded to. `ReserveContact` takes the same overrides for a contact's first version by the same unresolved route (`reserve_contact.rs:292-296`), so the fix belongs to both writers of a contact version.

## Evidence

The handler's own comment, now resting on a false premise — `src/ops/update_contact.rs:252-256`:
```
        // An `ephemerisId` inside `trackingOverrides` is stored as given rather than
        // resolved: no ephemeris can exist in this store while `CreateEphemeris` is a
        // scaffold (its own issue), which is also how `ReserveContact` takes one.
        let version_id = contact.append_version(
            input.satellite_arn,
```
The premise is false: `src/ops/create_ephemeris.rs:283` — `guard.ephemerides.insert(ephemeris_id.clone(), stored);` (the handler is implemented; its scaffold issue `20261023T235316Z-groundstation-createephemeris-stub.md` is fixed).

The sibling rule this path skips — `src/ops/update_config.rs:232-258`:
```
            match guard.typed_reference_fault(&echo.antenna_uplink_config_arn, ECHOED_TYPE) {
                None => {}
                Some(crate::state::TypedReferenceFault::Absent) => {
                    return Err(UpdateConfigErrorModeled::ResourceNotFoundException(
```
and `src/ops/create_ephemeris.rs:199-211`:
```
        // An ephemeris applies to a satellite of this account, so a request naming one
        // this account is not on-boarded to references nothing: that is a missing
        // resource, not a bad value.
        if let Some(satellite_id) = input.satellite_id.as_deref() {
            if !guard.satellites.contains_key(satellite_id) {
                return Err(CreateEphemerisErrorModeled::ResourceNotFoundException(
```
What the member is: `smithy_query(query="shape", shape="AzElProgramTrackSettings")` — `ephemerisId` is `@required`, the structure carries `references: [{resource: com.amazonaws.groundstation#Ephemeris}]`, doc "Unique identifier of the azimuth elevation ephemeris." (`OemProgramTrackSettings`/`TleProgramTrackSettings` likewise). `UpdateContact` declares `ResourceNotFoundException (434)` and `InvalidParameterException (431)`, so both answers are available.

Test coverage: `tests/all/contact_versions.rs:158-190` and `tests/all/contact_version_description.rs:419-432` drive updates with hand-written ids such as `"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"` and assert the stored overrides round-trip — they exercise the round-trip, and state no reason for the reference going unresolved.

## Steps to Reproduce

1. `CreateMissionProfile` + `ReserveContact` to get a contact in `SCHEDULING`/`SCHEDULED`.
2. `POST /contact/{contactId}/versions` with `{"trackingOverrides":{"programTrackSettings":{"azEl":{"ephemerisId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"}}}}` — an id no `CreateEphemeris` ever minted.
3. The call answers 200 with a new `versionId`, and `DescribeContactVersion` reports the dangling `ephemerisId` back.
4. Compare with `CreateConfig` carrying an `uplinkEchoConfig.antennaUplinkConfigArn` nothing resolves: that reference is refused 434 before anything is stored.
