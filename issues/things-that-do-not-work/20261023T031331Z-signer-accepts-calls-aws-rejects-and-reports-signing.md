Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: signer.put-signing-profile-duplicate-name-permanently-rejected signer.get-revocation-status-malformed-arn-rejected signer.start-signing-job-invalid-source-transitions-to-failed
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer.put-signing-profile-duplicate-name-permanently-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: signer.get-revocation-status-malformed-arn-rejected|_error#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3|HeadObject|404
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# Signer accepts calls AWS rejects and reports signing jobs whose signed object was never written

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSigningProfile

## Description
Three distinct root causes in the signer emulator, all surfaced by the simparity-behavioral-aws-v1 suite. All three must be closed before this issue is resolved.

Cause 1 of 3: PutSigningProfile silently re-creates an existing profile name instead of rejecting it as a duplicate (check signer.put-signing-profile-duplicate-name-permanently-rejected).
AWS: Smithy PutSigningProfile is PUT /signing-profiles/{profileName}, http code 200, output {arn, profileVersion (@length 10, @pattern ^[a-zA-Z0-9]{10}$), profileVersionArn}; its modeled client errors are AccessDeniedException/403, ResourceNotFoundException/404, TooManyRequestsException/429 and ValidationException/400. The model does not state duplicate-name semantics, so: recorded behavior only — a signing-profile name is permanently reserved once used, and a second PutSigningProfile for the same profileName is rejected with HTTP 400 / error code ValidationException. The recording's message is an example ("Profile with name <profileName> already exists"), not a requirement. The recording further shows this rejection holds regardless of the overridable signatureValidityPeriod and even after the profile has been Canceled, and that the original profile's profileVersion/profileVersionArn are left untouched by the rejected attempt.
Emulator: answers HTTP 200 and mints a NEW version for the already-taken name — the second identical PutSigningProfile returned {arn: arn:aws:signer:<region>:<account>:/signing-profiles/<profileName>, profileVersion: '0000000002', profileVersionArn: '<arn>/0000000002'} rather than a 400 ValidationException.
Why: a client that uses PutSigningProfile as a create (relying on AWS's duplicate rejection to detect a name collision, or to make creation safely retryable without clobbering an existing profile) gets a success where AWS gives an error. The pre-existing profile's current version silently advances, so the profileVersionArn a client already holds and pins signing jobs to stops being the profile's latest version, and a name-collision guard built on the ValidationException never fires.
Fix: the PutSigningProfile handler's validation/state path — it must check the signing-profile store for an existing entry under profileName (including Canceled ones, whose names are not released) and raise ValidationException/400 instead of appending a version.

Cause 2 of 3: GetRevocationStatus accepts a malformed profileVersionArn/jobArn and answers 200 (check signer.get-revocation-status-malformed-arn-rejected).
AWS: Smithy GetRevocationStatus is GET /revocations (host prefix data-) with signatureTimestamp, platformId, profileVersionArn, jobArn and certificateHashes all @required @httpQuery; profileVersionArn and jobArn target Arn with @length min 20 max 2048; the operation's modeled client errors are AccessDeniedException/403, TooManyRequestsException/429 and ValidationException/400. Beyond the length bound the ARN shape is validated server-side: recorded behavior only — a syntactically malformed ARN in profileVersionArn/jobArn is rejected with HTTP 400 / error code ValidationException (recorded message, as an example: "Invalid profileVersionArn: <value>"), while the identical call with real profileVersionArn and jobArn values succeeds. The rejection is therefore a decision about ARN validity, not a refusal of the operation.
Emulator: answers HTTP 200 with body {"revokedEntities": []} for a profileVersionArn and jobArn of 'arn:aws:signer:<region>:<account>:not-a-real-arn-shape' — no validation of the ARN's shape at all.
Why: GetRevocationStatus is a security decision point. An empty revokedEntities list is the "nothing is revoked, this signature is fine" answer, so a client that passed a wrong, truncated or wrongly-constructed ARN is told the artifact is not revoked instead of being told its request was invalid. A revocation check silently becomes a no-op and a revoked signature can be treated as good.
Fix: the GetRevocationStatus handler's input validation — parse/validate the profileVersionArn and jobArn query members as signer ARNs (and reject with ValidationException/400 naming the offending member) before the revocation lookup runs.

Cause 3 of 3: a signing job reports Succeeded with a signedObject that does not exist at the destination (check signer.start-signing-job-invalid-source-transitions-to-failed).
AWS: Smithy StartSigningJob is POST /signing-jobs with @required source, destination, profileName and clientRequestToken and returns {jobId}; DescribeSigningJob is GET /signing-jobs/{jobId} returning status (SigningStatus enum), statusReason and signedObject (SignedObject{s3{bucketName,key}}). Recorded behavior only for the job's effect: when DescribeSigningJob reports status Succeeded and a signedObject whose s3.bucketName/s3.key name the destination given to StartSigningJob, that object actually exists and is independently readable at the destination (HeadObject on the reported bucket/key returns 200). Conversely a job whose source is not a valid zip archive reaches status Failed with a non-empty statusReason and no signedObject, and writes nothing under the destination prefix.
Emulator: DescribeSigningJob reported Succeeded and a signedObject in the destination bucket, but the independent read of exactly that reported key failed — HeadObject returned HTTP 404 ("An error occurred (404) when calling the HeadObject operation: Not Found"). The job's reported terminal state is not backed by any object at the destination.
Why: the whole point of a signing job is the signed artifact. A client that polls DescribeSigningJob to Succeeded and then fetches signedObject.s3 — the documented handoff for Lambda code signing and container signing — gets a 404 for an object the service just told it exists, so every downstream deploy/verify step fails on a job the service called successful. It also makes the Failed path untestable: a job that produces nothing is indistinguishable from one that succeeded.
Fix: the StartSigningJob job-execution/state path — the job must either write a real object at destination.s3 (bucketName + prefix) before it is allowed to report Succeeded, or report Failed with a statusReason and no signedObject; and the DescribeSigningJob serializer must only emit signedObject for a job whose artifact was actually persisted. The source-validation leg (non-zip source must drive the job to Failed) lives in the same path.

Related but NOT the same cause: the open issue 20261023T002214Z-signer-signing-profile-lifecycle-responses-diverge-from-aws.md covers PutSigningProfile rejecting a VALID create that omits signingMaterial, plus CancelSigningProfile/RevokeSigningProfile/GetSigningProfile response defects. Cause 1 here is the opposite direction (an invalid create being ACCEPTED) and Causes 2 and 3 are different operations; none of the three is covered by that issue.

## Evidence
Replay of suite simparity-behavioral-aws-v1 (signer 3/23 passed, 20 failed). Only the three causes below are net-new; the suite marked the remaining signer failures ALREADY FILED (unimplemented AddProfilePermission / GetSigningPlatform / ListSigningJobs / ListTagsForResource) or OWNED by an existing issue.

Cause 1 — PutSigningProfile, check signer.put-signing-profile-duplicate-name-permanently-rejected [mismatch]:
  expected: HTTP 400, Error.Code = ValidationException (recorded message example: "Profile with name <profileName> already exists")
  actual:   HTTP 200 with {'arn': 'arn:aws:signer:us-west-2:000000000000:/signing-profiles/sim2real_signer_version_2d0deceffc9e', 'profileVersion': '0000000002', 'profileVersionArn': 'arn:aws:signer:us-west-2:000000000000:/signing-profiles/sim2real_signer_version_2d0deceffc9e/0000000002'}
  harness: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {...}
  Smithy: PutSigningProfile PUT /signing-profiles/{profileName} code 200; errors AccessDeniedException/403, InternalServiceErrorException/500, ResourceNotFoundException/404, TooManyRequestsException/429, ValidationException/400.

Cause 2 — GetRevocationStatus, check signer.get-revocation-status-malformed-arn-rejected [mismatch]:
  request: profileVersionArn = jobArn = 'arn:aws:signer:us-east-1:123456789012:not-a-real-arn-shape', platformId Notation-OCI-SHA384-ECDSA, one correctly-shaped certificateHashes entry
  expected: HTTP 400, Error.Code = ValidationException (recorded message example: "Invalid profileVersionArn: <value>")
  actual:   HTTP 200, body {'revokedEntities': []} (content-length 22)
  harness: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... 'revokedEntities': []}
  Smithy: GetRevocationStatus GET /revocations; profileVersionArn/jobArn @required, target Arn @length 20..2048; errors AccessDeniedException/403, InternalServiceErrorException/500, TooManyRequestsException/429, ValidationException/400.

