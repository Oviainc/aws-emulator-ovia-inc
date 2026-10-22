Issue-Source: trace-sweep-divergence
Issue-Trace: signer-error-paths-missing-resources
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signer-error-paths-missing-resources signer-error-paths-jobs-and-permissions signer-profile-revocation-workflow signer-certificate-hashes-empty-list-rejected
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer|GetSigningProfile|4xx->4xx|$.code
Issue-Key: signer|GetSigningProfile|4xx->4xx|$.message
Issue-Key: signer|DescribeSigningJob|4xx->4xx|$.code
Issue-Key: signer|DescribeSigningJob|4xx->4xx|$.message
Issue-Key: signer|ListSigningProfiles|4xx->4xx|$.code
Issue-Key: signer|ListSigningProfiles|4xx->4xx|$.message
Issue-Key: signer|PutSigningProfile|4xx->2xx|$.code
Issue-Key: signer|PutSigningProfile|4xx->2xx|$.message
Issue-Key: signer|PutSigningProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: signer|PutSigningProfile|4xx->2xx|status
Issue-Key: signer|CancelSigningProfile|4xx->2xx|$
Issue-Key: signer|CancelSigningProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: signer|CancelSigningProfile|4xx->2xx|status
Issue-Key: signer|GetRevocationStatus|4xx->4xx|$.message
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# Signer error responses omit the modeled code member and error-path validation never fires

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSigningProfile

## Description
Six distinct error-path defects in the signer emulator, all on the same fault: the error path either serializes an incomplete error body (the modeled `code` member is never emitted) or never fires at all (an invalid or illegal request is answered 200). A fixer must close ALL SIX causes below before submitting.

Cause 1 of 6: GetSigningProfile on a non-existent profile returns a 404 body with no `code` member.
AWS: Smithy (services/service-com-amazonaws-signer-wallabyservice) GetSigningProfile errors include ResourceNotFoundException @httpError 404, and that shape models BOTH members: `message` (ErrorMessage) and `code` (ErrorCode). The recorded response is status 404 with code `ProfileNotFound` (message e.g. 'profile with name <name> does not exists' — recorded example).
Emulator: status 404 is correct, but `$.code` is absent entirely (expected 'ProfileNotFound', got None).
Why: SDKs and clients branch on the error code, not on prose. With `code` missing the client cannot distinguish a missing profile from any other 404, so retry/create-if-absent logic takes the wrong branch.
Fix: the GetSigningProfile handler's error serializer — populate the modeled `code` member on ResourceNotFoundException.
Traces: signer-error-paths-missing-resources#task#1

Cause 2 of 6: DescribeSigningJob on a non-existent job id returns a 404 body with no `code` member.
AWS: Smithy DescribeSigningJob errors include ResourceNotFoundException @httpError 404 with members `message` + `code`. Recorded: status 404, code `JobNotFound` (message e.g. 'Signing job not found for id <id>' — recorded example).
Emulator: status 404 is correct, `$.code` absent (expected 'JobNotFound', got None).
Why: same as Cause 1 — the code is the only machine-readable discriminator, and signer uses a job-specific code distinct from the profile one.
Fix: the DescribeSigningJob handler's error serializer.
Traces: signer-error-paths-missing-resources#task#2

Cause 3 of 6: ListSigningProfiles with an unrecognised nextToken returns 400 with no `code` member.
AWS: recorded behavior only for the code value — ListSigningProfiles models only AccessDeniedException/InternalServiceErrorException/TooManyRequestsException, and the model does not name a pagination error, but all signer error shapes model `code` (ErrorCode). The recording shows status 400 with code `InvalidNextToken` (message 'Invalid next token provided' — recorded example).
Emulator: status 400 is correct; `$.code` is absent (expected 'InvalidNextToken', got None). The emulator's message is a different, request-derived text; the message alone is not the requirement here, the missing code is.
Why: a paginating client that retries or resets on InvalidNextToken has nothing to match on and treats the failure as generic/retryable.
Fix: the ListSigningProfiles pagination validation + its error serializer.
Traces: signer-error-paths-jobs-and-permissions#task#9

Cause 4 of 6: PutSigningProfile with no signing material is ACCEPTED with 200 instead of rejected 400.
AWS: Smithy PutSigningProfile errors include ValidationException @httpError 400 (members `message` + `code`); `signingMaterial` is an optional member in the model, so this is recorded behavior for the rule: the recording shows status 400, code `ValidationException`, header x-amzn-errortype: ValidationException, message 'signing material cannot be null.' (recorded example) when a profile is created with no signing material.
Emulator: returns 200 and creates the profile — no status, no `$.code`, no x-amzn-errortype header (expected 400/ValidationException, got 200 and None).
Why: the most damaging class of divergence — an invalid request silently succeeds, so a client's error handling is never exercised and a profile exists in the emulator that could not exist on AWS, corrupting every downstream signing step built on it.
Fix: PutSigningProfile input validation (reject the request before state mutation) plus the ValidationException serializer, including the x-amzn-errortype header.
Traces: signer-error-paths-missing-resources#task#4

