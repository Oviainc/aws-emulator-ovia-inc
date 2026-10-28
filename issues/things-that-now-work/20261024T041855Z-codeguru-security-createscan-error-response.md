Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0ca8c03330eb2e28bf9c83ef6b425c1dfc77ebe9

# Code artifacts are deleted 15 minutes after CreateUploadUrl, so CreateScan 404s on a codeArtifactId AWS still accepts

_Filed: 2026-10-24T04:18:55Z_
_Operation: CreateScan | Class: error-response_

> _Note: this issue was identified against the code base at commit `0ca8c03330eb2e28bf9c83ef6b425c1dfc77ebe9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `ResourceId$codeArtifactId` is documented only as "The identifier for the uploaded code resource. Returned by CreateUploadUrl" — nothing in the model or the service documentation gives a code artifact a lifetime. In AWS the time limit attached to CreateUploadUrl is the *signature* validity of the pre-signed S3 PUT URL; once the archive has been uploaded, the artifact (and its codeArtifactId) remains scannable, so a client that uploads its source and then calls CreateScan later — after a long build, a CI stage boundary, or a manual pause — gets HTTP 200 and a new scan run.

Actual: the emulator gives every minted artifact a hard 15-minute TTL and *deletes the artifact record* when it comes due (`UPLOAD_ARTIFACT_TTL_MILLIS` + `Store::expire_due_artifacts`, called at the top of both CreateUploadUrl and CreateScan). CreateScan resolves `resourceId.codeArtifactId` against that record, so once the TTL has elapsed the call is refused with ResourceNotFoundException (HTTP 404, resourceType "codeArtifact", message "no code artifact <id>: a codeArtifactId must be one returned by CreateUploadUrl") — the same answer the emulator gives for an id it never minted. The emulator thus conflates "the pre-signed URL can no longer be used to upload" with "the artifact no longer exists", and turns a documented success path into a 404. The TTL also caps how long the quota slot is held, so the store bound it exists for could be met with a quota/eviction policy that does not change the API's answer for a valid id.

## Evidence

Smithy model: `CreateScanRequest$resourceId` -> `ResourceId$codeArtifactId` is documented as "Returned by CreateUploadUrl" with no expiry, and `CreateUploadUrlResponse$codeArtifactId`/`s3Url` document only that the URL is pre-signed. Handler/state: services/.../src/state.rs lines 99-103 define `UPLOAD_ARTIFACT_TTL_MILLIS = 15 * 60 * 1_000` and its own doc comment admits the lifetime is "CHOSEN, not documented"; `Store::expire_due_artifacts` (state.rs lines 353-360) removes the record from `upload_artifacts`; src/ops/create_scan.rs lines 194-196 runs it, and lines 240-253 answer ResourceNotFoundException when the record is gone. Test coverage: tests/all/scan_lifecycle.rs lines 1089-1113 (`unknown_names_and_ids_are_the_modelled_refusals_not_a_501`) seeds an artifact with `expires_at_millis = now - 1` and ASSERTS the 404 plus "the expired artifact must have been released from the store" — i.e. the invented expiry is currently locked in by a test, which must change with the behavior.

## Steps to Reproduce

1. POST /uploadUrl with {"scanName":"demo"} and keep the returned codeArtifactId
2. Let more than 15 minutes of wall-clock pass (or seed an artifact whose expires_at_millis is in the past, as tests/all/scan_lifecycle.rs:1089 does)
3. POST /scans with {"scanName":"demo","resourceId":{"codeArtifactId":"<that id>"}}
4. Observe HTTP 404 ResourceNotFoundException (resourceType "codeArtifact"); against AWS the same call succeeds with 200 and a new runId in scanState InProgress
5. GET /scans/demo afterwards -> 404, because no run was ever recorded
