Issue-Source: trace-sweep-divergence
Issue-Trace: signer-error-paths-jobs-and-permissions
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signer-error-paths-jobs-and-permissions signer-error-paths-missing-resources signer-cross-account-profile-permissions signer-profile-revocation-workflow
Issue-Shape-Id: com.amazonaws.signer#WallabyService
Issue-Key: signer|AddProfilePermission|4xx->4xx|$.message
Issue-Key: signer|RemoveProfilePermission|4xx->4xx|$.code
Issue-Key: signer|RemoveProfilePermission|4xx->4xx|$.message
Issue-Key: signer|RevokeSignature|4xx->4xx|$.message
Issue-Key: signer|GetSigningPlatform|4xx->4xx|$.message
Issue-Key: signer|PutSigningProfile|4xx->4xx|$.code
Issue-Key: signer|PutSigningProfile|4xx->4xx|$.message
Issue-Key: signer|PutSigningProfile|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: signer|PutSigningProfile|4xx->4xx|status
Issue-Key: signer|ListTagsForResource|4xx->4xx|$.message
Issue-Key: signer|TagResource|4xx->4xx|$.message
Issue-Key: signer|ListProfilePermissions|4xx->2xx|$.code
Issue-Key: signer|ListProfilePermissions|4xx->2xx|$.message
Issue-Key: signer|ListProfilePermissions|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: signer|ListProfilePermissions|4xx->2xx|status
Issue-Key: signer|CancelSigningProfile|4xx->2xx|$
Issue-Key: signer|CancelSigningProfile|4xx->2xx|$headers.content-length
Issue-Key: signer|CancelSigningProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: signer|CancelSigningProfile|4xx->2xx|status
Issue-Commit: f04d881c92d4dd0b362060ab10db1f8e619a8cb7

# Signer error paths diverge from AWS in message wording, error code and status class

