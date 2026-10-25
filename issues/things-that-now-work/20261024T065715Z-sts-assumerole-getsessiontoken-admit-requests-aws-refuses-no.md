Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-nonexistent-role-denied sts.assume-role-foreign-account-role-denied sts.assume-role-with-web-identity-nonexistent-role-denied sts.assume-role-external-id-mismatch-rejected sts.assume-role-arn-pattern-validation-rejected sts.assume-role-invalid-session-name-chars-rejected sts.assume-role-duration-exceeds-max-rejected sts.assume-role-duration-exceeds-role-max-session-duration-rejected sts.assume-role-chaining-caps-duration-at-one-hour sts.assume-role-managed-policy-arns-exceeding-max-count-rejected sts.assume-role-session-tags-require-tag-session-permission sts.assume-role-transitive-tag-survives-role-chaining sts.assume-role-duplicate-tag-keys-rejected sts.assume-role-session-tags-unicode-value-round-trip sts.get-access-key-info-resolves-temporary-asia-key sts.get-access-key-info-returns-correct-account sts.assume-role-packed-policy-too-large-rejected sts.get-federation-token-policy-caps-caller-permissions sts.assume-role-external-id-success-with-correct-id sts.e2e-role-lifecycle-assume-use-delete-revokes-future-assumption sts.get-caller-identity-userid-matches-created-role-id sts.e2e-session-tag-propagation-through-role-chain sts.server-side-source-identity-persists-through-role-chain sts.server-side-source-identity-missing-permission-rejected sts.get-federation-token-cannot-be-called-by-role-session sts.get-session-token-rejected-for-assumed-role-session sts.get-session-token-cannot-call-other-sts-operations sts.forged-credentials-rejected sts.get-caller-identity-reflects-iam-user sts.mfa-conditioned-role-assumable-only-with-a-real-virtual-mfa-device-token
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.assume-role-nonexistent-role-denied|_refusal#11|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED: {result!r}")
Issue-Key: sts.assume-role-foreign-account-role-denied|_refusal#12|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED: {result!r}")
Issue-Key: sts.assume-role-with-web-identity-nonexistent-role-denied|run#8|AssertionError|assert control_code is not None, (
Issue-Key: sts.assume-role-external-id-mismatch-rejected|_refusal#14|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED: {result!r}")
Issue-Key: sts.assume-role-arn-pattern-validation-rejected|_refusal#7|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED: {result!r}")
Issue-Key: sts.assume-role-invalid-session-name-chars-rejected|_refusal#9|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED: {result!r}")
Issue-Key: sts.assume-role-duration-exceeds-max-rejected|_refusal#9|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED: {result!r}")
Issue-Key: sts.assume-role-duration-exceeds-role-max-session-duration-rejected|_refusal#14|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED: {result!r}")
Issue-Key: sts.assume-role-chaining-caps-duration-at-one-hour|_refusal#22|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED: {result!r}")
Issue-Key: sts.assume-role-managed-policy-arns-exceeding-max-count-rejected|_refusal#13|AssertionError|raise AssertionError(f"expected a refusal, but the call SUCCEEDED: {result!r}")
Issue-Key: sts.assume-role-session-tags-require-tag-session-permission|_error_detail#22|AssertionError|raise AssertionError(
Issue-Key: sts.assume-role-transitive-tag-survives-role-chaining|run#35|AssertionError|raise AssertionError(
Issue-Key: sts.assume-role-duplicate-tag-keys-rejected|run#18|AssertionError|assert code == "InvalidParameterValue", code
Issue-Key: sts.assume-role-session-tags-unicode-value-round-trip|_error#28|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: sts.get-access-key-info-resolves-temporary-asia-key|run#13|AssertionError|assert temp_key_id.startswith("ASIA"), (
Issue-Key: sts.get-access-key-info-returns-correct-account|run#5|AssertionError|assert access_key_id.startswith("ASIA"), (
Issue-Key: sts.assume-role-packed-policy-too-large-rejected|run#15|AssertionError|assert control_creds["AccessKeyId"].startswith("ASIA"), (
Issue-Key: sts.get-federation-token-policy-caps-caller-permissions|run#34|AssertionError|assert narrow["Credentials"]["AccessKeyId"].startswith("ASIA"), (
Issue-Key: sts.assume-role-external-id-success-with-correct-id|run#20|AssertionError|assert aru["AssumedRoleId"] == expected_uid, (
Issue-Key: sts.e2e-role-lifecycle-assume-use-delete-revokes-future-assumption|run#28|AssertionError|assert user["AssumedRoleId"] == expected_user_id, (
Issue-Key: sts.get-caller-identity-userid-matches-created-role-id|run#26|AssertionError|assert aru["AssumedRoleId"] == expected_uid, (
Issue-Key: sts.e2e-session-tag-propagation-through-role-chain|run#26|KeyError|assert session2["SourceIdentity"] == "alice", (
Issue-Key: sts.server-side-source-identity-persists-through-role-chain|run#19|KeyError|assert session2["SourceIdentity"] == source_identity, (
Issue-Key: sts.server-side-source-identity-missing-permission-rejected|_error#23|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded and returned {result!r}")
Issue-Key: sts.get-federation-token-cannot-be-called-by-role-session|_error#18|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: sts.get-session-token-rejected-for-assumed-role-session|_refusal#20|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED: {result!r}")
Issue-Key: sts.get-session-token-cannot-call-other-sts-operations|_refusal#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED: {result!r}")
Issue-Key: sts.forged-credentials-rejected|run#7|AssertionError|assert code == "InvalidClientTokenId", f"expected credential rejection, got {code}"
Issue-Key: sts.get-caller-identity-reflects-iam-user|run#8|AssertionError|assert identity["Arn"] == user_arn, identity
Issue-Key: sts.mfa-conditioned-role-assumable-only-with-a-real-virtual-mfa-device-token|run#48|AssertionError|assert user_ident["Arn"] == user_arn, (
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# STS AssumeRole/GetSessionToken admit requests AWS refuses: no role existence or trust check, no input validation, no duration caps, unauthorized session tagging, and wrong identity/credential fields (9 causes)

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
This issue bundles NINE distinct net-new root causes in the STS service (service token `sts`). A fixer must close all nine before submitting; each cause names its own failing check ids.

