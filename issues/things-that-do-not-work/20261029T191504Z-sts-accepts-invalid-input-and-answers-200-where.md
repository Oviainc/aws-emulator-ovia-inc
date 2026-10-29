Issue-Source: trace-sweep-divergence
Issue-Trace: sts-access-key-info-lookup
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sts-access-key-info-lookup sts-get-session-token-invalid-mfa
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts|GetAccessKeyInfo|4xx->2xx|$.ErrorResponse
Issue-Key: sts|GetAccessKeyInfo|4xx->2xx|status
Issue-Key: sts|GetSessionToken|4xx->2xx|$.ErrorResponse
Issue-Key: sts|GetSessionToken|4xx->2xx|status
Issue-Commit: 0ebbd81840eb1d8236e49c909db275b2251d47fa

# STS accepts invalid input and answers 200 where AWS refuses the request

> _Note: this issue was identified against the code base at commit `0ebbd81840eb1d8236e49c909db275b2251d47fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessKeyInfo

## Description
Two STS operations have no admission check at all: a request AWS refuses is served as a success, so both the status and the whole error envelope are absent. The shared fault is missing request validation on the handler's entry path. 2 causes, both on service sts.

Cause 1 of 2: GetAccessKeyInfo answers 200 for an access key id that is not a valid key id, instead of 400 ValidationError.
AWS: Smithy: GetAccessKeyInfo.AccessKeyId is required, target accessKeyIdType, @length min 16 max 128, @pattern ^[\w]*$; the operation models no operation-specific errors, so the refusal is the service-common client error. The recorded response is HTTP 400 with <ErrorResponse><Error><Type>Sender</Type><Code>ValidationError</Code><Message>Access key ID is not valid.</Message></Error><RequestId>...</RequestId></ErrorResponse>. The whole message 'Access key ID is not valid.' is fixed text — no part of it is taken from the request, so the rejected key id is never echoed.
Emulator: HTTP 200 with a GetAccessKeyInfoResponse body; $.ErrorResponse is missing entirely. The handler resolves an Account for a key id it should have rejected.
Why: a caller using GetAccessKeyInfo to decide whether a key id is well formed and which account owns it gets a fabricated account for garbage input, and code that catches the ValidationError to branch (for example, to report 'not an access key') never sees it; validation bugs in the caller stay invisible against the emulator and surface only against AWS.
Fix: the GetAccessKeyInfo handler's input validation — reject an AccessKeyId that does not satisfy the modeled constraints, and more broadly one that is not a recognizable AWS key id, with 400 ValidationError (Type Sender) and the fixed message above, before any account lookup.
Traces: sts-access-key-info-lookup#task#1.

Cause 2 of 2: GetSessionToken answers 200 for an invalid MFA TokenCode/SerialNumber pair, instead of 403 AccessDenied — it vends credentials on a failed MFA challenge.
AWS: Smithy: GetSessionToken.SerialNumber (@length 9-256, pattern ^[\w+=/:,.@-]*$) and TokenCode (@length exactly 6, pattern ^[\d]*$) carry the MFA challenge; the only modeled operation error is RegionDisabledException (403), so an MFA failure is the service-common AccessDenied. The recorded response is HTTP 403 with <Error><Type>Sender</Type><Code>AccessDenied</Code><Message>MultiFactorAuthentication failed, unable to validate MFA code.  Please verify your MFA serial number is valid and associated with this user.</Message></Error> plus a RequestId. That message is entirely fixed text — including the two consecutive spaces after 'MFA code.' — and echoes neither the serial number nor the code.
Emulator: HTTP 200 with a GetSessionTokenResponse carrying Credentials; $.ErrorResponse is missing. The MFA challenge is not evaluated.
Why: this is a security-relevant bypass of the emulator's own credential vending: a session that AWS would refuse because the MFA code is wrong is handed working temporary credentials, so any test that asserts an MFA-gated path is actually gated passes falsely, and the aws:MultiFactorAuthPresent-conditioned policy behavior built on top of it cannot be exercised honestly.
Fix: the GetSessionToken handler's validation/authorization step — when SerialNumber is present, validate the TokenCode against the MFA device registered for the calling identity and refuse with 403 AccessDenied and the fixed message above before minting Credentials.
Traces: sts-get-session-token-invalid-mfa#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, bootstrap_signal=false).

Cause 1 — sts-access-key-info-lookup#task#1, GetAccessKeyInfo: 'status: expected 400, got 200' and '$.ErrorResponse: missing (expected {Error: {Type: Sender, Code: ValidationError, Message: "Access key ID is not valid."}, RequestId: <per-run uuid>}, got None)'.

Cause 2 — sts-get-session-token-invalid-mfa#task#1, GetSessionToken: 'status: expected 403, got 200' and '$.ErrorResponse: missing (expected {Error: {Type: Sender, Code: AccessDenied, Message: "MultiFactorAuthentication failed, unable to validate MFA code.  Please verify your MFA serial number is valid and associated with this user."}, RequestId: <per-run uuid>}, got None)'. The emulator instead returned a 200 GetSessionTokenResponse with Credentials.

The RequestId values in the recordings are per-run and are not part of the requirement; the diverging status, error Code and the fixed message text are.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay sts-access-key-info-lookup step task#1 — POST awsQuery Action=GetAccessKeyInfo with an AccessKeyId that is not a valid access key id; expect 400 / Sender / ValidationError / 'Access key ID is not valid.', observe 200 with a GetAccessKeyInfoResponse.
4. Cause 2: replay sts-get-session-token-invalid-mfa step task#1 — POST awsQuery Action=GetSessionToken with a SerialNumber and a TokenCode that do not match the identity's MFA device; expect 403 / Sender / AccessDenied with the MultiFactorAuthentication-failed message, observe 200 with Credentials.
5. Both steps must pass before this issue is closed.
