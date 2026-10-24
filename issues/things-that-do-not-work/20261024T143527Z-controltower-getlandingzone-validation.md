Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# Refuse a landingZoneIdentifier that is not a landing zone ARN with ValidationException instead of resolving a bare resource id

_Filed: 2026-10-24T14:35:27Z_
_Operation: GetLandingZone | Class: validation_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`landingZoneIdentifier` is modelled as a plain `String` (no pattern), so no front-door check applies and the handler's resolver decides what is acceptable. The emulator's resolver accepts the full ARN AND the bare resource id the ARN ends in, and answers `ResourceNotFoundException` (404) for anything else.

Expected (real AWS): the landing zone identifier is the landing zone ARN (`arn:aws:controltower:<region>:<account>:landingzone/<id>`) — the value `ListLandingZones`/`CreateLandingZone` hand back. A value that is not an ARN (a bare resource id such as `4B3E6KDSOOU1BV4V`, or `not-an-arn`) is refused with `ValidationException` (400, "The input does not satisfy the constraints specified by an Amazon Web Services service"), not reported as a successful read and not reported as a missing resource.

Actual (emulator): a bare resource id resolves to the one stored landing zone and answers 200 with the full `LandingZoneDetail` — a read AWS would reject — while a malformed identifier answers 404 instead of 400, so a client cannot distinguish "bad input" from "no landing zone". The same resolver backs `UpdateLandingZone` and `DeleteLandingZone`, so a bare id also drives a mutation.

## Evidence

Smithy model: GetLandingZone (POST /get-landingzone, @readonly) takes `landingZoneIdentifier` (required, plain String with no length/pattern constraint) and declares BOTH ValidationException (400) and ResourceNotFoundException (404); `LandingZoneSummary$arn`/`CreateLandingZoneOutput$arn` are typed `Arn` (len 20..2048, pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$), i.e. the only identifier the service ever hands a caller for a landing zone is its ARN. Because the model puts no pattern on the input member, the shape check is the handler's job — which is why the emulator gets to choose, and currently chooses a looser rule than AWS.

Handler: services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/get_landing_zone.rs lines 164-177 calls `store.resolve_landing_zone_arn(&input.landing_zone_identifier)` and maps a miss to ResourceNotFoundException; src/state.rs lines 923-936 — `resolve_landing_zone_arn` matches the map key OR `arn.split('/').next_back() == Some(identifier)`, i.e. the bare resource id. No ARN-shape check exists anywhere on this path.

Tests: tests/all/landing_zone_lifecycle.rs `a_landing_zone_resolves_by_arn_or_by_bare_id` (around line 878) pins the CURRENT lenient behavior, and `an_identifier_no_call_returned_is_not_found` (around line 781) pins the 404 for arbitrary strings; no test asserts that a non-ARN identifier is refused with ValidationException. The backlog has no GetLandingZone issue.

## Steps to Reproduce

1. POST /create-landingzone with {"version":"3.3","manifest":{...}} and note the returned arn, e.g. arn:aws:controltower:us-west-2:123456789012:landingzone/<id>.
2. POST /get-landingzone with {"landingZoneIdentifier":"<id>"} (just the resource segment, no ARN) — the emulator answers 200 with the full landing zone detail; AWS refuses the request with ValidationException because the identifier is not a landing zone ARN.
3. POST /get-landingzone with {"landingZoneIdentifier":"not-an-arn"} — the emulator answers 404 ResourceNotFoundException; AWS answers 400 ValidationException for an identifier that is not an ARN at all.
4. Repeat step 2 against POST /delete-landingzone or POST /update-landingzone to see the bare id drive a mutation AWS would have rejected.