Cause 1 of 9: AssumeRole mints credentials for a role that does not exist, for a role in another account, and for a trust policy whose ExternalId does not match.
AWS: Smithy model `AssumeRole` errors are ExpiredTokenException / MalformedPolicyDocumentException / PackedPolicyTooLargeException / RegionDisabledException; a principal that no trust policy authorizes is refused with the service-common client error AccessDenied, HTTP 403, message of the form "User: <caller arn> is not authorized to perform: sts:AssumeRole on resource: <role arn>" (recorded behavior for the resource-existence and ExternalId legs: STS does not disclose non-existence, it answers AccessDenied, and a wrong sts:ExternalId in the Condition is an AccessDenied too). AssumeRoleWithWebIdentity behaves the same way for a role ARN that names nothing.
Emulator: every one of these calls returns HTTP 200 with a full Credentials/AssumedRoleUser body. Replay shows credentials issued for `assumed-role/sim2real-behav-no-such-role-.../...`, for an ARN in account 111122223333 while the caller is 000000000000, and for an ExternalId the trust policy does not accept.
Why: a client that relies on least-privilege role boundaries is told cross-account and never-created roles are assumable, and any negative test of a trust policy passes against the emulator and fails in AWS. Role-name enumeration is also no longer refused.
Fix: the AssumeRole handler's authorization step (resolve the RoleArn in IAM state, evaluate the role's trust policy including the sts:ExternalId condition, and emit AccessDenied/403 when it does not authorize the caller) and the same step in AssumeRoleWithWebIdentity.
Traces: sts.assume-role-nonexistent-role-denied, sts.assume-role-foreign-account-role-denied, sts.assume-role-with-web-identity-nonexistent-role-denied, sts.assume-role-external-id-mismatch-rejected.

