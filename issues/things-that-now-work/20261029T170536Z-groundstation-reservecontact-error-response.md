Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 097d82b3cbc72c7bfc910d8eaa03c5f730d6b648

# ReserveContact answers the store-bound account limit BEFORE the request's own reference and placement gates, against the comment on that very check and the order both sibling creates use

_Filed: 2026-10-29T17:05:36Z_
_Operation: ReserveContact | Class: error-response_

> _Note: this issue was identified against the code base at commit `097d82b3cbc72c7bfc910d8eaa03c5f730d6b648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every create in this crate refuses a request that would take a store past `crate::state::MAX_RECORDS_PER_STORE` with `ResourceLimitExceededException` (HTTP 429), and each one carries the same comment saying the bound is judged LAST, after the request itself, "so what the caller hears is what is wrong with the request".

`CreateConfig` and `CreateMissionProfile` do exactly that: their reference gates run first and the bound last. `ReserveContact` carries the same sentence on its bound check, but places that check BEFORE three gates that judge the request itself — the program-track reference (`program_track_fault`), the satellite reference (`satellite_reference_fault`) and the antenna placement (`free_antenna`).

So on a store already holding `MAX_RECORDS_PER_STORE` contacts, a `ReserveContact` whose `trackingOverrides` name an `ephemerisId` this account holds no ephemeris under — a request the service refuses outright with `ResourceNotFoundException` (HTTP 434) on any other store — is instead answered 429 `ResourceLimitExceededException`, i.e. "retry later / raise your limit" for a request that can never succeed. That is the precise inversion the comment above the check rules out and that the two sibling creates avoid by ordering the bound last.

## Evidence

The check and the claim it makes, in `services/service-com-amazonaws-groundstation-groundstation/src/ops/reserve_contact.rs:228-243`:

        // The store this emulator keeps is bounded, so a caller cannot grow it without
        // limit; past the bound a reservation is refused the way the service refuses one
        // that runs past an account limit. Checked after the request itself, so what the
        // caller hears is what is wrong with the request.
        if guard.contacts.len() >= crate::state::MAX_RECORDS_PER_STORE {
            return Err(ReserveContactErrorModeled::ResourceLimitExceededException(

but three gates on the request itself come AFTER it — `reserve_contact.rs:250`, `:265` and `:279`:

        match guard.program_track_fault(input.tracking_overrides.as_ref()) {
...
        if let Some(error) = guard.satellite_reference_fault(input.satellite_arn.as_deref()) {
...
        let Some(antenna_name) = guard.free_antenna(

The sibling creates of THIS crate state and apply the opposite order.

`src/ops/create_mission_profile.rs:249-253` — the bound AFTER the references:

        // The store this emulator keeps is bounded, so a caller cannot grow it without
        // limit; past the bound a create is refused the way the service refuses one that
        // runs past an account limit. Checked after the references above, so what the
        // caller hears is what is wrong with the request.
        if guard.mission_profiles.len() >= crate::state::MAX_RECORDS_PER_STORE {

(its reference gates are at `create_mission_profile.rs:213-247`).

`src/ops/create_config.rs:217-220` — the reference gate explicitly placed before the bound:

        // every write of a config is held to, so it is answered by `crate::state` and only
        // lifted into this operation's error enum here. Checked before the store bound
        // below, so what the caller hears is what is wrong with the request.
        match guard.uplink_echo_reference_fault(&stored.config_data) {

with the bound at `create_config.rs:233`.

`src/ops/register_agent.rs:206-210` carries the same sentence and also orders its request checks (the IPv4 parse at `register_agent.rs:174-193`) before the bound.

Test coverage: only `CreateConfig`'s bound is exercised — `tests/all/scheduling_loop.rs:1878-1918` fills the config store to `MAX_RECORDS_PER_STORE` and asserts 429/`ResourceLimitExceededException` on the next create. No test fills the contact store, and no test or comment states a reason for `ReserveContact` ordering the bound differently from its siblings.

## Steps to Reproduce

1. Create a trackingConfig and a mission profile, and reserve MAX_RECORDS_PER_STORE (1,000) contacts so state.contacts is at its bound (each in a window of its own, as tests/all/scheduling_loop.rs:1881 does for configs).
2. POST /contact (ReserveContact) with a valid window and that missionProfileArn, but trackingOverrides={"programTrackSettings":{"azEl":{"ephemerisId":"00000000-0000-0000-0000-000000000000"}}} — a well-formed Uuid naming no ephemeris.
3. Expected (per the comment on the bound check at reserve_contact.rs:228-231 and the order create_config.rs:217-219 / create_mission_profile.rs:249-252 state): HTTP 434 ResourceNotFoundException from crate::state::program_track_fault, because what is wrong is the request.
4. Actual: HTTP 429 ResourceLimitExceededException, "this account already holds the maximum of 1000 contacts" — the bound is judged first.
5. Same with satelliteArn set to an ARN naming no satellite of the account (expected 434 from satellite_reference_fault, reserve_contact.rs:265) and with a window every antenna at the ground station is already held for (expected 431 from free_antenna, reserve_contact.rs:279).
