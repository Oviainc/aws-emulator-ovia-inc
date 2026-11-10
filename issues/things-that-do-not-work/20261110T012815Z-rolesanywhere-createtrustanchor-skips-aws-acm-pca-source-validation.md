Issue-Source: trace-sweep-divergence
Issue-Trace: rolesanywhere-validation-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rolesanywhere-validation-error-paths
Issue-Shape-Id: com.amazonaws.rolesanywhere#RolesAnywhere
Issue-Key: rolesanywhere|CreateTrustAnchor|4xx->2xx|$.trustAnchor.notificationSettings[*].configuredBy
Issue-Related: 20261029T192145Z-rolesanywhere-validation-refusals-render-patterns-and-enum-sets.md
Issue-Commit: d9137e240d6d86ce2d0eba5736851daf32b7e9eb

# rolesanywhere CreateTrustAnchor skips AWS_ACM_PCA source validation and answers 201 instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `d9137e240d6d86ce2d0eba5736851daf32b7e9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrustAnchor

## Description
One cause: CreateTrustAnchor never validates the trust anchor's `source` before writing state, so a request AWS refuses is accepted and an unusable trust anchor is minted.

Cause 1 of 1 (CreateTrustAnchor, fixture rolesanywhere-validation-error-paths step task#5):

AWS: CreateTrustAnchor takes a required `source` member targeting the Source structure {sourceType: TrustAnchorType (AWS_ACM_PCA | CERTIFICATE_BUNDLE | SELF_SIGNED_REPOSITORY), sourceData: SourceData}, and models ValidationException with httpError 400 (Smithy: CreateTrustAnchor.source @required -> Source; CreateTrustAnchor errors AccessDeniedException httpError 403, ValidationException httpError 400; http {method: POST, uri: /trustanchors, code: 201}). AWS resolves the source BEFORE creating anything: when sourceType is AWS_ACM_PCA and the sourceData CA ARN is malformed or names a private CA the service cannot read, the call is refused with HTTP 400, error code ValidationException, and the message 'Error creating TrustAnchor. Given AWS PCA did not allow get request, or invalid ARN.' That message is entirely fixed text for this error class - it interpolates nothing from the request, so the submitted ARN does not appear in it. The refusal also carries the x-amzn-errortype response header naming ValidationException (the recorded response carries it with the coral shape suffix, i.e. 'ValidationException:<coral shape namespace>'; the requirement is the ValidationException error code in that header, not the recording's particular suffix string). No trust anchor is created and no id is allocated.

Emulator: answers HTTP 201 with a fully populated trustAnchor body - no `message`, no x-amzn-errortype header - and stores the anchor. The source member is never resolved or validated, so the AWS_ACM_PCA negative path does not exist in the emulator at all. (The body's trustAnchor.notificationSettings[*].configuredBy = 'rolesanywhere.amazonaws.com' values flagged by the replay are not a separate defect: that is the documented value for IAM Roles Anywhere's default notification settings (Smithy: NotificationSettingDetail.configuredBy doc). They diverge only because this response body should not exist at all.)

Why: this divergence changes the OUTCOME, not just wording. A client that relies on the service to reject a bad private-CA reference is told its trust anchor was created successfully and receives a trustAnchorId; against real AWS it would have failed fast with ValidationException. The failure surfaces much later, when X.509 certificate-based sessions cannot be vended against the unreadable CA. Conversely, any test or deployment guard that asserts the bad-ARN path is refused cannot be exercised against the emulator, because the emulator reports success for it.

Fix: the CreateTrustAnchor handler's input validation - resolve and validate the `source` member (for sourceType AWS_ACM_PCA, reject a malformed or unreadable CA ARN) and return 400 ValidationException with the fixed message above plus the x-amzn-errortype header, BEFORE any state write, so no trust anchor id is allocated on the refusal path.

Related open issue: '20261029T192145Z-rolesanywhere-validation-refusals-render-patterns-and-enum-sets.md' describes this same PCA source check as its cause 2, but its replay pointer binds only Issue-Trace rolesanywhere-validation-error-paths / Issue-Step task#4 and its Issue-Trace-Ids entry 'rolesanywhere-validation-error-paths' carries no step half, so step task#5 is not covered by any open issue's replay key (the replay reports owned_by empty for this cluster). This issue exists to carry that key so the fix is actually re-replayed on task#5.

Traces: rolesanywhere-validation-error-paths#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 1 net-new mismatch cluster (owned_by empty, stub_path empty).

Fixture rolesanywhere-validation-error-paths, step task#5, operation CreateTrustAnchor, verdict mismatch, expected_status 400 / actual_status 201:
  status: expected 400, got 201
  $.message: missing (expected 'Error creating TrustAnchor. Given AWS PCA did not allow get request, or invalid ARN.', got None)
  $headers.x-amzn-errortype: missing (expected 'ValidationException:<coral shape namespace for the rolesanywhere service>', got None)
  $.trustAnchor.notificationSettings[0].configuredBy: host_not_own (expected None, got 'rolesanywhere.amazonaws.com')
  $.trustAnchor.notificationSettings[1].configuredBy: host_not_own (expected None, got 'rolesanywhere.amazonaws.com')

The two configuredBy entries are flagged only because the recorded response is a 400 error with no trustAnchor body; 'rolesanywhere.amazonaws.com' is itself the documented value for default notification settings and is not the divergence.

Requirement split for the diverging message: the whole string 'Error creating TrustAnchor. Given AWS PCA did not allow get request, or invalid ARN.' is fixed text for this error class - there are no variable parts taken from the request or from emulator state. The status (400) and error code (ValidationException, also surfaced in x-amzn-errortype) are likewise requirements.

## Steps to Reproduce
1. Build the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus against it.
2. Replay fixture rolesanywhere-validation-error-paths and watch step task#5: POST /trustanchors with a required `source` whose sourceType is AWS_ACM_PCA and whose sourceData names a malformed or unreadable private-CA ARN.
3. Observe the emulator answers 201 Created with a trustAnchor body and no x-amzn-errortype header, and that the anchor is persisted (a following GetTrustAnchor/ListTrustAnchors returns it).
4. Expected: HTTP 400 with error code ValidationException, x-amzn-errortype naming ValidationException, body message 'Error creating TrustAnchor. Given AWS PCA did not allow get request, or invalid ARN.', and no trust anchor created.