Cause 2 of 9: AssumeRole performs no input validation and enforces no duration ceiling.
AWS: Smithy `AssumeRoleRequest` constrains RoleArn (@length 20..2048, @pattern), RoleSessionName (@length 2..64, @pattern ^[\w+=,.@-]*$) and DurationSeconds (@range 900..43200); violations are ValidationError, HTTP 400, "1 validation error detected: ...". Beyond the model (recorded behavior): DurationSeconds above the role's MaxSessionDuration is ValidationError, a role-chained AssumeRole is capped at 3600s, and more than the allowed number of PolicyArns is refused.
Emulator: all of these are accepted with HTTP 200 and credentials; e.g. a malformed RoleArn, a RoleSessionName containing characters outside the modeled pattern, DurationSeconds over the role's maximum, and an over-count PolicyArns list all succeed (replay shows Expiration one hour or more out with PackedPolicySize present).
Why: callers cannot discover a bad session name or an over-long duration until they run against AWS, and a session that AWS would never issue is handed out.
Fix: AssumeRole request validation (model-constraint checks before any state change) plus the duration-ceiling logic in the handler (role MaxSessionDuration, 1-hour chaining cap).
Traces: sts.assume-role-arn-pattern-validation-rejected, sts.assume-role-invalid-session-name-chars-rejected, sts.assume-role-duration-exceeds-max-rejected, sts.assume-role-duration-exceeds-role-max-session-duration-rejected, sts.assume-role-chaining-caps-duration-at-one-hour, sts.assume-role-managed-policy-arns-exceeding-max-count-rejected.

Cause 3 of 9: session tagging is never authorized, and a duplicate tag key is refused with the wrong error code.
AWS: passing Tags on AssumeRole requires the trust policy to allow sts:TagSession for the caller; without it the call is AccessDenied/403 (recorded behavior), and carrying transitive tags into a chained AssumeRole requires sts:TagSession on the TARGET role's trust policy. Duplicate tag keys are refused with error code InvalidParameterValue, HTTP 400 (recorded behavior; the model lists no dedicated error).
Emulator: AssumeRole with Tags against a role whose trust policy allows only sts:AssumeRole returns 200 with credentials and PackedPolicySize=1; the chained assume carrying transitive tags also succeeds; duplicate tag keys are refused with code "ValidationException" instead of InvalidParameterValue.
Why: tag-based access control (aws:PrincipalTag) can be established by any caller, so an emulator-validated ABAC design is unenforceable in AWS; and a client switching on the error code for duplicate keys never sees the branch AWS takes.
Fix: the AssumeRole handler's tag path — authorize sts:TagSession against the resolved trust policy, and map duplicate-key rejection to InvalidParameterValue.
Traces: sts.assume-role-session-tags-require-tag-session-permission, sts.assume-role-transitive-tag-survives-role-chaining, sts.assume-role-duplicate-tag-keys-rejected, sts.assume-role-session-tags-unicode-value-round-trip.

Cause 4 of 9: temporary credentials are issued with an access key id outside the ASIA class.
AWS: temporary credentials from AssumeRole/GetSessionToken/GetFederationToken carry an AccessKeyId beginning with the ASIA key-class prefix; GetAccessKeyInfo is documented in terms of that class ("Access keys consist of two parts..."), and the prefix is how every SDK and audit tool distinguishes a temporary key from a long-term AKIA key (recorded behavior; the Credentials shape carries no pattern).
Emulator: every minted key id begins with "EMULATOR" (e.g. 'EMULATORASUHRX6W3IDD', 'EMULATORASTEI5KL7UJQ'), so the prefix check fails with got prefix 'EMUL'.
Why: client code and policies that branch on the key class (ASIA vs AKIA) take the wrong branch, and GetAccessKeyInfo cannot classify the keys the emulator itself issued.
Fix: the credential-minting component shared by the STS session operations (key-id generation).
Traces: sts.get-access-key-info-resolves-temporary-asia-key, sts.get-access-key-info-returns-correct-account, sts.assume-role-packed-policy-too-large-rejected, sts.get-federation-token-policy-caps-caller-permissions.