Cause 3 — StartSigningJob/DescribeSigningJob, check signer.start-signing-job-invalid-source-transitions-to-failed [error]:
  the check's positive control signed a genuinely valid zip, polled DescribeSigningJob to a terminal status, asserted status == Succeeded and signedObject.s3.bucketName == the destination bucket, then read that exact key back
  expected: HeadObject on signedObject.s3.key in the destination bucket returns 200
  actual:   botocore.exceptions.ClientError: An error occurred (404) when calling the HeadObject operation: Not Found
  i.e. DescribeSigningJob advertised a signedObject at the destination that was never written. The malformed-source leg (non-zip source must reach Failed with a statusReason and leak no object under the destination prefix) was never reached.
  Smithy: StartSigningJob POST /signing-jobs -> {jobId}; DescribeSigningJob GET /signing-jobs/{jobId} -> {status: SigningStatus, statusReason, signedObject: SignedObject}.

## Steps to Reproduce
1. git_clone the repo and build/boot the emulator from the checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite (run_behavioral_suite).
3. Cause 1: PutSigningProfile(profileName=<fresh name matching ^[a-zA-Z0-9_]{2,}$>, platformId='AWSLambda-SHA384-ECDSA'), then call PutSigningProfile again with the SAME profileName. Observe HTTP 200 and a new profileVersion ('0000000002') instead of HTTP 400 ValidationException. Repeat with signatureValidityPeriod={'value':10,'type':'MONTHS'} and again after CancelSigningProfile — all three must be rejected, and the original profileVersion/profileVersionArn must be unchanged.
4. Cause 2: GetRevocationStatus(signatureTimestamp=<now>, platformId='Notation-OCI-SHA384-ECDSA', profileVersionArn='arn:aws:signer:us-east-1:123456789012:not-a-real-arn-shape', jobArn=<same>, certificateHashes=['ab'*96]). Observe HTTP 200 {'revokedEntities': []} instead of HTTP 400 ValidationException. Then repeat with a real profileVersionArn from PutSigningProfile and a real jobArn — that call must still succeed, proving the rejection is about ARN validity.
5. Cause 3: create a signing profile on 'AWSLambda-SHA384-ECDSA', create a versioned S3 source bucket and a destination bucket, PutObject a VALID zip, StartSigningJob(source=s3{bucket,key,version}, destination=s3{bucketName:<dst>,prefix:'signed/'}, profileName, clientRequestToken), poll DescribeSigningJob to a terminal status. Observe status Succeeded with a signedObject, then HeadObject(Bucket=<dst>, Key=signedObject.s3.key) -> 404.
6. Then PutObject a non-zip body (b'this is not a valid zip archive') and StartSigningJob on it: the job must reach status Failed with a non-empty statusReason, no signedObject, and nothing written under the 'signed/' prefix.
7. Re-replay these check ids after fixing: signer.put-signing-profile-duplicate-name-permanently-rejected, signer.get-revocation-status-malformed-arn-rejected, signer.start-signing-job-invalid-source-transitions-to-failed.