Cause 5 of 6: CancelSigningProfile on a REVOKED profile succeeds with 200 instead of being refused 400.
AWS: recorded behavior only — the model binds no revoked-state error to CancelSigningProfile. The recording shows status 400 with code `ProfileRevoked` and an x-amzn-errortype header (message e.g. 'Signing Profile <name> cannot be canceled because it has been revoked. A revoked profile cannot be canceled.' — recorded example). The rule: a profile whose status is Revoked (SigningProfileStatus enum: Active|Canceled|Revoked) cannot be canceled.
Emulator: returns 200 with an empty body; no error code, no x-amzn-errortype header.
Why: the revoked->canceled transition is illegal on AWS. A client that treats cancel-after-revoke as an error (or that relies on it failing to detect a revoked profile) sees success, and the emulator's profile ends in a state AWS cannot reach.
Fix: CancelSigningProfile handler — gate on the profile's status in state and raise the ProfileRevoked error. Depends on revocation actually being recorded (see the companion profile-state issue).
Traces: signer-profile-revocation-workflow#teardown#6

Cause 6 of 6: GetRevocationStatus with an EMPTY certificateHashes list is rejected for the wrong constraint.
AWS: Smithy GetRevocationStatus.certificateHashes is @required, bound @httpQuery 'certificateHashes', and the op models ValidationException @httpError 400. Recorded: status 400 whose message identifies the member and the constraint it actually violated — the list is present but empty ('certificateHashes must not be null or empty' — recorded example of that rule).
Emulator: status 400 is correct, but the body reports the member as NULL ('1 validation error detected: Value null at \u0027certificateHashes\u0027 failed to satisfy constraint: Member must not be null'), i.e. the generic required-member message fires for a supplied-but-empty list. The requirement is the rule, not the recorded text: the 400 must name certificateHashes and the constraint that was actually violated (empty), not claim it was absent.
Why: a client that distinguishes 'you forgot the parameter' from 'you sent nothing in it' is told the wrong thing, and an empty-list guard in the emulator's validation is evidently collapsed into the null check.
Fix: GetRevocationStatus input validation — separate the empty-list constraint from the required/null constraint.
Traces: signer-certificate-hashes-empty-list-rejected#task#2

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0, so these are not reset artefacts).

Cause 1 — signer-error-paths-missing-resources#task#1, GetSigningProfile: expected 404, got 404; $.code: missing (expected 'ProfileNotFound', got None); $.message differs (recorded example only).
Cause 2 — signer-error-paths-missing-resources#task#2, DescribeSigningJob: expected 404, got 404; $.code: missing (expected 'JobNotFound', got None).
Cause 3 — signer-error-paths-jobs-and-permissions#task#9, ListSigningProfiles: expected 400, got 400; $.code: missing (expected 'InvalidNextToken', got None).
Cause 4 — signer-error-paths-missing-resources#task#4, PutSigningProfile: status expected 400, got 200; $.code missing (expected 'ValidationException'); $headers.x-amzn-errortype missing (expected 'ValidationException'); $.message missing.
Cause 5 — signer-profile-revocation-workflow#teardown#6, CancelSigningProfile: status expected 400, got 200; body empty_body (expected {'code': 'ProfileRevoked', ...}, got {}); $headers.x-amzn-errortype missing.
Cause 6 — signer-certificate-hashes-empty-list-rejected#task#2, GetRevocationStatus: expected 400, got 400; $.message value_mismatch — recorded names the empty-list constraint, emulator reports the member as null.

Model evidence (smithy_query on services/service-com-amazonaws-signer-wallabyservice/smithy-model/com-amazonaws-signer-wallabyservice.json): ResourceNotFoundException {message: ErrorMessage, code: ErrorCode} @httpError 404; ValidationException {message: ErrorMessage, code: ErrorCode} @httpError 400; AccessDeniedException likewise — `code` is modeled on every signer error shape, so omitting it is a dropped modeled member, not a message-wording difference.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: GET /signing-profiles/<name> for a profile that was never created -> observe 404 with no `code` in the JSON body (fixture signer-error-paths-missing-resources, step task#1).
3. Cause 2: GET /signing-jobs/00000000-0000-0000-0000-000000000000 -> 404 with no `code` (same fixture, step task#2).
4. Cause 3: GET /signing-profiles?nextToken=not-a-valid-pagination-token -> 400 with no `code` (fixture signer-error-paths-jobs-and-permissions, step task#9).
5. Cause 4: PUT /signing-profiles/<name> with platformId set and no signingMaterial -> emulator answers 200 and creates the profile; expected 400 ValidationException with x-amzn-errortype (fixture signer-error-paths-missing-resources, step task#4).
6. Cause 5: create a profile, RevokeSigningProfile it, then DELETE /signing-profiles/<name> -> emulator answers 200; expected 400 ProfileRevoked (fixture signer-profile-revocation-workflow, step teardown#6).
7. Cause 6: GET /revocations with certificateHashes supplied as an empty list -> 400 whose message reports the member as null instead of naming the empty-list constraint (fixture signer-certificate-hashes-empty-list-rejected, step task#2).
