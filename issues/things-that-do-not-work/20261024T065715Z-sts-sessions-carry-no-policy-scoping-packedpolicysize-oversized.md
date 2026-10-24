Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-session-policy-cannot-exceed-role-permissions sts.assume-role-session-policy-cannot-exceed-role-permissions-denied sts.assume-role-session-policy-explicit-deny-wins sts.assume-role-session-tag-empty-string-value-preserved sts.assume-role-session-tag-unicode-value-round-trip sts.concurrent-assume-role-session-tag-updates-do-not-leak-across-sessions sts.concurrent-get-federation-token-calls-produce-independent-policies sts.get-caller-identity-cannot-be-denied-by-policy sts.get-federation-token-no-policy-grants-no-permissions sts.get-federation-token-without-policy-denies-actions sts.get-federation-token-cannot-call-iam-denied sts.get-federation-token-policy-document-unicode-round-trip sts.get-delegated-access-token-without-required-permission-denied sts.principal-type-condition-distinguishes-an-assumed-role-from-a-federated-user sts.e2e-federation-token-policy-restricts-then-expires sts.e2e-federation-token-policy-scopes-down-then-expires sts.assume-role-session-policy-arns-intersect-role-permissions sts.assume-role-session-policy-over-max-length-rejected sts.get-delegated-access-token-returns-usable-credentials sts.get-delegated-access-token-cannot-exceed-source-token-privileges-denied sts.get-delegated-access-token-region-disabled-blocked sts.get-web-identity-token-duration-escalation-rejected sts.get-web-identity-token-duration-exceeds-max-rejected sts.get-web-identity-token-outbound-federation-disabled-blocked sts.get-web-identity-token-outbound-federation-disabled-rejected sts.get-web-identity-token-payload-size-exceeded-rejected sts.get-web-identity-token-rejected-by-assume-role-with-web-identity sts.get-web-identity-token-returns-valid-jwt-structure sts.get-web-identity-token-session-duration-escalation-rejected sts.get-web-identity-token-ttl-matches-requested-duration sts.assume-role-with-web-identity-invalid-session-name-rejected sts.get-federation-token-invalid-name-rejected sts.assume-role-session-tag-value-exceeds-max-length-rejected sts.get-federation-token-policy-over-max-length-rejected sts.a-chained-session-outlasts-the-parent-role-and-expires-later-than-the-parent-session sts.assume-role-external-id-required-denied sts.assume-role-session-policy-action-wildcard-granularity sts.assume-role-session-policy-intersection-allows-and-denies sts.assume-role-session-tag-decides-policy-condition sts.assume-role-trust-policy-principal-restriction-enforced sts.assumed-role-credentials-are-rejected-after-their-expiration-instant-passes sts.decode-authorization-message-round-trip sts.deleting-the-role-invalidates-credentials-already-issued-and-recreating-it-by-name-does-not-restore-them sts.e2e-assume-role-session-tag-propagates-to-resource-access sts.managed-policy-arn-and-inline-session-policy-intersect-to-the-narrower-of-the-two sts.permissions-boundary-denies-an-action-the-roles-own-identity-policy-allows sts.session-policy-region-condition-rejected-in-one-region-and-permitted-in-another sts.token-issue-time-is-present-only-for-the-temporary-half-of-one-identity sts.two-differently-scoped-sessions-used-concurrently-keep-their-own-authorization-outcomes
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.assume-role-session-policy-cannot-exceed-role-permissions|run#25|AssertionError|assert code == "AccessDenied", (
Issue-Key: sts.assume-role-session-policy-cannot-exceed-role-permissions-denied|run#20|AssertionError|assert code in allowed_codes, msg
Issue-Key: sts.assume-role-session-policy-explicit-deny-wins|run#28|AssertionError|assert code == "AccessDenied", (
Issue-Key: sts.assume-role-session-tag-empty-string-value-preserved|run#9|AssertionError|assert code == "AccessDenied", (
Issue-Key: sts.assume-role-session-tag-unicode-value-round-trip|run#16|AssertionError|assert code == "AccessDenied", (
Issue-Key: sts.concurrent-assume-role-session-tag-updates-do-not-leak-across-sessions|run#27|AssertionError|assert result["inner_ok"] is False, f"blue-tagged session leaked the red grant: {result}"
Issue-Key: sts.concurrent-get-federation-token-calls-produce-independent-policies|run#5|AssertionError|assert sm_result == ("AccessDeniedException", _SM_DENIED.format(arn=fed_arn)), (
Issue-Key: sts.get-caller-identity-cannot-be-denied-by-policy|_client_error#21|AssertionError|raise AssertionError(f"expected the call to fail, but it succeeded: {result!r}")
Issue-Key: sts.get-federation-token-no-policy-grants-no-permissions|run#18|AssertionError|assert code == "AccessDenied", (
Issue-Key: sts.get-federation-token-without-policy-denies-actions|probe#12|AssertionError|raise AssertionError(
Issue-Key: sts.get-federation-token-cannot-call-iam-denied|_error#13|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: sts.get-federation-token-policy-document-unicode-round-trip|_client_error#31|AssertionError|raise AssertionError(f"expected this call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: sts.get-delegated-access-token-without-required-permission-denied|_poll_until#18|AssertionError|raise AssertionError(f"{timeout_message} (last observed: {result!r})")
Issue-Key: sts.principal-type-condition-distinguishes-an-assumed-role-from-a-federated-user|run#51|AssertionError|assert role_outcomes == expected_role, (
Issue-Key: sts.e2e-federation-token-policy-restricts-then-expires|_client_error#27|AssertionError|raise AssertionError("the call succeeded where AWS refuses it")
Issue-Key: sts.e2e-federation-token-policy-scopes-down-then-expires|_client_error#18|AssertionError|raise AssertionError("the call succeeded where AWS refuses it")
Issue-Key: sts.assume-role-session-policy-arns-intersect-role-permissions|run#34|AssertionError|assert isinstance(base_packed, int) and 0 <= base_packed <= 100, (
Issue-Key: sts.assume-role-session-policy-over-max-length-rejected|run#24|AssertionError|assert code == "ValidationError", code
Issue-Key: sts.get-delegated-access-token-returns-usable-credentials|run#6|AssertionError|assert code_garbage == "ValidationError", code_garbage
Issue-Key: sts.get-delegated-access-token-cannot-exceed-source-token-privileges-denied|run#5|AssertionError|assert code == "ValidationError", f"unexpected code: {code}"
Issue-Key: sts.get-delegated-access-token-region-disabled-blocked|run#5|AssertionError|assert control_code == "ValidationError", f"unexpected control code: {control_code}"
Issue-Key: sts|GetOutboundWebIdentityFederationInfo|InvalidAction
Issue-Key: sts|EnableOutboundWebIdentityFederation|InvalidAction
Issue-Key: sts.assume-role-with-web-identity-invalid-session-name-rejected|<lambda>#5|ResponseParserError|lambda: client.assume_role_with_web_identity(
Issue-Key: sts.get-federation-token-invalid-name-rejected|run#8|AssertionError|assert message == (
Issue-Key: sts.assume-role-session-tag-value-exceeds-max-length-rejected|run#16|AssertionError|assert message == over_len_message, message
Issue-Key: sts.get-federation-token-policy-over-max-length-rejected|run#11|AssertionError|assert message == (
Issue-Key: sts.a-chained-session-outlasts-the-parent-role-and-expires-later-than-the-parent-session|_poll#57|Timeout|time.sleep(interval_s)
Issue-Key: sts.assume-role-external-id-required-denied|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: sts.assume-role-session-policy-action-wildcard-granularity|_poll#32|Timeout|time.sleep(interval_s)
Issue-Key: sts.assume-role-session-policy-intersection-allows-and-denies|_poll#38|Timeout|time.sleep(interval_s)
Issue-Key: sts.assume-role-session-tag-decides-policy-condition|_poll#30|Timeout|time.sleep(interval_s)
Issue-Key: sts.assume-role-trust-policy-principal-restriction-enforced|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: sts.assumed-role-credentials-are-rejected-after-their-expiration-instant-passes|run#47|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: sts.decode-authorization-message-round-trip|_provoke_encoded_authorization_message#19|Timeout|time.sleep(2.0)
Issue-Key: sts.deleting-the-role-invalidates-credentials-already-issued-and-recreating-it-by-name-does-not-restore-them|_poll#43|Timeout|time.sleep(interval_s)
Issue-Key: sts.e2e-assume-role-session-tag-propagates-to-resource-access|_retry_until#32|Timeout|time.sleep(interval_s)
Issue-Key: sts.managed-policy-arn-and-inline-session-policy-intersect-to-the-narrower-of-the-two|_poll#51|Timeout|time.sleep(interval_s)
Issue-Key: sts.permissions-boundary-denies-an-action-the-roles-own-identity-policy-allows|_poll#44|Timeout|time.sleep(interval_s)
Issue-Key: sts.session-policy-region-condition-rejected-in-one-region-and-permitted-in-another|_poll#50|Timeout|time.sleep(interval_s)
Issue-Key: sts.token-issue-time-is-present-only-for-the-temporary-half-of-one-identity|_poll#46|Timeout|time.sleep(interval_s)
Issue-Key: sts.two-differently-scoped-sessions-used-concurrently-keep-their-own-authorization-outcomes|_poll#48|Timeout|time.sleep(interval_s)
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# STS sessions carry no policy scoping, PackedPolicySize/oversized-policy classification is wrong, and GetDelegatedAccessToken plus the outbound web-identity federation operations are unserved (8 causes)

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFederationToken

