Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# PutSigningProfile accepts a platformId the service publishes no signing platform for

_Filed: 2026-10-24T15:57:39Z_
_Operation: PutSigningProfile | Class: validation_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A signing profile is a template bound to one of the signing platforms the service publishes; `platformId` is the only @required member besides `profileName`, and the platform is what supplies the signing configuration (allowed/default hash + encryption algorithms, image format, max payload size) a later signing job runs under. AWS Signer publishes a FIXED catalog of platforms (`ListSigningPlatforms` / `GetSigningPlatform`), so a `platformId` outside that catalog names no platform at all and a profile cannot be created on it: AWS refuses the create (4xx — the operation models both `ValidationException`/400 and `ResourceNotFoundException`/404, and the platform is the only resource `PutSigningProfile` refers to besides the profile itself).

The emulator never looks the platform id up. `src/ops/put_signing_profile.rs` only asks `crate::state::platform_requires_signing_material(&input.platform_id)` (which answers `true` for an unknown id) and `crate::state::platform_display_name(&input.platform_id)` (which answers `None`). Consequence: `PutSigningProfile` with `platformId: "PrivatePlatform-SHA256-ECDSA"` (or any other unpublished id) plus a `signingMaterial` answers 200 and PERSISTS an Active profile whose `platformId` is a platform the very same emulator reports as non-existent — `GetSigningPlatform /signing-platforms/PrivatePlatform-SHA256-ECDSA` answers 404 `ResourceNotFoundException` / code `PlatformNotFound` (src/ops/get_signing_platform.rs). The emulator therefore contradicts itself: a profile exists on a platform the service says does not exist, `GetSigningProfile` reports it with a null `platformDisplayName`, and `StartSigningJob` will happily start jobs on it.

Second symptom of the same class and same cause (no validation against the platform catalog): `overrides.signingConfiguration` is stored verbatim without being checked against the platform's published `signingConfiguration.hashAlgorithmOptions.allowedValues` / `encryptionAlgorithmOptions.allowedValues`, so a profile can be created asking for an algorithm the platform does not sign under (e.g. `SHA1`/`RSA` overrides on `AWSLambda-SHA384-ECDSA`, whose allowed values are SHA384/ECDSA only).

Expected: a create naming a platform outside the published catalog is refused with a modeled 4xx naming the platform (nothing persisted), and an override outside the platform's `allowedValues` is refused with `ValidationException`/400. Actual: both are accepted with 200 and stored.

Distinct from the two open PutSigningProfile issues: 20261023T031331Z (Cause 1 = a SECOND put on an existing name must be refused as a duplicate) and 20261023T002214Z (Cause 1 = a valid put that omits `signingMaterial` on the Notation platform must be ACCEPTED). Neither concerns the platform id's existence or the override values.

## Evidence

Smithy model: `PutSigningProfileRequest$platformId` (@required, "The ID of the signing platform to be created"); the operation models `ValidationException`/400 and `ResourceNotFoundException`/404. `GetSigningPlatform`/`ListSigningPlatforms` define the platform catalog as a published, fixed set, and `SigningConfiguration` publishes `allowedValues`/`defaultValue` per platform, which is what an override must fall inside.

Handler: services/service-com-amazonaws-signer-wallabyservice/src/ops/put_signing_profile.rs — the only platform-related checks are `platform_requires_signing_material(&input.platform_id)` (src/state.rs: `signing_platform(platform_id).is_none_or(|e| e.requires_signing_material)`, i.e. an unknown platform is simply treated as 'needs a certificate') and `platform_display_name(&input.platform_id)` (returns `None` for an unknown id). `input.overrides` is moved into the record untouched. There is no `signing_platform(...)` lookup in the handler (grep for `signing_platform(` in the crate hits only src/state.rs and src/ops/get_signing_platform.rs).

Test coverage: no test asserts a refusal — the opposite is pinned. tests/all/signing_profile_lifecycle.rs case "Private_Platform_Profile_9" (platformId `PrivatePlatform-SHA256-ECDSA`) and tests/all/profile_revocation_and_identity.rs case `Private_Platform_Profile` assert that an unpublished-platform profile is stored and readable, while tests/all/signing_platform_catalog.rs `an_unpublished_platform_id_is_refused_as_not_found` asserts that the SAME id is a 404 from `GetSigningPlatform` — the self-contradiction is asserted on both sides.

## Steps to Reproduce

1. GET /signing-platforms/PrivatePlatform-SHA256-ECDSA — observe 404 ResourceNotFoundException (code PlatformNotFound): the service publishes no such platform.
2. PUT /signing-profiles/Private_Platform_Profile with body {"platformId": "PrivatePlatform-SHA256-ECDSA", "signingMaterial": {"certificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/abc"}} — observe 200 with arn/profileVersion/profileVersionArn instead of a 4xx naming the unknown platform.
3. GET /signing-profiles/Private_Platform_Profile — observe 200, status Active, platformId echoed and platformDisplayName null: a profile persisted on a platform the service reports does not exist.
4. POST /signing-jobs against that profile — observe the job starts on the non-existent platform.
5. Override check: PUT /signing-profiles/Bad_Override_Profile with {"platformId": "AWSLambda-SHA384-ECDSA", "overrides": {"signingConfiguration": {"hashAlgorithm": "SHA1", "encryptionAlgorithm": "RSA"}}} — observe 200 and the override stored, although GET /signing-platforms/AWSLambda-SHA384-ECDSA publishes allowedValues SHA384 / ECDSA only.