> _Note: this issue was identified against the code base at commit `f04d881c92d4dd0b362060ab10db1f8e619a8cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddProfilePermission

## Description
Nine net-new wire divergences in the signer crate's refusal paths share one root cause: the refusals are composed by the emulator in its own prose and its own classification instead of reproducing the service's error contract. Two of them are not refusals at all — the emulator answers a success status where AWS refuses. All nine are on the same service and must all be closed before this issue is submitted.

Cause 1 of 9: AddProfilePermission — unsupported action refusal (fixture signer-error-paths-jobs-and-permissions, step task#2).
AWS: recorded behavior only — the model gives ValidationException a free-form `message`/`code` pair and documents no text (Smithy: AddProfilePermission.action is a required String, errors ValidationException httpError 400). The recorded 400 reply words it: "Action signer:BogusAction not supported for platform AWSLambda-SHA384-ECDSA. Action must be one of [signer:StartSigningJob, signer:GetSigningProfile, signer:RevokeSignature]". Fixed text: "Action " … " not supported for platform " … ". Action must be one of [signer:StartSigningJob, signer:GetSigningProfile, signer:RevokeSignature]". Variable parts: the rejected value of the request's `action` member, and the platformId of the profile named in the request path. The allowed set is derived from that platform and is listed in the order StartSigningJob, GetSigningProfile, RevokeSignature.
Emulator: answers 400 but with "action signer:BogusAction is not an AWS Signer action that can be granted to another account; expected one of signer:GetSigningProfile, signer:RevokeSignature, signer:SignPayload, signer:StartSigningJob" — emulator-authored prose, lower-case opener, no platform named, and a global alphabetised four-action list that includes signer:SignPayload.
Why: a client that reads the allowed-action list out of the refusal concludes signer:SignPayload is grantable cross-account and never learns which platform narrowed the set, so it retries with an action AWS will refuse again.
Fix: AddProfilePermission's action validation in the signer crate — resolve the allowed actions from the profile's signing platform and emit AWS's message template.

Cause 2 of 9: RemoveProfilePermission — stale revisionId refusal carries the wrong code (fixture signer-error-paths-jobs-and-permissions, step task#4).
AWS: 409 (Smithy: RemoveProfilePermission error ConflictException, httpError 409, members message + code) with code "PolicyRevisionIdMismatch" and message "Specified revisionId (stale-revision-id) does not match the revisionId of the existing policy (00000000-0000-0002-0000-000000000001)". Fixed text: "Specified revisionId (" … ") does not match the revisionId of the existing policy (" … ")". Variable parts: the revisionId supplied on the request, and the policy's current revision id read from state.
Emulator: right status (409) but code "ConflictException" — the Smithy shape name rather than signer's error code — and message "revision id stale-revision-id is not the current revision of the permissions of profile <profileName>".
Why: clients branch on the `code` member, so PolicyRevisionIdMismatch is unreachable, and the message withholds the current revision id a caller needs to retry the removal.
Fix: RemoveProfilePermission's revision check and its ConflictException construction (set `code`, format AWS's message).

Cause 3 of 9: RevokeSignature — unknown job message (fixture signer-error-paths-jobs-and-permissions, step task#7).
AWS: 404 ResourceNotFoundException (Smithy: httpError 404) with message "Signing Job with id 00000000-0000-0000-0000-000000000000 does not exist." Fixed text: "Signing Job with id " … " does not exist.", including the trailing period and the capitalised "Signing Job". Variable part: the `jobId` path label from the request.
Emulator: same 404 but "Signing job not found for id <jobId>".
Why: message text is part of the error contract; a client or test matching AWS's wording does not match.
Fix: the not-found path of the RevokeSignature handler.

Cause 4 of 9: GetSigningPlatform — unknown platform message (fixture signer-error-paths-missing-resources, step task#3).
AWS: 404 ResourceNotFoundException with message "Signing platform not found for ID NoSuchPlatform-Probe". Fixed text: "Signing platform not found for ID "; variable part: the `platformId` path label.
Emulator: 404 with "Platform with id NoSuchPlatform-Probe does not exist".
Why: same contract break as above, on the platform catalog reader.
Fix: GetSigningPlatform's not-found path.

Cause 5 of 9: PutSigningProfile — validation precedence inverted (fixture signer-error-paths-missing-resources, step task#4).
AWS: the request named an unknown platformId AND omitted signingMaterial; AWS validates the input members first and answers 400 ValidationException, code "ValidationException", header x-amzn-errortype: ValidationException, message "signing material cannot be null." — that text is entirely fixed, with no request value in it (Smithy: PutSigningProfile.signingMaterial is an optional member whose absence the service still refuses for this platform class; errors ValidationException 400 and ResourceNotFoundException 404).
Emulator: resolves the platform catalog first and answers 404, code "PlatformNotFound", x-amzn-errortype: ResourceNotFoundException, message "Platform with id NoSuchPlatform-Probe does not exist".
Why: the whole error class and status change (400 fix-the-request vs 404 missing-resource), so a client classifies a malformed request as a missing platform and never learns which member it left out.
Fix: PutSigningProfile's validation ordering — required/conditional member validation runs before signing-platform resolution.

Cause 6 of 9: ListTagsForResource — malformed-ARN message echoes the request (fixture signer-error-paths-missing-resources, step task#5).
AWS: 400 BadRequestException (Smithy: ListTagsForResource errors BadRequestException httpError 400, documented for "invalid parameters for the ARN or tags") with message "Invalid input resource arn: bad syntax". The whole string is fixed text: AWS does NOT echo the supplied ARN.
Emulator: 400 with "not-a-valid-signer-arn is not a valid signing profile resource ARN", echoing the caller's value.
Why: wrong wording, and echoing unbounded caller input into a refusal is the same no-echo rule the crate states elsewhere.
Fix: the ARN-parse refusal in the tagging readers.

Cause 7 of 9: TagResource — same malformed-ARN message defect (fixture signer-error-paths-missing-resources, step task#6).
AWS: 400 BadRequestException with the identical fixed message "Invalid input resource arn: bad syntax".
Emulator: 400 with "not-a-valid-signer-arn is not a valid signing profile resource ARN".
Why/Fix: as cause 6 — the wording is wrong in every tagging operation that parses resourceArn. The open issue 20261029T133741Z-signer-tagresource-maintainability.md notes that the TaggingArnProblem→error mapping is written out three times; it is about duplication and does not cover this wording, but the duplication is why the same wrong text appears in each copy.

Cause 8 of 9: ListProfilePermissions — emptied policy answered 200 instead of 404 PolicyNotFound (fixture signer-cross-account-profile-permissions, step task#5).
AWS: once the profile's last permission statement has been removed, the resource policy does not exist, and the read answers 404 ResourceNotFoundException (Smithy: ListProfilePermissions errors ResourceNotFoundException httpError 404) with code "PolicyNotFound", message "No policies associated with profile sim2real_probe_signer_perm_profile_76f51a35", and an x-amzn-errortype header. Fixed text: "No policies associated with profile "; variable part: the `profileName` path label.
Emulator: answers 200 with a body that carries no code/message at all — the gate never fires.
Why: a client that removes the last statement and re-reads sees an existing-but-empty policy instead of PolicyNotFound, so it cannot tell "policy gone" from "policy with no statements" and skips the re-create it should do.
Fix: the ListProfilePermissions handler's absent-policy gate (and the state that should drop the policy when its last statement is removed). Related but distinct: open issue 20261028T170733Z-signer-listprofilepermissions-pagination.md covers a nextToken that becomes unusable after a statement is removed.

Cause 9 of 9: CancelSigningProfile — revoked profile cancelled with 204 instead of refused 400 (fixture signer-profile-revocation-workflow, step teardown#6).
AWS: a revoked signing profile cannot be cancelled; the call answers 400 with code "ProfileRevoked" and message "Signing Profile sim2real_probe_signer_revoke_profile_3b769ce6 cannot be canceled because it has been revoked. A revoked profile cannot be canceled." Fixed text: "Signing Profile " … " cannot be canceled because it has been revoked. A revoked profile cannot be canceled."; variable part: the `profileName` path label. The reply carries x-amzn-errortype and content-length.
Emulator: answers 204 with an empty body, no error code, no x-amzn-errortype — it performs the cancellation.
Why: the profile's state machine diverges — a revoked profile silently becomes cancelled, so subsequent reads report a state AWS would never reach, and a client never sees the ProfileRevoked refusal it must handle.
Fix: CancelSigningProfile's state gate in the signer crate's profile handler. Note the resolved issue 20261024T155739Z-signer-cancelsigningprofile-error-response.md removed a ValidationException the operation does not model from this path; the gate must be restored as a 400 carrying code "ProfileRevoked", not removed.

Traces: signer-error-paths-jobs-and-permissions#task#2 (representative), signer-error-paths-jobs-and-permissions#task#4, signer-error-paths-jobs-and-permissions#task#7, signer-error-paths-missing-resources#task#3, signer-error-paths-missing-resources#task#4, signer-error-paths-missing-resources#task#5, signer-error-paths-missing-resources#task#6, signer-cross-account-profile-permissions#task#5, signer-profile-revocation-workflow#teardown#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (9 fixtures, 49 steps, score 55.1%).

Cause 1 — AddProfilePermission, signer-error-paths-jobs-and-permissions#task#2, status 400 expected / 400 actual:
  $.message: expected 'Action signer:BogusAction not supported for platform AWSLambda-SHA384-ECDSA. Action must be one of [signer:StartSigningJob, signer:GetSigningProfile, signer:RevokeSignature]', got 'action signer:BogusAction is not an AWS Signer action that can be granted to another account; expected one of signer:GetSigningProfile, signer:RevokeSignature, signer:SignPayload, signer:StartSigningJob'

Cause 2 — RemoveProfilePermission, signer-error-paths-jobs-and-permissions#task#4, status 409/409:
  $.code: expected 'PolicyRevisionIdMismatch', got 'ConflictException'
  $.message: expected 'Specified revisionId (stale-revision-id) does not match the revisionId of the existing policy (00000000-0000-0002-0000-000000000001)', got 'revision id stale-revision-id is not the current revision of the permissions of profile sim2real_probe_signer_errperm_profile_994b0a57'

Cause 3 — RevokeSignature, signer-error-paths-jobs-and-permissions#task#7, status 404/404:
  $.message: expected 'Signing Job with id 00000000-0000-0000-0000-000000000000 does not exist.', got 'Signing job not found for id 00000000-0000-0000-0000-000000000000'

Cause 4 — GetSigningPlatform, signer-error-paths-missing-resources#task#3, status 404/404:
  $.message: expected 'Signing platform not found for ID NoSuchPlatform-Probe', got 'Platform with id NoSuchPlatform-Probe does not exist'

Cause 5 — PutSigningProfile, signer-error-paths-missing-resources#task#4:
  status: expected 400, got 404
  $.code: expected 'ValidationException', got 'PlatformNotFound'
  $.message: expected 'signing material cannot be null.', got 'Platform with id NoSuchPlatform-Probe does not exist'
  $headers.x-amzn-errortype: expected 'ValidationException', got 'ResourceNotFoundException'

Cause 6 — ListTagsForResource, signer-error-paths-missing-resources#task#5, status 400/400:
  $.message: expected 'Invalid input resource arn: bad syntax', got 'not-a-valid-signer-arn is not a valid signing profile resource ARN'

Cause 7 — TagResource, signer-error-paths-missing-resources#task#6, status 400/400:
  $.message: expected 'Invalid input resource arn: bad syntax', got 'not-a-valid-signer-arn is not a valid signing profile resource ARN'

Cause 8 — ListProfilePermissions, signer-cross-account-profile-permissions#task#5:
  status: expected 404, got 200
  $.code: expected 'PolicyNotFound', got None (missing)
  $.message: expected 'No policies associated with profile sim2real_probe_signer_perm_profile_76f51a35', got None (missing)
  $headers.x-amzn-errortype: expected (shaped, not asserted), got None
  Also affected: signer-error-paths-jobs-and-permissions#task#6 (same cluster)

Cause 9 — CancelSigningProfile, signer-profile-revocation-workflow#teardown#6:
  status: expected 400, got 204
  $: expected {'code': 'ProfileRevoked', 'message': 'Signing Profile sim2real_probe_signer_revoke_profile_3b769ce6 cannot be canceled because it has been revoked. A revoked profile cannot be canceled.'}, got {} (empty body)
  $headers.x-amzn-errortype and $headers.content-length: expected present, got None

Model facts used: Smithy signer (sdkId signer, restJson1, service WallabyService) — ValidationException/BadRequestException httpError 400, ResourceNotFoundException 404, ConflictException 409, each with members {message, code}; AddProfilePermission.action required String; ListProfilePermissions errors include ResourceNotFoundException(404); PutSigningProfile errors include ValidationException(400) and ResourceNotFoundException(404); ListTagsForResource/TagResource errors BadRequestException(400)/NotFoundException(404).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Cause 1: replay signer-error-paths-jobs-and-permissions step task#2 — AddProfilePermission on a profile on platform AWSLambda-SHA384-ECDSA with action 'signer:BogusAction'; compare $.message.
4. Cause 2: same fixture, step task#4 — RemoveProfilePermission with revisionId=stale-revision-id against a profile whose policy revision differs; compare $.code and $.message.
5. Cause 3: same fixture, step task#7 — RevokeSignature on jobId 00000000-0000-0000-0000-000000000000; compare $.message.
6. Cause 4: replay signer-error-paths-missing-resources step task#3 — GetSigningPlatform on platformId NoSuchPlatform-Probe; compare $.message.
7. Cause 5: same fixture, step task#4 — PutSigningProfile with an unknown platformId and no signingMaterial; compare status, $.code, $.message, x-amzn-errortype.
8. Causes 6 and 7: same fixture, steps task#5 and task#6 — ListTagsForResource and TagResource with resourceArn 'not-a-valid-signer-arn'; compare $.message.
9. Cause 8: replay signer-cross-account-profile-permissions through step task#5 — add a permission, remove it, then ListProfilePermissions on the same profile; compare status, $.code, $.message.
10. Cause 9: replay signer-profile-revocation-workflow through teardown#6 — revoke the profile, then CancelSigningProfile on it; compare status and body.