## Description
This issue bundles EIGHT distinct net-new root causes in the STS service (service token `sts`). A fixer must close all eight before submitting; each cause names its own failing check ids. These are separate from the AssumeRole admission/identity causes filed alongside this issue.

Cause 1 of 8: a temporary session's scoping is never evaluated — inline session policies, PolicyArns, permissions boundaries and principal tag/type conditions have no effect on any later call.
AWS: the effective permissions of a session are the intersection of the role's identity policy, the session policy (inline Policy and/or PolicyArns), the role's permissions boundary and any resource policy; an explicit Deny anywhere wins; a call outside that intersection is refused with AccessDenied/403 at the called service ("User: <session arn> is not authorized to perform: <action> on resource: <arn>"). A GetFederationToken session with NO policy has no permissions at all. aws:PrincipalTag/<key> and aws:PrincipalType conditions select on the session's own tags/class.
Emulator: every such call succeeds. Replay shows s3:ListAllMyBuckets answering 200 with a Buckets list under a federation token minted with no policy, secretsmanager:ListSecrets succeeding under a session policy that grants only s3:ListAllMyBuckets, s3:GetObject succeeding where the session policy denies it, IAM ListUsers answering 200 to a federated session, and a blue-tagged session being granted what only the red tag allows.
Why: this is the security property the whole credential-vending feature exists for. A developer validating least-privilege session scoping, ABAC, or a permissions boundary against the emulator ships a design that behaves completely differently in AWS, and the denial path their code must handle is never exercised.
Fix: a session-scoped authorization evaluation shared by the data-plane handlers (policy intersection + explicit-deny + condition-key evaluation keyed off the session carried in the request's credentials). Related but NOT covering this: open issue 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md (filed against `iam`).
Traces: sts.get-federation-token-no-policy-grants-no-permissions, sts.get-federation-token-without-policy-denies-actions, sts.get-federation-token-cannot-call-iam-denied, sts.get-federation-token-policy-document-unicode-round-trip, sts.concurrent-get-federation-token-calls-produce-independent-policies, sts.assume-role-session-policy-cannot-exceed-role-permissions, sts.assume-role-session-policy-cannot-exceed-role-permissions-denied, sts.assume-role-session-policy-explicit-deny-wins, sts.assume-role-session-tag-empty-string-value-preserved, sts.assume-role-session-tag-unicode-value-round-trip, sts.concurrent-assume-role-session-tag-updates-do-not-leak-across-sessions, sts.principal-type-condition-distinguishes-an-assumed-role-from-a-federated-user, sts.get-caller-identity-cannot-be-denied-by-policy, sts.get-delegated-access-token-without-required-permission-denied, sts.e2e-federation-token-policy-restricts-then-expires, sts.e2e-federation-token-policy-scopes-down-then-expires.

Cause 2 of 8: PackedPolicySize is omitted when no session policy is supplied.
AWS: Smithy `AssumeRoleResponse.PackedPolicySize` is a nonNegativeInteger reported as a percentage of the allowed packed size; it is present on every AssumeRole response, including one with no inline Policy (recorded behavior — the member is modeled, if deprecated in favour of SessionTokenUtilization).
Emulator: PackedPolicySize is None/absent on an AssumeRole that passes PolicyArns but no inline Policy.
Why: a client reading the member to monitor how close a session is to the packing limit gets nothing, and the response differs in shape between two AssumeRole calls that AWS answers identically.
Fix: the AssumeRole response serializer (always populate PackedPolicySize from the packed session state).
Traces: sts.assume-role-session-policy-arns-intersect-role-permissions.

Cause 3 of 8: an over-long inline session Policy is classified as PackedPolicyTooLarge instead of a validation failure.
AWS: an AssumeRole inline Policy longer than the plain-text limit is refused with code ValidationError, HTTP 400, before the packed-size check runs; PackedPolicyTooLarge (HTTP 400, awsQueryError code PackedPolicyTooLarge) is reserved for a request whose packed session state exceeds the token limit (recorded behavior; the model's own note is that the inline-Policy length limit is checked first).
Emulator: returns PackedPolicyTooLarge for the over-length plain-text Policy, so the two distinct rejection classes collapse into one.
Why: a client that distinguishes "your policy text is too long" from "your session state does not fit" — and retries differently for each — takes the wrong branch.
Fix: AssumeRole request validation ordering (length check on Policy before packing) in the AssumeRole handler.
Traces: sts.assume-role-session-policy-over-max-length-rejected.

Cause 4 of 8: GetDelegatedAccessToken is a scaffold that answers NotImplemented.
AWS: Smithy binds `GetDelegatedAccessToken` (input GetDelegatedAccessTokenRequest.TradeInToken required/sensitive; output Credentials + AssumedPrincipal; errors ExpiredTradeInTokenException 400, PackedPolicyTooLargeException 400, RegionDisabledException 403). A valid trade-in token yields usable credentials; a region-disabled call is RegionDisabledException/403.
Emulator: every call is refused with code NotImplemented, so neither the success shape nor any modeled error can be observed.
Why: the operation is unusable, and a client cannot distinguish an expired trade-in token from a disabled region.
Fix: implement the GetDelegatedAccessToken handler in the STS service crate.
Traces: sts.get-delegated-access-token-returns-usable-credentials, sts.get-delegated-access-token-cannot-exceed-source-token-privileges-denied, sts.get-delegated-access-token-region-disabled-blocked.

Cause 5 of 8: the outbound web-identity federation account operations are not routed at all.
AWS: Smithy binds `EnableOutboundWebIdentityFederation` (output IssuerIdentifier; error FeatureEnabledException, awsQueryError code FeatureEnabled, HTTP 409) and `GetOutboundWebIdentityFederationInfo` (output IssuerIdentifier + JwtVendingEnabled; error FeatureDisabledException, awsQueryError code FeatureDisabled, HTTP 404) on the IAM service; STS GetWebIdentityToken (errors JWTPayloadSizeExceededException, OutboundWebIdentityFederationDisabledException, SessionDurationEscalationException) is only reachable once the feature is enabled.
Emulator: both calls are answered with code InvalidAction, message "unknown action: GetOutboundWebIdentityFederationInfo" / "unknown action: EnableOutboundWebIdentityFederation" — the action is not dispatched, so the entire GetWebIdentityToken surface (9 checks) is unreachable.
Why: a modeled operation answers as if the client sent a typo, which is indistinguishable from an SDK/version problem; and no JWT-vending behavior can be exercised at all.
Fix: route and implement these two operations in the IAM service crate's query-protocol action dispatch (they are what gates STS GetWebIdentityToken).
Traces: sts.get-web-identity-token-returns-valid-jwt-structure, sts.get-web-identity-token-ttl-matches-requested-duration, sts.get-web-identity-token-duration-escalation-rejected, sts.get-web-identity-token-duration-exceeds-max-rejected, sts.get-web-identity-token-session-duration-escalation-rejected, sts.get-web-identity-token-payload-size-exceeded-rejected, sts.get-web-identity-token-outbound-federation-disabled-blocked, sts.get-web-identity-token-outbound-federation-disabled-rejected, sts.get-web-identity-token-rejected-by-assume-role-with-web-identity.

Cause 6 of 8: an unsigned (anonymous) STS request cannot be routed, and is answered with a JSON UnknownService body instead of the protocol's error shape.
AWS: Smithy marks `AssumeRoleWithWebIdentity` (and AssumeRoleWithSAML) optionalAuth with auth: [] — they are called with no SigV4 credentials at all. The service is awsQuery, so a refusal is an XML ErrorResponse; an invalid RoleSessionName is ValidationError/400 ("1 validation error detected: Value ... at 'roleSessionName' failed to satisfy constraint: Member must satisfy regular expression pattern ...").
Emulator: the request never reaches STS — the response body is JSON `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}`, which botocore cannot parse as XML (ResponseParserError).
Why: every anonymous STS entry point is unusable, and the client gets a parse error rather than any AWS error it can handle. Service resolution must not depend on a SigV4 credential scope for operations AWS allows unsigned.
Fix: the request router's service-resolution step (resolve awsQuery requests from the endpoint/Action when no credential scope or X-Amz-Target is present) and the STS error serializer for anonymous operations.
Traces: sts.assume-role-with-web-identity-invalid-session-name-rejected.

Cause 7 of 8: ValidationError messages do not render the request value and the modeled constraint the way AWS does.
AWS: a constraint violation is reported as "1 validation error detected: Value '<the value as sent>' at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <the model's @pattern, verbatim>" / "Member must have length less than or equal to <max>". The rule is known: the message is built from the request value and the member's modeled constraint — e.g. GetFederationToken.Name @pattern [\w+=,.@-]* , Policy @length max 2048, AssumeRole Tags value max 256.
Emulator: the error code is right (ValidationError) but the message is built differently — the offending value is replaced by a truncation marker ("vvvv... [257 bytes]", "{\"Version\"... [8840 bytes]") instead of the value as sent, and the pattern is rendered anchored (^[\w+=,.@-]*$) instead of as the model states it.
Why: clients and tooling that parse the validation message to point a user at the offending input see a byte count instead of the value, and a pattern that is not the one the model publishes.
Fix: the shared validation-message formatter (echo the request value; print the @pattern verbatim from the model).
Traces: sts.get-federation-token-invalid-name-rejected, sts.assume-role-session-tag-value-exceeds-max-length-rejected, sts.get-federation-token-policy-over-max-length-rejected.

Cause 8 of 8: fifteen checks never complete — each establishes a positive control, then polls for the AccessDenied / credential-expiry transition that the emulator never produces, and so exhausts the 60s harness cap.
AWS: every one of these transitions is observable: a session policy / trust-policy principal restriction / region condition / session tag condition / permissions boundary produces AccessDenied/403 on the next call; credentials stop being accepted once Expiration passes; deleting the role invalidates sessions already issued and re-creating it by name does not restore them; DecodeAuthorizationMessage round-trips an encoded authorization failure.
Emulator: no divergent RESPONSE is captured for these checks — the expected refusal simply never arrives (the checks' _poll_denied/_poll helpers run to their deadline, inside the 60s cap), which is the same missing evaluation as cause 1 plus missing session-expiry enforcement. Reported here so they are not lost; verifying them requires causes 1-7 and session expiry to be in place.
Why: a client cannot rely on a session ever losing access — neither by policy nor by expiry.
Fix: the session-authorization evaluation (cause 1) plus expiry/lifecycle enforcement in the credential validation path (reject credentials past Expiration, and invalidate sessions whose role was deleted).
Traces: sts.permissions-boundary-denies-an-action-the-roles-own-identity-policy-allows, sts.managed-policy-arn-and-inline-session-policy-intersect-to-the-narrower-of-the-two, sts.assume-role-session-policy-action-wildcard-granularity, sts.assume-role-session-policy-intersection-allows-and-denies, sts.assume-role-session-tag-decides-policy-condition, sts.e2e-assume-role-session-tag-propagates-to-resource-access, sts.session-policy-region-condition-rejected-in-one-region-and-permitted-in-another, sts.two-differently-scoped-sessions-used-concurrently-keep-their-own-authorization-outcomes, sts.assume-role-trust-policy-principal-restriction-enforced, sts.assume-role-external-id-required-denied, sts.assumed-role-credentials-are-rejected-after-their-expiration-instant-passes, sts.a-chained-session-outlasts-the-parent-role-and-expires-later-than-the-parent-session, sts.deleting-the-role-invalidates-credentials-already-issued-and-recreating-it-by-name-does-not-restore-them, sts.token-issue-time-is-present-only-for-the-temporary-half-of-one-identity, sts.decode-authorization-message-round-trip.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 (49 failing checks clustered here).

Cause 1 — "a federated session with no session policy must have no permissions at all, got code='<call succeeded>'"; "expected the policy-less federation token to be refused, but the call SUCCEEDED and returned {... 'Buckets': [], 'Owner': {...}}" (HTTP 200); "session policy granted only 's3:ListAllMyBuckets': expected ListSecrets to be denied, got ('<call succeeded>', None)"; "session policy must not grant permissions beyond the role's identity-based policy, got code='(no error: the call succeeded)'"; "explicit Deny in the session policy must override the role's Allow, got code='(no error: the call succeeded)'"; "expected a refusal, but the call succeeded: {'Users': [], 'IsTruncated': False, ... 'HTTPStatusCode': 200}" (IAM ListUsers under a federation token); "blue-tagged session leaked the red grant: {'outer_ok': True, 'inner_ok': True}"; "aws:PrincipalType is wrong for an AssumeRole session: expected {'AssumedRole': '<call succeeded>', 'FederatedUser': 'AccessDenied', 'User': 'AccessDenied'}, got {'AssumedRole': '<call succeeded>', 'FederatedUser': '<call succeeded>', 'User': '<call succeeded>'}"; "expected this call to be denied, but it SUCCEEDED and returned {... 'ETag': '\"af055f9b...\"', 'ContentLength': 14 ...}" (S3 GetObject under a unicode-conditioned federation policy).

Cause 2 — "AssumeRole must report PackedPolicySize as a 0..100 percentage even with no session policy supplied; got None".

Cause 3 — assert code == "ValidationError" -> AssertionError: PackedPolicyTooLarge.

Cause 4 — "AssertionError: NotImplemented" / "unexpected code: NotImplemented" / "unexpected control code: NotImplemented" on GetDelegatedAccessToken.

Cause 5 — botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the GetOutboundWebIdentityFederationInfo operation: unknown action: GetOutboundWebIdentityFederationInfo (and the same for EnableOutboundWebIdentityFederation).

Cause 6 — botocore.parsers.ResponseParserError: Unable to parse response (not well-formed (invalid token): line 1, column 0), invalid XML received: b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}' — raised for an unsigned AssumeRoleWithWebIdentity call.

Cause 7 — got "1 validation error detected: Value 'vvvv...[257 bytes]' at 'tags.1.member.value' failed to satisfy constraint: Member must have length less than or equal to 256" where AWS sends the 257-character value itself; got pattern "^[\\w+=,.@-]*$" where the model and AWS message carry "[\\w+=,.@-]*"; got "Value '{\"Version\"...[8840 bytes]' at 'policy'" for the over-2048 policy.

Cause 8 — "Error: Check timed out after 60.0s" for all fifteen listed checks; each check's helper is a bounded poll for the expected refusal (e.g. _poll_denied(..., "AccessDenied", timeout_s=180) in sts.permissions-boundary-denies-an-action-the-roles-own-identity-policy-allows, reached only after a positive control proved the credential live).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay the check ids in trace_ids.
3. Minimal manual repro for cause 1: sts.get_federation_token(Name='probe', DurationSeconds=900) with NO Policy, then call s3.list_buckets with the returned credentials — the emulator answers 200 where AWS answers AccessDenied/403.
4. Cause 5: iam.get_outbound_web_identity_federation_info() — the emulator answers InvalidAction 'unknown action'.
5. Cause 6: build an STS client with botocore Config(signature_version=UNSIGNED) and call assume_role_with_web_identity(...) — the emulator returns a JSON UnknownService body instead of an XML awsQuery error.
6. Cause 8: run sts.permissions-boundary-denies-an-action-the-roles-own-identity-policy-allows and observe the PutObject outside the boundary keep succeeding until the cap.
