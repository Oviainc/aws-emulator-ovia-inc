Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# StartSigningJob accepts a signing profile on the container-image platform instead of refusing it

_Filed: 2026-10-24T19:47:57Z_
_Operation: StartSigningJob | Class: validation_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: AWS Signer's S3-based signing-job workflow (StartSigningJob) is not available for container-image (Notation/OCI) signing profiles — container images are signed through SignPayload. A StartSigningJob against a profile whose platformId is the container-image platform (`Notation-OCI-SHA384-ECDSA`) is refused by AWS with the operation's modelled 400 ValidationException rather than starting a job.

Actual: the handler validates only that `source.s3` is present and that `destination.s3.bucketName` is present, then resolves the profile and checks `profile.can_sign()` (i.e. status == Active). Nothing consults the profile's PLATFORM, so a profile created on `Notation-OCI-SHA384-ECDSA` starts a normal S3 signing job: StartSigningJob answers 200 with a jobId, and ~500 ms later DescribeSigningJob reports `status: Succeeded` with a synthesized `signedObject` for a workflow AWS never runs over S3 at all.

The emulator already knows which catalog entry is the container-image platform — `crate::state::SIGNING_PLATFORM_CATALOG` marks `Notation-OCI-SHA384-ECDSA` as the container-image platform (its `requires_signing_material: true` entry, documented as "the container-image platform"), and `crate::state::signing_platform()` is the lookup the gate would use — so the platform fact needed to refuse the call is already persisted on the job's profile record (`SigningProfileRecord::platform_id`).

## Evidence

Spec grounding (Smithy model of this service): `AddProfilePermissionRequest$action` documentation enumerates the grantable actions and states for them: "signer:StartSigningJob. This action isn't supported for container image workflows" and, symmetrically, "signer:SignPayload. This action isn't supported for AWS Lambda workflows." StartSigningJob is therefore not a container-image-workflow operation; its own doc describes the S3 source/destination flow only ("You must create an Amazon S3 source bucket… AWS Signer uses your S3 destination bucket to write your signed code"). StartSigningJob models ValidationException (HTTP 400) for exactly this kind of refusal.

Handler: services/service-com-amazonaws-signer-wallabyservice/src/ops/start_signing_job.rs — `handle()` lines ~203-231 validate only `source.s3` / `destination.s3.bucketName`; lines ~261-275 resolve the profile and gate on `profile.can_sign()` (status Active). `input`/`profile.platform_id` is copied onto the job record (lines ~281-307) but never checked against `crate::state::signing_platform(..)`, so no platform-based refusal exists.

State: services/service-com-amazonaws-signer-wallabyservice/src/state.rs — `SIGNING_PLATFORM_CATALOG` (lines ~780-841) holds the `Notation-OCI-SHA384-ECDSA` entry and its doc comment calls it "the container-image platform"; `signing_platform()` (lines ~845-849) is the existing lookup.

Test coverage: no test asserts a refusal here. tests/all/signing_profile_lifecycle.rs creates profiles on `Notation-OCI-SHA384-ECDSA` (case "Container_Profile", line ~230) and exercises StartSigningJob only against S3-style platforms; nothing covers StartSigningJob on a container-image profile, so this behavior is unpinned in either direction.

Not a duplicate: the open issue 20261023T031331Z-signer-accepts-calls-aws-rejects-and-reports-signing covers (a) PutSigningProfile accepting a duplicate profile name, (b) GetRevocationStatus ARN validation, and (c) a job reporting Succeeded with a signedObject that was never written to S3 — none of which is a platform-eligibility check on the StartSigningJob input.

## Steps to Reproduce

1. Boot the emulator and PutSigningProfile `PUT /signing-profiles/Container_Profile` with body {"platformId": "Notation-OCI-SHA384-ECDSA", "signingMaterial": {"certificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/abc"}} — observe 200.
2. Call StartSigningJob `POST /signing-jobs` with {"profileName": "Container_Profile", "source": {"s3": {"bucketName": "src", "key": "code.zip", "version": "1"}}, "destination": {"s3": {"bucketName": "dst", "prefix": "signed/"}}, "clientRequestToken": "tok-1"}.
3. Observe HTTP 200 with a jobId instead of HTTP 400 ValidationException: the container-image profile started an S3 signing job.
4. Wait ~1s and call DescribeSigningJob `GET /signing-jobs/{jobId}` — observe status `Succeeded` with a `signedObject` for a workflow AWS does not run through StartSigningJob.
5. Contrast with the same StartSigningJob against a profile on `AWSLambda-SHA384-ECDSA`, which must keep succeeding — the refusal is a decision about the profile's platform, not about StartSigningJob generally.
