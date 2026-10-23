Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify__s3.bucket-prefix-job-needs-list-bucket-as-well-as-get-object amplify__s3.bucket-root-deploy-of-the-misaligned-seed-is-rejected amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files amplify__s3.start-deployment-without-an-amplify-bucket-grant-is-rejected amplify__s3.zip-deploy-of-a-missing-s3-object-is-rejected
Issue-Key: amplify__s3|StartDeployment|NotImplemented
Issue-Key: amplify__s3.start-deployment-without-an-amplify-bucket-grant-is-rejected|run#9|AssertionError|assert refused == expected, (
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# Amplify StartDeployment is an unimplemented scaffold stub: answers 501 NotImplemented, blocking all S3-sourced manual deployments

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDeployment

## Description
One root cause, grounded in 5 failing cross-service (amplify + s3) behavioral checks.

Cause 1 of 1: `StartDeployment` is still a scaffold stub and answers HTTP 501 `NotImplemented` for every request, so no manually deployed Amplify app can be driven from an S3 source.

NOTE on service attribution: these checks live under the composite check-id prefix `amplify__s3` (they exercise amplify and s3 together). The defect itself is in the Amplify crate (`services/service-com-amazonaws-amplify-amplify`); S3 behaves correctly in these checks. Both constituent services are stood up in this checkout, so this is a fidelity finding, not a bootstrap gap. This operation is NOT marked "ALREADY FILED" or "OWNED by" in the suite report, so no issue currently covers it.

AWS: Smithy model com.amazonaws.amplify#StartDeployment — POST /apps/{appId}/branches/{branchName}/deployments/start, success status 200, required output member `jobSummary` (JobSummary). Input: `appId` (required, httpLabel, len 1..20, pattern ^d[a-z0-9]+$), `branchName` (required, httpLabel, len 1..255), `jobId` (pattern ^[0-9]+$), `sourceUrl` (len 0..3000, pattern ^(s3|https|http)://), `sourceUrlType` enum BUCKET_PREFIX | ZIP. Modeled errors: BadRequestException (400), InternalFailureException (500), LimitExceededException (429), NotFoundException (404), UnauthorizedException (401). Model doc: "Starts a deployment for a manually deployed app... The maximum duration between the CreateDeployment call and the StartDeployment call cannot exceed 8 hours." Recorded behavior for the authorization path: a StartDeployment against an S3 source with no Amplify bucket grant is refused with error code `UnauthorizedException` (modeled httpError 401), recorded message example "The bucket policy is either missing or has insufficient permissions for this operation". Recorded behavior for the accepted path: the call returns a jobSummary whose job then progresses to a terminal status (SUCCEED / FAILED) observable via GetJob/ListJobs — e.g. a GetObject-only grant is accepted synchronously and the job later ends FAILED, while adding s3:ListBucket makes the same BUCKET_PREFIX deployment end SUCCEED.

Emulator: every StartDeployment call returns HTTP 501 with error code `NotImplemented` and message "operation StartDeployment is scaffolded but not implemented in this emulator". The handler body in services/service-com-amazonaws-amplify-amplify/src/ops/start_deployment.rs is `Err(crate::scaffolded::Scaffolded::not_implemented("StartDeployment"))` (line 175) and the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker (line 4).

Why: a client cannot deploy anything to a manually deployed Amplify app. Four checks never get past the first call, and the negative check gets the wrong refusal entirely: instead of a 401 `UnauthorizedException` describing a missing bucket grant it sees 501 `NotImplemented`. 501/NotImplemented is not a modeled error for this operation, so SDK retry/error-handling logic that branches on UnauthorizedException, NotFoundException or BadRequestException takes no branch at all, and an agent testing "deploy frontend from S3" against the emulator sees a service outage rather than the authorization or job outcome AWS would give.

Fix: implement the operation's handler — services/service-com-amazonaws-amplify-amplify/src/ops/start_deployment.rs — and delete the SCAFFOLDED marker. It needs: (a) source-authorization validation of the referenced s3:// bucket/prefix against the bucket policy, refusing with UnauthorizedException (401) when the Amplify principal lacks the required grant and with NotFoundException/BadRequestException for an unresolvable or malformed source per the recordings; (b) creation of a job record in the service state so the returned `jobSummary` is readable through GetJob/ListJobs and reaches a terminal status; (c) BUCKET_PREFIX vs ZIP source handling, including the asset set that the served site and its rewrite rules resolve against.

Traces: amplify__s3.bucket-prefix-job-needs-list-bucket-as-well-as-get-object, amplify__s3.bucket-root-deploy-of-the-misaligned-seed-is-rejected, amplify__s3.rewrite-rules-map-the-referenced-asset-paths-onto-the-seeded-files, amplify__s3.start-deployment-without-an-amplify-bucket-grant-is-rejected, amplify__s3.zip-deploy-of-a-missing-s3-object-is-rejected.

## Evidence
Expected (recorded AWS, negative check): StartDeployment with an s3:// sourceUrl and no Amplify bucket grant -> error code `UnauthorizedException` (modeled httpError 401), message example "The bucket policy is either missing or has insufficient permissions for this operation".
Actual: ('NotImplemented', 'operation StartDeployment is scaffolded but not implemented in this emulator') with HTTP 501.

Expected (recorded AWS, accepted path): StartDeployment returns 200 with a required `jobSummary`; the job is then observable and reaches SUCCEED or FAILED (GetObject-only BUCKET_PREFIX deployment -> FAILED; same deployment with s3:ListBucket added -> SUCCEED).
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartDeployment operation: operation StartDeployment is scaffolded but not implemented in this emulator — raised on the first StartDeployment call in 4 of the 5 checks.

Source: services/service-com-amazonaws-amplify-amplify/src/ops/start_deployment.rs:4 (`//! SCAFFOLDED, NOT IMPLEMENTED`) and :175 (`Err(crate::scaffolded::Scaffolded::not_implemented("StartDeployment"))`).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1.
2. Or directly: create an S3 bucket, seed objects under a src/ prefix, attach a bucket policy granting the Amplify principal s3:GetObject, then CreateApp + CreateBranch(appId, 'main') on amplify.
3. Call amplify.start_deployment(appId=<appId>, branchName='main', sourceUrl='s3://<bucket>/src/', sourceUrlType='BUCKET_PREFIX').
4. Observe HTTP 501 / error code NotImplemented instead of a 200 response carrying `jobSummary`.
5. Repeat step 3 with NO bucket policy granting Amplify: observe 501 NotImplemented instead of 401 UnauthorizedException.
