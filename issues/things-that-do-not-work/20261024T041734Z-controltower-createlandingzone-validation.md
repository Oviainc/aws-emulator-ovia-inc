Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6395e76ed9f748dc97d54e4052a4e63524456c87

# Validate the CreateLandingZone manifest contents and the requested landing zone version

_Filed: 2026-10-24T04:17:33Z_
_Operation: CreateLandingZone | Class: validation_

> _Note: this issue was identified against the code base at commit `6395e76ed9f748dc97d54e4052a4e63524456c87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateLandingZone treats the `manifest` document as entirely opaque and accepts any `version` matching the model pattern, so requests real AWS rejects with ValidationException succeed with 200 and mint a landing zone.

Expected (real AWS): the manifest is a configuration document with required keys — `governedRegions`, `organizationStructure` (security/sandbox OU names), `centralizedLogging` (accountId, configurations, enabled), `securityRoles` (accountId), `accessManagement` (enabled). A create whose manifest is an empty object, is a JSON scalar/array rather than an object, or omits those keys is refused with a 4xx validation failure and NO landing zone is created. Likewise, `version` must name a landing zone version the service actually offers (currently the 2.x/3.x line, e.g. up to 3.3); an unsupported value such as "9.9" is refused.

Actual (emulator): the handler only checks that `manifest` is PRESENT (src/ops/create_landing_zone.rs:174-183) and then stores the document verbatim; any `^\d+.\d+$` string is accepted as `version` and even becomes the reported `latestAvailableVersion` when it exceeds the catalog version (src/state.rs:455-462). So `{"version":"9.9","manifest":{}}` answers 200, persists a landing zone, and GetLandingZone reports version 9.9 / latestAvailableVersion 9.9 — state AWS would never produce.

## Evidence

Smithy model: CreateLandingZoneInput.manifest targets `Manifest` (document) and is documented as "The manifest JSON file is a text file that describes your Amazon Web Services resources. For examples, review Launch your landing zone" — i.e. a structured configuration file with documented required parameters, not an arbitrary blob; CreateLandingZone binds ValidationException(400). `version` targets LandingZoneVersion (len 3..10, pattern ^\d+.\d+$) whose doc gives "for example, 3.0", and the emulator's own catalog constant (CATALOG_LANDING_ZONE_VERSION = "3.3", src/state.rs:60) shows which versions exist.

Handler: services/.../src/ops/create_landing_zone.rs — the only input check is the presence of `manifest`; there is no inspection of its members and no check of `version` against any supported set.

Tests: services/.../tests/all/landing_zone_lifecycle.rs `any_manifest_document_round_trips_unchanged` deliberately asserts the manifest is opaque, including an "empty object" row, and `latest_available_version_is_the_numerically_later_of_deployed_and_catalog` asserts a version ABOVE the catalog is accepted — so the suite currently encodes the lenient behavior; nothing asserts a semantically invalid manifest or an unsupported version is refused.

## Steps to Reproduce

1. POST /create-landingzone with {"version":"3.3","manifest":{}} — emulator answers 200 with an arn and operationIdentifier; AWS refuses the request because the manifest declares none of governedRegions/organizationStructure/centralizedLogging/securityRoles/accessManagement.
2. POST /get-landingzone with that arn — the emulator reports the empty manifest back, so invalid configuration is now persisted state.
3. On a fresh emulator POST /create-landingzone with {"version":"9.9","manifest":{"governedRegions":["us-west-2"]}} — emulator answers 200 and GetLandingZone reports version 9.9 with latestAvailableVersion 9.9, where AWS refuses an unsupported landing zone version.
4. On a fresh emulator POST /create-landingzone with {"version":"3.3","manifest":"not-an-object"} — emulator accepts the scalar document.