Cause 5 of 9: AssumedRoleUser.AssumedRoleId is not built from the assumed role's RoleId.
AWS: AssumeRole returns AssumedRoleUser.AssumedRoleId as "<RoleId>:<RoleSessionName>", where RoleId is exactly the value iam:CreateRole/GetRole reported for that role; GetCallerIdentity under those credentials reports the same UserId (recorded behavior, and the only reading under which the two APIs agree).
Emulator: a fresh unrelated AROA id is generated per call — e.g. CreateRole reported 'AROABGXEQES5NRCCJJWAN' while AssumeRole answered 'AROABWBO334L2WDYW5P34:<session>' / 'AROAIUN3WOM5EHIGMXLVD:<session>' for the same role.
Why: anything that correlates a session back to its role (CloudTrail-style audit joins, policy conditions on the role id, UserId comparisons) cannot match, and two calls on one role disagree with each other.
Fix: the AssumeRole handler/serializer must read the role's stored RoleId from IAM state instead of generating one.
Traces: sts.assume-role-external-id-success-with-correct-id, sts.e2e-role-lifecycle-assume-use-delete-revokes-future-assumption, sts.get-caller-identity-userid-matches-created-role-id.

Cause 6 of 9: SourceIdentity is dropped from the AssumeRole response and is never authorized.
AWS: Smithy `AssumeRoleResponse` has member SourceIdentity (@length 2..64, @pattern ^[\w+=,.@-]*$); when the request carries SourceIdentity it is echoed and persists through a role chain. Setting SourceIdentity requires sts:SetSourceIdentity in the trust policy; without it the assume is refused with AccessDenied/403 (recorded behavior).
Emulator: the response omits SourceIdentity entirely (the checks fail with KeyError: 'SourceIdentity'), and an assume that sets SourceIdentity without the permission returns 200 with credentials.
Why: a required response member is missing, so any client reading it raises; and the audit guarantee SourceIdentity exists for (an identity a session cannot change) is unenforced.
Fix: the AssumeRole handler/serializer (echo and persist SourceIdentity across chained sessions) and its authorization step (sts:SetSourceIdentity).
Traces: sts.e2e-session-tag-propagation-through-role-chain, sts.server-side-source-identity-persists-through-role-chain, sts.server-side-source-identity-missing-permission-rejected.

Cause 7 of 9: GetFederationToken and GetSessionToken do not restrict the class of calling credential.
AWS: GetFederationToken cannot be called with assumed-role (temporary) credentials and GetSessionToken cannot be called by a role session; both are refused with AccessDenied/403, and a GetSessionToken credential may not call the other STS session operations (recorded behavior; documented restriction of these operations).
Emulator: each of these calls returns HTTP 200 — replay shows GetFederationToken answering a FederatedUser body to a role session, and GetSessionToken answering Credentials to an assumed-role session.
Why: a credential-vending design that AWS forbids appears to work, and the error path clients must handle never occurs.
Fix: the GetFederationToken and GetSessionToken handlers' caller-class precondition.
Traces: sts.get-federation-token-cannot-be-called-by-role-session, sts.get-session-token-rejected-for-assumed-role-session, sts.get-session-token-cannot-call-other-sts-operations.

Cause 8 of 9: never-issued (forged) credentials are refused with the wrong error code.
AWS: a syntactically valid access key id that was never issued is refused by the auth layer with code InvalidClientTokenId, HTTP 403 (recorded behavior; this is the standard STS/SigV4 unknown-key refusal, distinct from an authorization denial).
Emulator: the same request is refused with code AccessDenied.
Why: SDK retry/credential-refresh logic keys on InvalidClientTokenId to re-resolve credentials and on AccessDenied to fail the call; the wrong code sends the client down the wrong path.
Fix: the request authentication layer's unknown-access-key classification (before authorization).
Traces: sts.forged-credentials-rejected.

Cause 9 of 9: GetCallerIdentity ignores the signing credential and always reports one fixed user.
AWS: GetCallerIdentity reports the identity of the credential that signed the request — for an IAM user's own access key, UserId is that user's AIDA id and Arn is arn:aws:iam::<account>:user/<that user> ("Returns details about the IAM user or role whose credentials are used to call the operation").
Emulator: regardless of which created IAM user's access key signs the request, the answer is UserId 'AIDAH5I7GEMO7ADXNFAZU', Arn 'arn:aws:iam::000000000000:user/emulator'. The MFA check fails for the same reason: the long-term key created for the user under test resolves to user/emulator.
Why: every per-principal assertion (and any policy decision keyed on the caller) is wrong, and two different users are indistinguishable.
Fix: the GetCallerIdentity handler must resolve the identity from the request's signing credential in IAM/STS state rather than returning a constant.
Traces: sts.get-caller-identity-reflects-iam-user, sts.mfa-conditioned-role-assumable-only-with-a-real-virtual-mfa-device-token.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 (30 failing checks clustered here).

