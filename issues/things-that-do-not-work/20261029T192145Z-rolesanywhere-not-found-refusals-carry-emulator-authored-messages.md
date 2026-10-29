Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-crl-lifecycle
Issue-Step: teardown#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-crl-lifecycle rolesanywhere-not-found-error-paths rolesanywhere-trust-anchor-lifecycle rolesanywhere-tag-lifecycle
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|GetTrustAnchor|4xx->4xx|$.message
Issue-Key: rolesanywhere|GetProfile|4xx->4xx|$.message
Issue-Key: rolesanywhere|DeleteProfile|4xx->4xx|$.message
Issue-Key: rolesanywhere|DeleteTrustAnchor|4xx->4xx|$.message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# rolesanywhere not-found refusals carry emulator-authored messages instead of AWS wording

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrustAnchor

## Description
Every ResourceNotFoundException the emulator raises uses hand-written prose rather than the fixed message AWS words for that error class. The status (404) and the error class are right; only the message text diverges, and it diverges differently for trust anchors than for profiles. Four causes, one per failing replay cluster.

Cause 1 of 4 (GetTrustAnchor, fixture rolesanywhere-crl-lifecycle step teardown#11):
AWS: GetTrustAnchor on an unknown trustAnchorId answers 404 ResourceNotFoundException (Smithy: GetTrustAnchor errors include ResourceNotFoundException, httpError 404, doc 'The resource could not be found'). Its message is the FIXED text 'TrustAnchor not found' - there is no variable part at all: the requested id is NOT interpolated.
Emulator: 404 ResourceNotFoundException with 'Trust anchor <trustAnchorId> not found' - different fixed text ('Trust anchor' with a space and lower-case 'a', the word order 'not found' after an interpolated id) plus an id AWS does not include.
Why: a client that matches on the service's message text to distinguish a missing trust anchor from another not-found condition never matches, and anything that logs or asserts the refusal reason records a message no AWS deployment produces.
Fix: the not-found error construction on the trust-anchor read path (GetTrustAnchor handler / the service's error channel), which must emit the fixed string with no id interpolation.
Traces: rolesanywhere-crl-lifecycle#teardown#11 (cluster also covers rolesanywhere-not-found-error-paths#task#2 and rolesanywhere-trust-anchor-lifecycle#teardown#8).

Cause 2 of 4 (GetProfile, fixture rolesanywhere-not-found-error-paths step task#1):
AWS: GetProfile on an unknown profileId answers 404 ResourceNotFoundException (Smithy: GetProfile errors include ResourceNotFoundException, httpError 404) with the message "The item doesn't exist. arn: <profile-arn> ; aborting." The fixed text is "The item doesn't exist. arn:", then a space, then the ARN, then " ; aborting."; the only variable part is the profile ARN built from the requested profileId - arn:aws:rolesanywhere:<region>:<account-id>:profile/<profileId> (Smithy: ProfileArn @pattern ^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:profile.*)$), whose region and account come from the endpoint and caller.
Emulator: 404 with 'Profile <profileId> not found' - none of AWS's fixed text, and the bare id where AWS reports the full ARN.
Why: the profile not-found message has a different shape from the trust-anchor one in AWS, and clients (and humans reading logs) rely on the ARN it carries to identify which profile was missing; the emulator's wording supplies neither.
Fix: the not-found error construction on the profile read path (GetProfile handler / the service's error channel), including composing the profile ARN from the requested id.
Traces: rolesanywhere-not-found-error-paths#task#1 (cluster also covers rolesanywhere-tag-lifecycle#teardown#9).

Cause 3 of 4 (DeleteProfile, fixture rolesanywhere-not-found-error-paths step task#4):
AWS: DeleteProfile on an unknown profileId answers 404 ResourceNotFoundException (Smithy: DeleteProfile errors include ResourceNotFoundException, httpError 404) with the SAME message as the profile read path: "The item doesn't exist. arn: <profile-arn> ; aborting.", the ARN built from the requested profileId.
Emulator: 404 with 'Profile <profileId> not found'.
Why: as cause 2, on the delete path - a client cleaning up and tolerating a missing profile cannot recognise the refusal by its message.
Fix: the DeleteProfile handler's not-found path must use the same AWS-worded ARN message as GetProfile.
Traces: rolesanywhere-not-found-error-paths#task#4.

Cause 4 of 4 (DeleteTrustAnchor, fixture rolesanywhere-not-found-error-paths step task#5):
AWS: DeleteTrustAnchor on an unknown trustAnchorId answers 404 ResourceNotFoundException (Smithy: DeleteTrustAnchor errors include ResourceNotFoundException, httpError 404) with the fixed message 'TrustAnchor not found', with no id interpolated.
Emulator: 404 with 'Trust anchor <trustAnchorId> not found'.
Why: as cause 1, on the delete path.
Fix: the DeleteTrustAnchor handler's not-found path must emit the same fixed string as GetTrustAnchor.
Traces: rolesanywhere-not-found-error-paths#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator. Four mismatch clusters; in all four the expected and actual status are both 404 and only $.message diverges.

Cause 1 - rolesanywhere-crl-lifecycle#teardown#11 GetTrustAnchor, 404/404 (affected also: rolesanywhere-not-found-error-paths#task#2, rolesanywhere-trust-anchor-lifecycle#teardown#8):
  $.message: value_mismatch (expected 'TrustAnchor not found', got 'Trust anchor <trustAnchorId> not found')

Cause 2 - rolesanywhere-not-found-error-paths#task#1 GetProfile, 404/404 (affected also: rolesanywhere-tag-lifecycle#teardown#9):
  $.message: value_mismatch (expected "The item doesn't exist. arn: arn:aws:rolesanywhere:<region>:<account>:profile/<profileId> ; aborting.", got 'Profile <profileId> not found')

Cause 3 - rolesanywhere-not-found-error-paths#task#4 DeleteProfile, 404/404:
  $.message: value_mismatch (expected "The item doesn't exist. arn: arn:aws:rolesanywhere:<region>:<account>:profile/<profileId> ; aborting.", got 'Profile <profileId> not found')

Cause 4 - rolesanywhere-not-found-error-paths#task#5 DeleteTrustAnchor, 404/404:
  $.message: value_mismatch (expected 'TrustAnchor not found', got 'Trust anchor <trustAnchorId> not found')

Fixed vs variable text: the trust-anchor message is entirely fixed ('TrustAnchor not found'); the profile message's fixed parts are "The item doesn't exist. arn:" and " ; aborting.", and its single variable part is the profile ARN derived from the requested profileId plus the endpoint's region and the caller's account id.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Cause 1: GET /trustanchor/{trustAnchorId} for an id that does not exist (e.g. after the anchor has been deleted) - fixture rolesanywhere-crl-lifecycle step teardown#11. Compare $.message with 'TrustAnchor not found'.
3. Cause 2: GET /profile/{profileId} for an unknown id - fixture rolesanywhere-not-found-error-paths step task#1.
4. Cause 3: DELETE /profile/{profileId} for an unknown id - same fixture, step task#4.
5. Cause 4: DELETE /trustanchor/{trustAnchorId} for an unknown id - same fixture, step task#5.
