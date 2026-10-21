Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 103148eb649c3bac8e519c843b1d31d8637412c4

# Bootstrap the signer (AWS Signer) emulator

_Filed: 2026-10-21T22:33:24Z_
_Operation: service-com-amazonaws-signer-wallabyservice | Class: missing_

> _Note: this issue was identified against the code base at commit `103148eb649c3bac8e519c843b1d31d8637412c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 19 operations in `services/service-com-amazonaws-signer-wallabyservice/src/ops/` are unmodified scaffolds: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding zero state. Implement a FIRST stateful slice covering the signing-profile lifecycle plus one signing job, so a realistic client script works end-to-end. Every one of these must compute its response from persisted state — no canned bodies, no echoed input, no random ID that is never stored.

Happy path, in order (routes/fields from OPERATIONS.md, protocol `restJson1`):

1. **PutSigningProfile** — `PUT /signing-profiles/{profileName}`. Input: `profileName` (@httpLabel, len 2..64, `^[a-zA-Z0-9_]{2,}$`), `platformId` (required), `signingMaterial{certificateArn}`, `signatureValidityPeriod{value,type}`, `overrides`, `signingParameters` (map), `tags` (map). Must CREATE/overwrite the profile record, mint a new 10-char `[a-zA-Z0-9]{10}` `profileVersion`, set `status=Active`, and return `arn` (`arn:aws:signer:<region>:<account>:/signing-profiles/<name>`), `profileVersion`, `profileVersionArn` (`<arn>/<profileVersion>`) — all three derived from the stored record, so a second Put on the same name yields a NEW version.
2. **GetSigningProfile** — `GET /signing-profiles/{profileName}` (+ optional `profileOwner` @httpQuery). Must return the stored record: `profileName`, `profileVersion`, `profileVersionArn`, `arn`, `platformId`, `platformDisplayName`, `signingMaterial`, `signatureValidityPeriod`, `overrides`, `signingParameters`, `tags`, `status`, `statusReason`. An unknown name must be `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListSigningProfiles** — `GET /signing-profiles`. Input: `includeCanceled` (default false), `maxResults` (1..25), `nextToken`, `platformId`, `statuses`. Must return `profiles` built from the store as `SigningProfile` entries (profileName, profileVersion, profileVersionArn, arn, platformId, signingMaterial, signatureValidityPeriod, signingParameters, status, tags), honour `maxResults` by truncating and emitting an opaque `nextToken` that resumes where the page ended, and EXCLUDE `Canceled` profiles unless `includeCanceled=true`.
4. **StartSigningJob** — `POST /signing-jobs`. Input: `source{s3{bucketName,key,version}}`, `destination{s3{bucketName,prefix}}`, `profileName`, `clientRequestToken` (@idempotencyToken), `profileOwner`. Must reject an unknown/canceled `profileName` with `ResourceNotFoundException` (404), otherwise persist a job record (snapshotting the profile's `platformId`, `profileVersion`, `signingMaterial`, `createdAt=now`, `status`, `requestedBy`, `jobOwner`) and return the stored `jobId` + `jobOwner`. Idempotency: the same `clientRequestToken` must return the SAME `jobId` rather than creating a second job.
5. **DescribeSigningJob** — `GET /signing-jobs/{jobId}`. Must return the persisted job: `jobId`, `source`, `destination`-derived `signedObject`, `signingMaterial`, `platformId`, `platformDisplayName`, `profileName`, `profileVersion`, `overrides`, `signingParameters`, `createdAt`, `completedAt`, `status` (`InProgress`/`Succeeded`), `statusReason`, `requestedBy`, `jobOwner`, `jobInvoker`. Unknown `jobId` → `ResourceNotFoundException` (404). The job the emulator completes must report `Succeeded` with `completedAt` set, and the `source` echoed back must be the one StartSigningJob stored, not the current request's.
6. **CancelSigningProfile** — `DELETE /signing-profiles/{profileName}`. Must flip the stored profile `status` to `Canceled` (Signer does not hard-delete), so a subsequent GetSigningProfile shows `Canceled` and a default ListSigningProfiles (`includeCanceled=false`) omits it, and a subsequent StartSigningJob against it fails 404. Unknown name → `ResourceNotFoundException` (404).

Also required for the slice to be honest: @httpLabel/@httpQuery constraints must be enforced — a `profileName` violating len 2..64 or `^[a-zA-Z0-9_]{2,}$`, or `maxResults` outside 1..25, must be `ValidationException` (HTTP 400).

## State Model

Store in `ComAmazonawsSignerWallabyserviceState` (`src/state.rs`), reached via `crate::state::service_state(&*self.engine)`:

- `profiles: BTreeMap<String /* profileName */, SigningProfileRecord>` where `SigningProfileRecord { profile_name: String, profile_version: String /* 10 chars */, arn: String, profile_version_arn: String, platform_id: String, platform_display_name: Option<String>, signing_material: Option<SigningMaterial{certificate_arn}>, signature_validity_period: Option<{value: i32, type: ValidityType}>, overrides: Option<SigningPlatformOverrides>, signing_parameters: BTreeMap<String,String>, status: SigningProfileStatus /* Active|Canceled|Revoked */, status_reason: Option<String>, tags: BTreeMap<String,String>, created_at: SystemTime, versions: Vec<String> }`
- `jobs: BTreeMap<String /* jobId */, SigningJobRecord>` where `SigningJobRecord { job_id: String, profile_name: String, profile_version: String, platform_id: String, platform_display_name: Option<String>, source: Source, destination: Destination, signed_object: Option<SignedObject>, signing_material: Option<SigningMaterial>, signing_parameters: BTreeMap<String,String>, overrides: Option<SigningPlatformOverrides>, status: SigningStatus, status_reason: Option<String>, created_at: SystemTime, completed_at: Option<SystemTime>, signature_expires_at: Option<SystemTime>, requested_by: Option<String>, job_owner: String, job_invoker: Option<String>, is_revoked: bool, client_request_token: String }`
- `job_tokens: HashMap<String /* clientRequestToken */, String /* jobId */>` for StartSigningJob idempotency.
- A monotonic counter/uuid source for `jobId` and for the 10-char `profileVersion`, stored alongside so IDs are stable across calls.

The remaining operations (SignPayload, RevokeSignature, RevokeSigningProfile, GetRevocationStatus, ListSigningJobs, ListSigningPlatforms, GetSigningPlatform, Add/Remove/ListProfilePermissions, TagResource/UntagResource/ListTagsForResource) stay at 501 for this slice and are filed as individual gap issues later — note that the `tags` map stored by PutSigningProfile should be the same map those tagging ops will later read, so key profiles by ARN-resolvable name from the start.

## Evidence

OPERATIONS.md (crate root, re-rendered from `smithy-model/com-amazonaws-signer-wallabyservice.json`) documents all 19 operations with routes, members and errors: PutSigningProfile `PUT /signing-profiles/{profileName}` → `arn`/`profileVersion`/`profileVersionArn`; GetSigningProfile `GET /signing-profiles/{profileName}`; ListSigningProfiles `GET /signing-profiles` with @paginated(inputToken=nextToken,outputToken=nextToken) and `includeCanceled`/`maxResults` (range 1..25); StartSigningJob `POST /signing-jobs` with @idempotencyToken `clientRequestToken` → `jobId`/`jobOwner`; DescribeSigningJob `GET /signing-jobs/{jobId}` → full job incl. `status` (Failed|InProgress|Succeeded); CancelSigningProfile `DELETE /signing-profiles/{profileName}` (profile status enum includes `Canceled`, grounding soft-cancel). `ResourceNotFoundException (HTTP 404)` and `ValidationException (HTTP 400)` are modeled on these operations. Shape members for Source/Destination/SigningMaterial/SignatureValidityPeriod/SigningProfile/SigningJob confirmed via smithy_query. Census: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 19 handler files — zero implemented operations, no tests beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI signer client at it.
2. aws signer put-signing-profile --profile-name MyProfile_1 --platform-id AWSIoTDeviceManagement-SHA256-ECDSA --signing-material certificateArn=arn:aws:acm:us-east-1:123456789012:certificate/abc — expect 200 with arn, a 10-char profileVersion and profileVersionArn (today: HTTP 501).
3. aws signer get-signing-profile --profile-name MyProfile_1 — expect the platformId, signingMaterial, tags and signatureValidityPeriod exactly as submitted, status=Active, and the same profileVersion returned by the Put.
4. aws signer list-signing-profiles --max-results 1 — expect MyProfile_1 in `profiles` and, if more profiles exist, a nextToken that returns the next page.
5. aws signer start-signing-job --source s3={bucketName=src-bucket,key=code.bin,version=1} --destination s3={bucketName=dst-bucket,prefix=signed/} --profile-name MyProfile_1 --client-request-token tok-1 — expect 200 with a jobId; repeat with the SAME token and expect the SAME jobId.
6. aws signer describe-signing-job --job-id <jobId> — expect the stored source/destination, profileName=MyProfile_1, the profileVersion snapshotted at job start, createdAt, and status Succeeded/InProgress.
7. aws signer start-signing-job --profile-name NoSuchProfile ... — expect ResourceNotFoundException with HTTP 404.
8. aws signer cancel-signing-profile --profile-name MyProfile_1 — expect 200; then get-signing-profile shows status=Canceled, list-signing-profiles (default includeCanceled=false) omits it, and start-signing-job against it returns 404.
9. aws signer get-signing-profile --profile-name Nope_X — expect ResourceNotFoundException (404); aws signer list-signing-profiles --max-results 99 — expect ValidationException (400).