Cause 1 — expected AccessDenied/403, got HTTP 200: "expected a refusal, but the call SUCCEEDED: {'Credentials': {'AccessKeyId': 'EMULATORASWIIA7SHQQ7', ...}, 'AssumedRoleUser': {'AssumedRoleId': 'AROATUZD5NHSOOSWVUWE5:sim2real-behav-sess-...', 'Arn': 'arn:aws:sts::000000000000:assumed-role/sim2real-behav-no-such-role-.../...'}, 'ResponseMetadata': {'HTTPStatusCode': 200}}"; foreign account leg returned 'arn:aws:sts::111122223333:assumed-role/sim2real-behav-foreign-acct-role-.../...'; AssumeRoleWithWebIdentity leg: "AssumeRole ISSUED CREDENTIALS for a nonexistent role".

Cause 2 — "expected a refusal, but the call SUCCEEDED" with Expiration 1h+ out for DurationSeconds over the maximum and over the role's MaxSessionDuration; chained assume returned Expiration 08:45 for a 06:45 request (no 1h cap); over-count PolicyArns returned 200 with PackedPolicySize=25.

Cause 3 — "session tagging must be denied without sts:TagSession, but the call SUCCEEDED and returned {... 'PackedPolicySize': 1 ...}"; "a chained AssumeRole carrying transitive tags forward must be authorized for sts:TagSession on the TARGET role's trust policy ... it succeeded instead"; duplicate keys: assert code == "InvalidParameterValue" -> AssertionError: ValidationException.

Cause 4 — "expected an ASIA-prefixed temporary access key from AssumeRole, got prefix 'EMUL'"; "expected a temporary (ASIA) access key id from GetSessionToken, got 'EMUL'"; "a federation token's access key id must be in the ASIA class; got prefix 'EMUL'".

Cause 5 — "AssumeRole.AssumedRoleUser.AssumedRoleId must be '<RoleId>:<SessionName>' built from the RoleId CreateRole returned, i.e. 'AROABGXEQES5NRCCJJWAN:sim2real-behav-sess-fe3596ffbd72'; got 'AROAYWCISB76WZ2NWXQKB:sim2real-behav-sess-fe3596ffbd72'".

Cause 6 — KeyError: 'SourceIdentity' on the AssumeRole response in both chain checks; "expected a refusal, but the call succeeded and returned {'Credentials': ...}" for the missing-sts:SetSourceIdentity leg.

Cause 7 — "expected a refusal, but the call succeeded: {... 'FederatedUser': {'FederatedUserId': '000000000000:sim2realbehavprobe'} ...}" (GetFederationToken from a role session); "expected the call to be refused, but it SUCCEEDED: {'Credentials': ...}" (GetSessionToken from an assumed-role session).

Cause 8 — assert code == "InvalidClientTokenId" -> AssertionError: expected credential rejection, got AccessDenied (key AKIAIOSFODNN7EXAMPLE, never issued).

Cause 9 — GetCallerIdentity returned {'UserId': 'AIDAH5I7GEMO7ADXNFAZU', 'Account': '000000000000', 'Arn': 'arn:aws:iam::000000000000:user/emulator'} for the user under test; MFA check: "the long-term key does not belong to the user under test: 'arn:aws:iam::000000000000:user/emulator' != 'arn:aws:iam::000000000000:user/sim2real-behav-sts-user-...'".

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay the check ids in trace_ids; each is self-contained and establishes a positive control first.
3. Minimal manual repro for cause 1: iam.create_role with a trust policy naming only the caller, then sts.assume_role(RoleArn='arn:aws:iam::000000000000:role/does-not-exist', RoleSessionName='probe') — the emulator returns 200 with Credentials where AWS returns AccessDenied/403.
4. Cause 2: sts.assume_role(RoleArn=<role>, RoleSessionName='bad session name!', DurationSeconds=43200) against a role with MaxSessionDuration=3600 — 200 instead of ValidationError/400.
5. Cause 9: create an IAM user, create its access key, sign sts.get_caller_identity with it — the response always names user/emulator.
