Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: signer.sign-payload-missing-iam-permission-denied signer.list-signing-jobs-maxresults-out-of-range-rejected signer.revoke-signature-empty-reason-rejected
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer.sign-payload-missing-iam-permission-denied|_error#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: signer.list-signing-jobs-maxresults-out-of-range-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: signer.revoke-signature-empty-reason-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 4feb6bd43b3136ec20364422b5e95f1a19d31d6d

# signer: 3 net-new causes — SignPayload is never authorized, and ListSigningJobs/RevokeSignature validation refusals carry generic constraint text instead of AWS's wording

> _Note: this issue was identified against the code base at commit `4feb6bd43b3136ec20364422b5e95f1a19d31d6d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignPayload

## Description
Three net-new root causes on ONE service (signer), bundled: one authorization bypass on SignPayload and two refusal-message divergences on ListSigningJobs and RevokeSignature. A fixer must close all three before submitting.

=== Cause 1 of 3: SignPayload never consults the caller's identity, so a principal with zero permissions is signed for instead of refused ===
Failing check ids for this cause: signer.sign-payload-missing-iam-permission-denied.

AWS: HTTP 403 with Error.Code 'AccessDeniedException'. Smithy (services/service-com-amazonaws-signer-wallabyservice/smithy-model/com-amazonaws-signer-wallabyservice.json, operation SignPayload, POST /signing-jobs/with-payload, code 200): errors include AccessDeniedException with errorKind 'client' and httpError 403, alongside ValidationException(400), ResourceNotFoundException(404), TooManyRequestsException(429), InternalServiceErrorException(500). Input members: profileName (@required, @length 2..64, @pattern ^[a-zA-Z0-9_]{2,}$), payload (@required blob, @length 1..4096), payloadFormat (@required), profileOwner (optional AccountId). For an IAM user with no identity-based policy at all, the refusal is worded with AWS's standard implicit-deny sentence: "User: <caller ARN> is not authorized to perform: signer:SignPayload on resource: <signing-profile ARN> because no identity-based policy allows the signer:SignPayload action". Fixed text: "User: " … " is not authorized to perform: signer:SignPayload on resource: " … " because no identity-based policy allows the signer:SignPayload action". Variable parts, taken from the request and from emulator state and never from one recording: the caller ARN, of the form arn:aws:iam::<accountId>:user/<userName> where accountId is the credential scope's account and userName is the user created by iam.CreateUser in the same run; and the resource ARN arn:aws:signer:<region>:<accountId>:/signing-profiles/<profileName> built from the request's profileName. The operation is one-sided: the identical call from the fully-permissioned identity must still succeed, return a jobId, and reach a Succeeded job readable via DescribeSigningJob and ListSigningJobs.
Emulator: the zero-permission caller is served exactly what the permissioned caller is served — HTTP 200 with jobId, jobOwner, metadata and a signature blob — and no AccessDeniedException is ever produced. The check creates an IAM user with no attached or inline policy, mints an access key, and polls the same SignPayload call for 45s waiting for Error.Code 'AccessDeniedException'; it instead gets a 200 on the first probe, so the harness's _error() helper raises "expected the call to be rejected, but it SUCCEEDED and returned {...'jobId': ..., 'signature': b'...'}". The handler never evaluates the caller's identity, so the modelled AccessDeniedException variant is declared and never constructed. Side effect: the denied attempt also creates a real signing job, which the check's follow-on assertion (a denied SignPayload must not create a job) would catch as well.
Why: a client that fences signing off by granting no signer:SignPayload permission has that fence silently removed — any credential the emulator accepts can mint signatures. Least-privilege configurations and negative-path tests that assert the 403 validate green against the emulator and then fail closed against AWS, and a caller that branches on AccessDeniedException to fall back never takes that branch. Because the denied call also persists a job, audit/inventory logic sees signing activity AWS would never have recorded.
Fix: an authorization gate for SignPayload in the signer crate, ahead of the handler body where the credential scope is already resolved (the ops/ handler for SignPayload, with the deny wording and ARN derivation shared with the crate's other refusals) — evaluate the caller's identity-based policy for action signer:SignPayload against the profile ARN derived from the request's profileName, and on an implicit deny (no policy allows it) answer the modelled AccessDeniedException (403) with the message template above, creating no job. Related open issues, named rather than relied on: 20261024T054020Z-signer-signpayload-stub.md says SignPayload is an unimplemented scaffold stub, but the suite shows it now answering HTTP 200 with a real signature, so what remains is the missing authorization, not the missing handler; 20261108T035301Z-signer-getrevocationstatus-is-never-authorized-an-identity-carrying-an.md is the same class of bypass on a different operation (GetRevocationStatus) and does not cover SignPayload. Neither issue was marked as owning this check.
Traces: signer.sign-payload-missing-iam-permission-denied.

=== Cause 2 of 3: ListSigningJobs rejects an out-of-range maxResults with a generic framework constraint message instead of AWS's ListSigningJobs wording ===
Failing check ids for this cause: signer.list-signing-jobs-maxresults-out-of-range-rejected.

AWS: HTTP 400 with Error.Code 'ValidationException' and Error.Message "MaxResults parameter must be an integer between 1 and 25". Smithy: ListSigningJobs (GET /signing-jobs) input member maxResults targets MaxResults with @range min 1, max 25, bound as httpQuery 'maxResults'; errors include ValidationException (client, httpError 400). The message is a single fixed sentence for this member; its only variable parts are the modelled range bounds (1 and 25), so no request value appears in it.
Emulator: the error code is right (ValidationException) but the message is the framework's generic constraint report: "1 validation error detected: Value '26' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25". That text echoes the offending value and the member path instead of AWS's sentence for this parameter.
Why: the status and code match, so a client that only branches on the code is unaffected, but anything that surfaces, logs, matches or asserts on the message sees text AWS never emits. Tests and error-mapping tables keyed on the real sentence fail against the emulator, and operators debugging against emulator output learn the wrong message for this limit.
Fix: the refusal message for the maxResults range check in the ListSigningJobs handler's input validation in the signer crate — this member's out-of-range refusal must carry the operation-specific sentence rather than falling through to the crate's generic constraint-violation formatter. The in-range positive control in the same check (maxResults=1 truncating to one job and returning a nextToken) must keep working.
Traces: signer.list-signing-jobs-maxresults-out-of-range-rejected.

=== Cause 3 of 3: RevokeSignature rejects an empty reason with a generic framework constraint message instead of AWS's wording ===
Failing check ids for this cause: signer.revoke-signature-empty-reason-rejected.

AWS: HTTP 400 with Error.Code 'ValidationException' and Error.Message "Reason cannot be empty." (including the trailing period). Smithy: RevokeSignature (PUT /signing-jobs/{jobId}/revoke) input member reason targets RevocationReasonString, @required, @length min 1 max 500; errors include ValidationException (client, httpError 400). The message is fixed text with no variable parts.
Emulator: the code is right (ValidationException) but the message is the generic constraint report: "1 validation error detected: Value '' at 'reason' failed to satisfy constraint: Member must have length greater than or equal to 1". Note the check deliberately disables botocore's client-side parameter validation so the empty reason actually reaches the service; the refusal therefore is the emulator's own.
Why: same impact class as Cause 2 — message-keyed clients, assertions and error catalogues see text AWS never produces for an empty revocation reason. The effect-absent and positive-control legs of the check (no revocationRecord after the rejected call; a non-empty reason then revoking and sticking) are not what fails here, so only the wording is at issue.
Fix: the refusal message for the empty/short reason check in the RevokeSignature handler's input validation in the signer crate — emit the operation-specific sentence rather than the crate's generic constraint-violation formatter.
Traces: signer.revoke-signature-empty-reason-rejected.

Not filed as defects, for the record: the emulator's deterministic jobId (00000000-0000-0000-0000-000000000001), account id and signature bytes in the SignPayload response are per-environment values, not requirements, and are not part of any cause above.

## Evidence
Behavioral suite replay against the emulator built from this checkout: 26/40 passed (65.0%), 13 mismatched, 1 errored, 0 skipped; the only service exercised was signer. Eleven of the fourteen failures were marked 'OWNED by' an open issue and skipped. The three below are the surviving failures.

[mismatch] signer.sign-payload-missing-iam-permission-denied:
  File "check_signer_sign_payload_missing_iam_permission_denied.py", line 54, in probe
    return _error(
  File "check_signer_sign_payload_missing_iam_permission_denied.py", line 183, in _error
    raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'jobId': '00000000-0000-0000-0000-000000000001', 'jobOwner': '000000000000', 'metadata': {'profileVersionArn': 'arn:aws:signer:us-west-2:000000000000:/signing-profiles/<profile>/0000000001', 'jobId': '00000000-0000-0000-0000-000000000001'}, 'signature': b'0e\x020d...'}
Sequence exercised, all against emulated services: signer.PutSigningProfile(profileName=<generated>, platformId='Notation-OCI-SHA384-ECDSA'); iam.CreateUser(UserName=<generated>) with NO policy attached; iam.CreateAccessKey for that user; then signer.SignPayload(profileName=<that profile>, payload=<notation payload>, payloadFormat='application/vnd.cncf.notary.payload.v1+json') with that user's credentials, polled every 3s for up to 45s.
Expected: HTTP 403, Error.Code 'AccessDeniedException', Error.Message "User: arn:aws:iam::<accountId>:user/<userName> is not authorized to perform: signer:SignPayload on resource: arn:aws:signer:<region>:<accountId>:/signing-profiles/<profileName> because no identity-based policy allows the signer:SignPayload action" — accountId/userName/region/profileName fill that template from the request and from the iam.CreateUser of the same run; the requirement is the fixed text plus the derivation rule, not one run's literals. Additionally no signing job may exist for that profile afterwards.
Actual: HTTP 200 with jobId, jobOwner, metadata and a signature blob, on the first probe.

[mismatch] signer.list-signing-jobs-maxresults-out-of-range-rejected:
  File "check_signer_list_signing_jobs_maxresults_out_of_range_rejected.py", line 50, in run
    assert message == expected
  AssertionError: expected message 'MaxResults parameter must be an integer between 1 and 25', got "1 validation error detected: Value '26' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25"
Call: signer.ListSigningJobs(maxResults=26) after two real signing jobs are visible. Error.Code was 'ValidationException' as expected (that assertion passed); only the message diverges. The check notes maxResults=0 is rejected client-side by botocore and is deliberately not asserted.

[mismatch] signer.revoke-signature-empty-reason-rejected:
  File "check_signer_revoke_signature_empty_reason_rejected.py", line 43, in run
    assert message == expected
  AssertionError: expected message 'Reason cannot be empty.', got "1 validation error detected: Value '' at 'reason' failed to satisfy constraint: Member must have length greater than or equal to 1"
Call: signer.RevokeSignature(jobId=<job from SignPayload>, reason='') issued through a client configured with Config(parameter_validation=False) so the empty reason reaches the service. Error.Code was 'ValidationException' as expected; only the message diverges.

Model facts used — smithy_query query='operation' on services/service-com-amazonaws-signer-wallabyservice/smithy-model/com-amazonaws-signer-wallabyservice.json:
  - SignPayload: POST /signing-jobs/with-payload, code 200; input profileName(@required, length 2..64, pattern ^[a-zA-Z0-9_]{2,}$), payload(@required blob, length 1..4096), payloadFormat(@required), profileOwner(AccountId); errors AccessDeniedException(client,403), ValidationException(client,400), ResourceNotFoundException(client,404), TooManyRequestsException(client,429), InternalServiceErrorException(server,500).
  - ListSigningJobs: GET /signing-jobs; maxResults -> MaxResults with range {min:1,max:25}, httpQuery 'maxResults'; errors include ValidationException(client,400); paginated inputToken/outputToken nextToken, pageSize maxResults.
  - RevokeSignature: PUT /signing-jobs/{jobId}/revoke; reason -> RevocationReasonString, @required, length {min:1,max:500}; errors include ValidationException(client,400) and AccessDeniedException(client,403).
The model supplies the statuses, the error shapes and the constraints; the exact refusal sentences are AWS's wording for these error classes as the recordings show them.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite and filter to the 'signer' service heading.
3. Cause 1 — run check signer.sign-payload-missing-iam-permission-denied:
   a. signer.PutSigningProfile(profileName=<generated, matching ^[a-zA-Z0-9_]{2,}$>, platformId='Notation-OCI-SHA384-ECDSA').
   b. iam.CreateUser(UserName=<generated>) and attach NO policy whatsoever; iam.CreateAccessKey for that user.
   c. With those credentials, call signer.SignPayload(profileName=<the profile>, payload=<a notation payload blob>, payloadFormat='application/vnd.cncf.notary.payload.v1+json'), polling every 3s for up to 45s.
   d. Observe HTTP 200 with jobId/jobOwner/metadata/signature. Expected: HTTP 403, Error.Code 'AccessDeniedException', Error.Message "User: arn:aws:iam::<accountId>:user/<userName> is not authorized to perform: signer:SignPayload on resource: arn:aws:signer:<region>:<accountId>:/signing-profiles/<profileName> because no identity-based policy allows the signer:SignPayload action".
   e. Then verify the denied attempt created no job: signer.ListSigningJobs(status='Succeeded', jobInvoker=<accountId>, maxResults=25) must list no job for that profile.
   f. Regression guard: the identical SignPayload from the default fully-permissioned identity must still return a jobId, DescribeSigningJob must reach status 'Succeeded' with profileName set, and the job must appear via ListSigningJobs.
4. Cause 2 — run check signer.list-signing-jobs-maxresults-out-of-range-rejected: create a profile, SignPayload twice so >=2 jobs are visible, then call signer.ListSigningJobs(maxResults=26). Observe Error.Code 'ValidationException' (correct) with Error.Message "1 validation error detected: Value '26' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25". Expected Error.Message: "MaxResults parameter must be an integer between 1 and 25". Regression guard: ListSigningJobs(maxResults=1) must still return exactly one job plus a nextToken.
5. Cause 3 — run check signer.revoke-signature-empty-reason-rejected: create a profile, SignPayload to get a jobId, then call signer.RevokeSignature(jobId=<that job>, reason='') through a botocore client built with Config(parameter_validation=False) so the empty value reaches the service. Observe Error.Code 'ValidationException' (correct) with Error.Message "1 validation error detected: Value '' at 'reason' failed to satisfy constraint: Member must have length greater than or equal to 1". Expected Error.Message: "Reason cannot be empty.". Regression guards: DescribeSigningJob must show no revocationRecord after the rejected call, and RevokeSignature with a non-empty reason must then succeed and persist that reason.
