Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2bc7d6b091147721cb0b696f6323d28be421a007

# An AVAILABLE search resolves missionProfileArn but silently answers an empty page for a satelliteArn that names no satellite, against the rule the same handler states two lines earlier

_Filed: 2026-10-29T04:56:57Z_
_Operation: ListContacts | Class: validation_

> _Note: this issue was identified against the code base at commit `2bc7d6b091147721cb0b696f6323d28be421a007`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListContacts`' availability branch computes the offered windows from THREE references the handler itself has just made mandatory: `missionProfileArn`, `satelliteArn` and `groundStation`. All three are arguments of the computation (`guard.available_contact_windows(satellite_arn, ground_station, &profile, ...)`), yet only the mission profile is RESOLVED: an ARN naming no profile is refused with `ResourceNotFoundException`, while an ARN naming no satellite of this account — and a `groundStation` name this account does not know — produce a 200 with an empty `contactList`.

That splits one rule in two inside a single handler. The comment above the mission-profile lookup states the rule the handler is applying:

  "The mission profile is not a filter here: its pre- and post-pass durations and its minimum viable contact duration are part of what the windows are computed FROM, so an ARN naming no profile of this account names nothing to compute them for. That is a missing resource"

The satellite is literally the first argument of the window computation, so by that same rule it too "names nothing to compute them for" — but the code twelve lines later asserts the opposite and answers empty. The crate already owns the check: `satellite_reference_fault(Option<&str>)` takes exactly a satellite ARN and is called by both writers of a contact version (`ReserveContact`, `UpdateContact`); `knows_ground_station` is called by the two sibling listings bound on a ground station. Two sibling LISTINGS with a satellite filter also refuse rather than answer empty, for the stated reason that a caller cannot tell the two outcomes apart.

Effect: the first step of the documented discovery-then-reserve loop cannot distinguish "this pairing has no passes in that window" from "you typed a satellite ARN this account holds nothing under", while the very next call (`ReserveContact`) does refuse that same ARN with 434.

## Evidence

Both sides are in the SAME handler.

The rule, stated: services/service-com-amazonaws-groundstation-groundstation/src/ops/list_contacts.rs:413-425 —
  "// The mission profile is not a filter here: its pre- and post-pass durations and
   // its minimum viable contact duration are part of what the windows are computed
   // FROM, so an ARN naming no profile of this account names nothing to compute
   // them for. That is a missing resource, answered with the text this service
   // already answers for a mission profile a reservation could not find.
   let profile = guard
       .mission_profile_by_arn(input.mission_profile_arn.as_deref().unwrap_or_default())
       .cloned()
       .ok_or_else(|| {
           ListContactsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
               message: Some(crate::state::MISSION_PROFILE_NOT_FOUND.to_string()),
           })
       })?;"

The rule, broken, 4 lines later: services/service-com-amazonaws-groundstation-groundstation/src/ops/list_contacts.rs:426-432 —
  "// A satellite this account is not on-boarded to, and a station nothing was
   // supplied for, have no pointing data in force and so fly nothing: that is an
   // empty answer rather than a refusal.
   if let (Some(ground_station), Some(satellite_arn)) = (
       input.ground_station.as_deref(),
       input.satellite_arn.as_deref(),
   ) {"

…and the satellite is an argument of the computation, not a filter of it: list_contacts.rs:447-456 —
  "let offered = guard.available_contact_windows(
       satellite_arn,
       ground_station,
       &profile,"

The helper this path skips already exists and is called by two other handlers — services/service-com-amazonaws-groundstation-groundstation/src/state.rs:4650-4667 —
  "/// against a satellite the other writer would have refused.
   pub fn satellite_reference_fault(
       &self,
       satellite_arn: Option<&str>,
   ) -> Option<ResourceNotFoundException> {
       let arn = satellite_arn?;
       self.satellite_by_arn(arn)
           .is_none()
           .then(|| ResourceNotFoundException {
               message: Some(satellite_reference_not_found_message(arn)),
           })
   }"
called at services/service-com-amazonaws-groundstation-groundstation/src/ops/reserve_contact.rs:258 —
  "if let Some(error) = guard.satellite_reference_fault(input.satellite_arn.as_deref()) {"

Sibling listings refuse instead of answering empty, with the reason spelled out — services/service-com-amazonaws-groundstation-groundstation/src/ops/list_ephemerides.rs:214-227 —
  "// A satellite filter names a satellite of this account; one this account is not
   // on-boarded to references nothing, so the request is answered as the missing
   // resource it names rather than with the empty page it would otherwise select —
   // which a caller could not tell from \"no ephemeris applies to it\".
   if let Some(satellite_id) = input.satellite_id.as_deref() {
       if !guard.satellites.contains_key(satellite_id) {
           return Err(ListEphemeridesErrorModeled::ResourceNotFoundException("
and services/service-com-amazonaws-groundstation-groundstation/src/ops/list_ground_stations.rs:203-217 (same comment, same `ResourceNotFoundException`).

For the ground-station half, the same crate rule is applied by the two listings bound on a ground station — services/service-com-amazonaws-groundstation-groundstation/src/ops/list_ground_station_reservations.rs:258-277 ("A `groundStationId` naming no ground station this account knows references nothing, so the request is answered as the bad parameter it carries rather than with the empty page the lookup would otherwise select") and src/ops/list_antennas.rs:197-211 — while `ListContacts` never calls `knows_ground_station`.

Test coverage: no test pins the unknown-satellite answer for an AVAILABLE search. tests/all/scheduling_loop.rs:1619-1649 asserts the empty page only for an ON-BOARDED satellite ("this account holds no ephemeris here, so the satellite it is on-boarded to has no trajectory"), and tests/all/available_contacts.rs:777-794 likewise uses `account.satellite_arn`. The unknown-satellite refusal IS pinned for the sibling listing in tests/all/search_window_rule.rs:251 and tests/all/ephemeris_catalog.rs:994.

## Steps to Reproduce

1. Create a mission profile and note its `missionProfileArn`; onboard nothing (or onboard one satellite and keep its ARN aside).
2. POST /contacts with {"statusList":["AVAILABLE"], "startTime":<t>, "endTime":<t+86400>, "groundStation":"Ohio-1", "missionProfileArn":<the real ARN>, "satelliteArn":"arn:aws:groundstation:us-east-1:000000000000:satellite/00000000-0000-0000-0000-000000000000"}.
3. Observe HTTP 200 with `contactList: []` — the ARN that names no satellite of this account is indistinguishable from a pairing with no passes.
4. Repeat the same request with a `missionProfileArn` that names no profile: observe HTTP 434 `ResourceNotFoundException`, i.e. the handler resolves one computation argument and not the other.
5. Repeat with the real satellite ARN but `"groundStation":"Bahrain-1"` (a name this account does not know): observe 200/empty, while GET /groundstation/Bahrain-1/reservation and GET /groundstation/Bahrain-1/antenna both answer 431 `InvalidParameterException`.
6. POST /contact (ReserveContact) with the same bogus `satelliteArn`: observe 434, confirming the crate resolves the reference everywhere except in the listing that requires it.
