Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 71cff0481dbaf9b41262283aed7350a64aff149f

# A stored ephemeris never becomes its satellite's currentEphemeris, so GetSatellite/ListSatellites always report none

_Filed: 2026-10-26T06:53:03Z_
_Operation: CreateEphemeris | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `71cff0481dbaf9b41262283aed7350a64aff149f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEphemeris` stores an ephemeris against the `satelliteId` the request names (after proving that satellite is on-boarded), and the ephemeris reaches `ENABLED` once its validation settles. Nothing ever records that ephemeris on the satellite: `Satellite::current_ephemeris` is written in exactly two places in this crate, both of which set it to `None` (the onboarding at `state.rs:2787` and the delete cleanup at `state.rs:3510`). So `GetSatellite` (`get_satellite.rs:185`) and `ListSatellites` (`list_satellites.rs:172`) report `currentEphemeris` absent for a satellite that holds an enabled, customer-provided ephemeris, forever.

The crate contradicts itself twice over this. The state field's own doc says the onboarding carries none only *until an ephemeris is stored* — and `CreateEphemeris` stores one. And `remove_ephemeris` goes out of its way to clear the satellite's reference so it is "not left dangling"; since no path ever sets the reference, that loop can never fire — it is unreachable code maintaining an invariant the create half never establishes. The crate's own tests have to poke `stored.current_ephemeris = Some(...)` directly into the store because no operation populates it.

## Evidence

The state field states the rule the create breaks — `src/state.rs:2254-2256`:
```
    /// The ephemeris currently used to compute this satellite's trajectory, once one is
    /// known for it. An onboarding carries none until an ephemeris is stored.
    pub current_ephemeris: Option<EphemerisMetaData>,
```
The delete half maintains that reference — `src/state.rs:3501-3512`:
```
        // A satellite reports the ephemeris its trajectory is computed from; a deleted one
        // is no longer that, so the reference goes with it rather than being left dangling.
        for satellite in self.satellites.values_mut() {
            if satellite
                .current_ephemeris
                .as_ref()
                .and_then(|metadata| metadata.ephemeris_id.as_deref())
                == Some(ephemeris_id)
            {
                satellite.current_ephemeris = None;
```
The create half never establishes it — `src/ops/create_ephemeris.rs:238-283` builds and inserts the record (`satellite_id: input.satellite_id,` at :241, `guard.ephemerides.insert(ephemeris_id.clone(), stored);` at :283) and the only other access to `guard.satellites` in the handler is the existence check at :202-211. A grep over `src/` for `current_ephemeris` yields only the declaration (`state.rs:2256`), `current_ephemeris: None` (`state.rs:2787`), the clearing loop (`state.rs:3505-3510`) and the two readers (`get_satellite.rs:185`, `list_satellites.rs:172`) — no writer of a value.

The model has somewhere to put it: `smithy_query(query="shape", shape="EphemerisMetaData")` — `ephemerisId`: "UUID of a customer-provided ephemeris. This field is not populated for default ephemerides from Space Track.", `source` (required) over `EphemerisSource` = `CUSTOMER_PROVIDED | SPACE_TRACK`.

Test coverage shows nothing pins the gap: `tests/all/satellite_reads.rs:182-191` and `tests/all/satellite_catalog.rs:154` have to write `stored.current_ephemeris = Some(ephemeris)` into the store by hand to exercise the member, and `tests/all/satellite_catalog.rs:540-545` only asserts that a *freshly parsed* onboarding "knows no ephemeris yet" — no test creates an ephemeris and then reads the satellite.

## Steps to Reproduce

1. Start the emulator with `GROUNDSTATION_SATELLITES=25994=Ohio 1` so the account holds one on-boarded satellite; read its `satelliteId` with `ListSatellites`.
2. `CreateEphemeris` with `{"name":"tle one","satelliteId":"<satelliteId>","enabled":true,"ephemeris":{"tle":{"tleData":[...]}}}` → 200 with an `ephemerisId`.
3. Wait past `EPHEMERIS_VALIDATION_SETTLES_AFTER` and `DescribeEphemeris` the new id → `status: ENABLED`.
4. `GetSatellite` (and `ListSatellites`) for that `satelliteId` → `currentEphemeris` is absent, although the account holds an enabled customer-provided ephemeris for exactly that satellite.
5. `DeleteEphemeris` the id → the cleanup loop at state.rs:3505-3510 finds nothing to clear, because nothing ever set the reference.
