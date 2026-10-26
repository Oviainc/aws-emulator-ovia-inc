Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Validate UpdateLandingZone's requested version and manifest instead of deploying any pattern-shaped value

_Filed: 2026-10-24T07:41:10Z_
_Operation: UpdateLandingZone | Class: validation_

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateLandingZone accepts any `version` matching the model pattern and treats `manifest` as an entirely opaque document, so requests real AWS rejects with ValidationException answer 200, log a SUCCEEDED UPDATE operation, and then become the landing zone's reported DEPLOYED configuration.

Expected (real AWS): `version` must name a landing zone version the service offers (the 2.x/3.x line, e.g. up to 3.3) AND must not move the landing zone backwards — a landing zone can be updated to the same or a newer version only; `{"version":"9.9"}` or a downgrade from 3.3 to 2.0 is refused with ValidationException (400) and NOTHING is deployed. The `manifest` is a structured configuration document (governedRegions, organizationStructure, centralizedLogging, securityRoles, accessManagement); an update whose manifest is an empty object, a JSON scalar/array, or omits those keys is refused, and so is one that removes a region/OU the landing zone requires.

Actual (emulator): the handler resolves the landing zone, refuses an in-flight operation, and then parks `version`, `manifest` and `remediationTypes` as the pending configuration with no inspection of either value; `settle` deploys them when the operation becomes due, after which GetLandingZone reports e.g. version 9.9 (and `latestAvailableVersion` 9.9, because the latest version is recomputed as the numerically later of the deployed and catalog versions) or a landing zone whose manifest is `{}` / `"not-an-object"`. A downgrade is accepted the same way.

## Evidence

Smithy model: UpdateLandingZone (POST /update-landingzone) declares ValidationException (400) and ConflictException (409); `version` targets LandingZoneVersion (len 3..10, pattern ^\d+.\d+$) documented "for example, 3.0", i.e. the pattern bounds the shape of a version, not the set that exists; `manifest` targets `Manifest` (document) documented as "the manifest JSON file ... that describes your Amazon Web Services resources" with documented required parameters; `LandingZoneDetail$version` is "the landing zone's current deployed version" and `$latestAvailableVersion` "the latest available version of the landing zone" — both derived from the stored value. That Control Tower landing zones upgrade forward only, to published versions, is well-established service behavior; the emulator even carries its own catalog constant (CATALOG_LANDING_ZONE_VERSION = "3.3", src/state.rs).

Handler: services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/update_landing_zone.rs lines 181-250 — after the ResourceNotFound and ConflictException checks the handler writes `PendingLandingZoneConfiguration { version: input.version, manifest: ..., remediation_types: ... }` with no validation of either member and no comparison against the currently deployed version; src/state.rs:540-552 then deploys it and recomputes `latest_available_version` from it via `latest_available_version(deployed, CATALOG_LANDING_ZONE_VERSION)`.

Tests: tests/all/landing_zone_update.rs `an_update_deploys_its_configuration_when_the_clock_makes_the_operation_due` drives arbitrary versions/manifests and asserts they deploy; the only refusals covered are the front door's bounds, the 404 for an unknown landing zone and the 409 for an in-flight operation. No test asserts an unsupported version, a version downgrade or a semantically invalid manifest is refused. The backlog's CreateLandingZone validation issue covers the CREATE path only; there is no UpdateLandingZone validation issue.

## Steps to Reproduce

1. POST /create-landingzone with {"version":"3.3","manifest":{"governedRegions":["us-west-2"], ...}} and wait past the settle span so the landing zone is ACTIVE.
2. POST /update-landingzone with {"landingZoneIdentifier":"<arn>","version":"9.9"} — the emulator answers 200 with an operationIdentifier; AWS refuses an unsupported landing zone version.
3. Wait past the settle span and POST /get-landingzone — the emulator reports version 9.9 and latestAvailableVersion 9.9, a state AWS would never produce.
4. On a landing zone at 3.3, POST /update-landingzone with {"version":"2.0"} — emulator answers 200 and deploys the downgrade; AWS refuses because a landing zone can only be updated forward.
5. POST /update-landingzone with {"version":"3.3","manifest":{}} (or "manifest":"not-an-object") — emulator answers 200 and, once due, GetLandingZone reports the empty/scalar manifest as the deployed configuration; AWS refuses a manifest missing its required parameters.
