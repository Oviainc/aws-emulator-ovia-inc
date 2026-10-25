Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f5120b87affb905e7d968cf57a1e02e568fab681

# The target CreateUploadUrl publishes refuses any code archive over 8 MiB (and 64 MiB per account), so realistic source uploads cannot be scanned

_Filed: 2026-10-24T15:05:06Z_
_Operation: CreateUploadUrl | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f5120b87affb905e7d968cf57a1e02e568fab681`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `CreateUploadUrlResponse$s3Url` is documented as a pre-signed URL a caller uploads "the code file you want to scan ... using any HTTP client" to. Neither the Smithy model nor the service documentation attaches a size limit to that upload; the published URL is a pre-signed S3 PUT, which accepts a single object up to 5 GB, and CodeGuru Security scans whole zipped source trees, which routinely exceed 8 MiB. A client that mints an upload URL, PUTs a ~50 MB repository archive to it and then calls CreateScan should get HTTP 200 on the upload and a scan run started on that resource.

Actual: the emulator's upload plane enforces two self-chosen ceilings and refuses the upload outright instead of accepting it:
* an archive larger than `UPLOAD_CONTENT_MAX_BYTES` (8 MiB) is answered 400 `EntityTooLarge` ("This upload target retains at most 8388608 bytes") and nothing is stored;
* an upload that would push the account's retained total past `UPLOAD_CONTENT_TOTAL_MAX_BYTES` (64 MiB) is answered 507 `InsufficientStorage` — a status code AWS never returns for an upload to a pre-signed URL.

The consequences are observable through the modelled API, not just on the data plane: because the artifact keeps `upload: None`, a later `CreateScan` on that `codeArtifactId` records `resource_uploaded: false`, and the run settles to `Failed` with the "nothing was uploaded to the target CreateUploadUrl minted for this code artifact" reason. So for any realistically sized code artifact, the emulator's whole documented workflow (CreateUploadUrl -> upload -> CreateScan -> GetScan) ends in a failed scan that claims the caller never uploaded anything, where AWS scans the resource.

Note the heap bound these constants exist for does not require refusing the request: this emulator performs no analysis, and the only thing any handler ever asks of the stored bytes is `artifact.upload.is_some()` (CreateScan) — so the upload could be accepted and accounted for (length + digest, or a capped retained prefix) without retaining the whole archive, keeping the store bounded while answering 200 as AWS does.

## Evidence

Spec: `CreateUploadUrlResponse$s3Url` / `$requestHeaders` (smithy_query query=operation operation=CreateUploadUrl, and query=doc shape=CreateUploadUrlResponse) document a pre-signed URL and the headers required to upload to it, with NO size limit anywhere in the model; well-established S3 behavior is that a single PUT to a pre-signed URL accepts up to 5 GB.

Emulator: `src/state.rs` line ~188 `pub const UPLOAD_CONTENT_MAX_BYTES: usize = 8 * 1024 * 1024;` and line ~193 `pub const UPLOAD_CONTENT_TOTAL_MAX_BYTES: usize = 64 * 1024 * 1024;` — both doc comments admit they are "A STORE BOUND chosen by this emulator, not a published AWS limit". Enforcement: `src/service/upload_plane.rs` lines 89-91 (`if body.len() > UPLOAD_CONTENT_MAX_BYTES { return too_large(); }`), lines 120-131 (`too_large()` -> 400 EntityTooLarge) and lines 107-116 (`UploadOutcome::StoreFull` -> 507 InsufficientStorage); `Store::receive_upload` (src/state.rs lines ~450-495) re-checks both ceilings. Downstream effect: `src/ops/create_scan.rs` lines ~352-357 set `resource_uploaded: artifact.upload.is_some()`, and `Store::settle_due_runs` (src/state.rs lines ~405-410) then stamps the run `Failed` with `SCAN_RESOURCE_NOT_UPLOADED_MESSAGE`.

Tests: `tests/all/upload_target.rs::an_upload_past_what_this_emulator_retains_is_refused_and_stores_nothing` (line ~611) asserts the current refusal, so the invented ceiling is locked in by a test and that test must change with the behavior; no test asserts that an archive larger than 8 MiB is accepted.

## Steps to Reproduce

1. POST /uploadUrl with {"scanName": "big-repo"} and note s3Url + codeArtifactId + requestHeaders.
2. PUT a ~16 MB body (any bytes) to the returned s3Url with the returned requestHeaders.
3. Observe HTTP 400 with {"code": "EntityTooLarge"} and nothing stored. Against AWS the pre-signed S3 PUT returns 200 for an archive of this size (and far larger).
4. POST /scans with {"scanName": "big-repo", "resourceId": {"codeArtifactId": "<id>"}} -> 200, scanState InProgress.
5. GET /scans/big-repo after the run settles -> scanState Failed with errorMessage "...nothing was uploaded to the target CreateUploadUrl minted for this code artifact...", even though the caller did upload the archive.
6. Repeat the mint+upload cycle with 8 MiB archives until the account has retained 64 MiB: the next upload is answered 507 InsufficientStorage, a status AWS never returns for an upload to a pre-signed URL.
