Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the ssm-guiconnect emulator

_Filed: 2026-10-21T21:57:40Z_
_Operation: service-com-amazonaws-ssmguiconnect-ssmguiconnect | Class: missing_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The whole service is a fresh scaffold: `src/ops/update_connection_recording_preferences.rs` and `src/ops/delete_connection_recording_preferences.rs` both carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and return 501 `NotImplemented`, and `GetConnectionRecordingPreferences` has no handler file at all (declined by the stub generator because it has a Unit/absent input — see `src/ops/SKIPPED_STUBS.md`), so it is answered by the framework's unroutable-operation path.

SSM GUI Connect models exactly ONE resource: the per-account/per-Region *connection recording preferences* singleton. The first slice is therefore the full lifecycle of that singleton, all three modeled operations, each computing its response from persisted state (restJson1, POST routes as listed in OPERATIONS.md):

1. **UpdateConnectionRecordingPreferences** (`POST /UpdateConnectionRecordingPreferences`, @idempotent) — input `ConnectionRecordingPreferences` (required) + `ClientToken` (@idempotencyToken, len 1..64). Persist the preferences as the account/Region singleton (create-or-replace). Output must echo back the *stored* `ConnectionRecordingPreferences` and the `ClientToken` (generate one when the client omits it, since the member is an idempotency token). `ConnectionRecordingPreferences` = `{ RecordingDestinations: { S3Buckets: [ S3Bucket{ BucketOwner, BucketName } ] } (required, list len 1..1), KMSKeyArn (required, len 1..2048) }` — store all nested members; none may be dropped.
2. **GetConnectionRecordingPreferences** (`POST /GetConnectionRecordingPreferences`, @readonly, no input members) — must be hand-authored (Unit input) and added to the service `operations(...)` allow-list. Return the stored `ConnectionRecordingPreferences` plus the `ClientToken` recorded by the last successful Update. When nothing has ever been configured, return `ResourceNotFoundException` (HTTP 404), not an empty 200.
3. **DeleteConnectionRecordingPreferences** (`POST /DeleteConnectionRecordingPreferences`, @idempotent) — input `ClientToken` (@idempotencyToken). Remove the stored singleton and return the `ClientToken`. A subsequent Get must report `ResourceNotFoundException` (404). Deleting when nothing is configured must surface `ResourceNotFoundException` rather than succeeding silently against no state.

No canned, zeroed or echoed-input responses: Get must read what Update wrote, and Delete must make that state observably gone to the next caller. A second Update must replace (not append to) the stored preferences.

Minimum validation for the slice, grounded in the model's constraints: reject a missing `ConnectionRecordingPreferences`, a `ClientToken` outside len 1..64, an `S3Buckets` list whose length is not exactly 1, a `BucketOwner` that does not match `^[0-9]{12}$`, a `BucketName` that violates the modeled S3 bucket-name pattern, and a missing/over-length `KMSKeyArn` — each as `ValidationException` (HTTP 400) via the crate's `scaffolded::validation` helper.

## State Model

In `src/state.rs` (`ComAmazonawsSsmguiconnectSsmguiconnectState`, reached from handlers via `crate::state::service_state(&*self.engine)` and its lock guard — never a freshly constructed engine):

- `connection_recording_preferences: Option<RecordingPrefsRecord>` — the single per-account/per-Region record (`None` = never configured / deleted).
- `RecordingPrefsRecord { kms_key_arn: String, s3_buckets: Vec<S3BucketRecord>, client_token: String, last_updated: SystemTime }`
- `S3BucketRecord { bucket_owner: String, bucket_name: String }`
- `idempotency_tokens: HashMap<String /* ClientToken */, StoredResponse>` — so replaying an Update/Delete with the same `@idempotencyToken` returns the first outcome instead of mutating twice (both ops are `@idempotent`).

Operations outside this slice: none — these three are the entire service surface, so the bootstrap slice IS the service.

## Evidence

`services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/OPERATIONS.md` lists all 3 bound operations with routes, input/output members and the error set (`ValidationException` 400, `ResourceNotFoundException` 404, `ConflictException` 409, `AccessDeniedException` 403, `ServiceQuotaExceededException` 402, `ThrottlingException` 429, `InternalServerException` 500). `src/ops/SKIPPED_STUBS.md` records that `GetConnectionRecordingPreferences` was never wired (Unit input), and the two existing files in `src/ops/` still carry the scaffold 501 marker. Shape constraints quoted above come from `smithy_query` on `ConnectionRecordingPreferences`, `RecordingDestinations` and `S3Bucket` in `smithy-model/com-amazonaws-ssmguiconnect-ssmguiconnect.json` (required `RecordingDestinations`/`KMSKeyArn`, `S3Buckets` length 1..1, `BucketOwner` pattern `^[0-9]{12}$`, modeled `BucketName` pattern). No tests cover any of this today (`tests/` holds only the generated `state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `ssm-guiconnect` at it.
2. Call GetConnectionRecordingPreferences on a clean store — expect `ResourceNotFoundException` (HTTP 404); today the operation is unroutable/501.
3. Call UpdateConnectionRecordingPreferences with `ConnectionRecordingPreferences={RecordingDestinations:{S3Buckets:[{BucketOwner:"123456789012",BucketName:"my-rdp-recordings"}]},KMSKeyArn:"arn:aws:kms:us-east-1:123456789012:key/abcd"}` and `ClientToken="tok-1"` — expect HTTP 200 whose body returns the stored preferences and `ClientToken="tok-1"`; today HTTP 501.
4. Call GetConnectionRecordingPreferences again — expect the exact bucket owner, bucket name and KMS key ARN just stored, plus the recorded ClientToken.
5. Call UpdateConnectionRecordingPreferences again with a different BucketName and `ClientToken="tok-2"`, then Get — expect the new value to have REPLACED the old one.
6. Call DeleteConnectionRecordingPreferences with `ClientToken="tok-3"` — expect HTTP 200 returning that ClientToken; today HTTP 501.
7. Call GetConnectionRecordingPreferences — expect `ResourceNotFoundException` (HTTP 404), proving the delete removed real persisted state.
8. Call UpdateConnectionRecordingPreferences with an `S3Buckets` list of 2 entries and a `BucketOwner` of "abc" — expect `ValidationException` (HTTP 400) for each constraint violation.
