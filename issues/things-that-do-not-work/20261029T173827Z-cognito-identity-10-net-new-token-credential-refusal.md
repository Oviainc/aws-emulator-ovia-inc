Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.get-open-id-token-jwt-structure cognito-identity.deep-cognito-identity-2-oidc-token-integrity-sts-detects-one-byte-tamper cognito-identity.get-credentials-for-identity-returns-sts-creds cognito-identity.swapping-the-authenticated-role-moves-the-next-session-and-spares-the-live-one cognito-identity.unauthenticated-upgrade-keeps-the-identity-but-the-guest-role-is-denied-the-privileged-prefix cognito-identity.server-side-principal-tag-attribute-map-enforced cognito-identity.get-credentials-for-identity-invalid-token-rejected cognito-identity.get-id-malformed-login-token-format-rejected cognito-identity.get-id-rejects-unconfigured-login-provider cognito-identity.deep-cognito-identity-1-unsigned-public-api-linked-identity-credentials-without-logins-rejected cognito-identity.deep-cognito-identity-1-unsigned-public-api-control-plane-rejected-while-credentials-vend cognito-identity.deep-cognito-identity-1-unsigned-public-api-delete-identities-rejected cognito-identity.deep-cognito-identity-1-unsigned-public-api-unlink-identity-with-another-users-login-rejected cognito-identity.delete-identities-then-not-found cognito-identity.delete-identity-pool-cascades-to-identities cognito-identity.delete-pool-invalidates-identities cognito-identity.vended-session-outlives-pool-deletion-until-the-role-policy-is-revoked cognito-identity.get-id-rejects-unauthenticated-when-disallowed cognito-identity.merge-developer-identities-self-merge-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.get-open-id-token-jwt-structure|_decode_jwt#10|AssertionError|assert parts[2], "signature segment must be non-empty"
Issue-Key: cognito-identity.deep-cognito-identity-2-oidc-token-integrity-sts-detects-one-byte-tamper|run#24|AssertionError|assert header.get("alg") == EXPECTED_ALG, (
Issue-Key: cognito-identity.get-credentials-for-identity-returns-sts-creds|run#18|AssertionError|assert c["AccessKeyId"].startswith("AS"), f"unexpected access key form: {c['AccessKeyId']!r}"
Issue-Key: cognito-identity.swapping-the-authenticated-role-moves-the-next-session-and-spares-the-live-one|run#27|AssertionError|assert old_new_code == DENIED_CODE, (
Issue-Key: cognito-identity.unauthenticated-upgrade-keeps-the-identity-but-the-guest-role-is-denied-the-privileged-prefix|run#26|AssertionError|assert guest_priv_code is not None, (
Issue-Key: cognito-identity.server-side-principal-tag-attribute-map-enforced|_poll_until#17|AssertionError|raise AssertionError(f"poll condition not met within {timeout_s}s ({description}): {result!r}")
Issue-Key: cognito-identity.get-credentials-for-identity-invalid-token-rejected|_client_error#16|AssertionError|raise AssertionError(
Issue-Key: cognito-identity.get-id-malformed-login-token-format-rejected|_refusal#9|AssertionError|raise AssertionError(f"expected GetId to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cognito-identity.get-id-rejects-unconfigured-login-provider|_refusal#7|AssertionError|raise AssertionError(
Issue-Key: cognito-identity.deep-cognito-identity-1-unsigned-public-api-linked-identity-credentials-without-logins-rejected|_error_code#18|AssertionError|raise AssertionError(f"call was expected to be refused but succeeded: {resp!r}")
Issue-Key: cognito-identity.deep-cognito-identity-1-unsigned-public-api-control-plane-rejected-while-credentials-vend|run#25|AssertionError|assert code == ERR_UNSIGNED, (
Issue-Key: cognito-identity.deep-cognito-identity-1-unsigned-public-api-delete-identities-rejected|run#7|AssertionError|assert code == ERR_UNSIGNED, f"unsigned DeleteIdentities answered {code!r}, expected {ERR_UNSIGNED!r}"
Issue-Key: cognito-identity.deep-cognito-identity-1-unsigned-public-api-unlink-identity-with-another-users-login-rejected|run#12|AssertionError|assert code == ERR_NOT_AUTHORIZED, (
Issue-Key: cognito-identity.delete-identities-then-not-found|_assert_not_found#14|AssertionError|assert status == 400, f"{label}: expected HTTP 400, got {status!r}"
Issue-Key: cognito-identity.delete-identity-pool-cascades-to-identities|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-identity.delete-pool-invalidates-identities|run#9|AssertionError|assert err == not_found, f"expected {not_found}, got {err}"
Issue-Key: cognito-identity.vended-session-outlives-pool-deletion-until-the-role-policy-is-revoked|run#30|AssertionError|assert vend_message == expected, (
Issue-Key: cognito-identity.get-id-rejects-unauthenticated-when-disallowed|run#8|AssertionError|assert message == "Unauthenticated access is not supported for this identity pool.", message
Issue-Key: cognito-identity.merge-developer-identities-self-merge-rejected|run#21|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40

# cognito-identity: 10 net-new token/credential/refusal root causes — OpenID tokens are unsigned (alg none), forged logins are accepted, vended sessions are authorization-unbounded, unsigned calls answer AccessDenied

> _Note: this issue was identified against the code base at commit `c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
Ten distinct root causes on the cognito-identity token/credential path. Each is a separate work item; all ten must be closed before submitting. Open issues that are related but do NOT carry these replay keys: 20261028T221339Z-cognito-identity-3-net-new-getcredentialsforidentity-root-causes.md, 20261029T071507Z-cognito-identity-getcredentialsforidentity-under-a-token-role-mapping.md, 20261026T234020Z-cognito-identity-4-net-new-root-causes-web.md, 20261026T054328Z-cognito-identity-4-net-new-root-causes-a.md, 20261025T005354Z-cognito-identity-getid-admits-unvalidatable-login-tokens-setprincipaltagattributemap.md, 20261023T185902Z-cognito-identity-unsigned-anonymous-requests-answer-unknownservice-and.md.

Cause 1 of 10 — GetOpenIdToken / GetOpenIdTokenForDeveloperIdentity mint an UNSIGNED JWT.
AWS: recorded behavior only (Smithy only types the member: GetOpenIdTokenResponse.Token -> OIDCToken, @sensitive): the OIDC token is a three-segment JWS whose JOSE header carries alg "RS512" and whose signature segment is non-empty, so STS AssumeRoleWithWebIdentity detects a one-byte tamper.
Emulator: the header's alg is the string "none" and the signature segment is empty.
Why: the token carries no integrity at all — any party can forge or edit one and every verifier that requires a signed assertion (STS web-identity federation, any OIDC library) must either reject the emulator's token or be configured to accept unsigned tokens, which is exactly the posture the check exists to forbid.
Fix: the OIDC token issuance/signing component behind both GetOpenIdToken operations in the cognito-identity service crate.
Traces: cognito-identity.get-open-id-token-jwt-structure, cognito-identity.deep-cognito-identity-2-oidc-token-integrity-sts-detects-one-byte-tamper.

Cause 2 of 10 — GetCredentialsForIdentity vends an AccessKeyId that is not an STS temporary-credential key id.
AWS: temporary session credentials carry an AccessKeyId beginning with the ASIA prefix (recorded behavior only; the model types it as Credentials.AccessKeyId).
Emulator: the key id begins with "EMULATOR" (e.g. EMULATORASIMYPI6WUDG).
Why: SDKs, policy conditions and audit tooling that classify credentials by key-id prefix see a long-term key where a session key is required. Already reported for other operations in 20261028T221339Z / 20261029T071507Z / 20261026T234020Z; this is the plain GetCredentialsForIdentity happy path, which those do not list.
Fix: the credential-vending component shared by GetCredentialsForIdentity.
Traces: cognito-identity.get-credentials-for-identity-returns-sts-creds.

Cause 3 of 10 — credentials vended for an identity are not bounded by the mapped role's policy.
AWS: recorded behavior only: the vended session IS a session of the mapped role, so S3 calls outside the role's allowed prefix are refused AccessDenied, swapping the pool's authenticated role moves the NEXT session's permissions, and a role lacking sts:TagSession cannot be assumed while PrincipalTags are configured.
Emulator: the calls succeed — the pre-swap role granting only old/ still reads new/obj, a guest role granting only readable/ still reads privileged/obj, and a role with no sts:TagSession still vends.
Why: every least-privilege guarantee a client tests against an identity pool is unenforced, so authorization regressions pass silently. Related to 20261026T054328Z-cognito-identity-4-net-new-root-causes-a.md, which does not carry these keys.
Fix: the role-assumption/session component behind GetCredentialsForIdentity and the authorization evaluation of the resulting session.
Traces: cognito-identity.swapping-the-authenticated-role-moves-the-next-session-and-spares-the-live-one, cognito-identity.unauthenticated-upgrade-keeps-the-identity-but-the-guest-role-is-denied-the-privileged-prefix, cognito-identity.server-side-principal-tag-attribute-map-enforced.

Cause 4 of 10 — GetId / GetCredentialsForIdentity accept login tokens that cannot be validated and providers the pool is not configured with.
AWS: NotAuthorizedException (Smithy: GetId and GetCredentialsForIdentity both model NotAuthorizedException, @httpError 403). For a pool with no SupportedLoginProviders entry for the key used, the message is "Identity pool - <identity-pool-id> does not have identity providers configured." (fixed text with the pool id from the request). A forged/malformed provider token is likewise refused, and a linked identity cannot vend credentials without the Logins that linked it.
Emulator: HTTP 200 — GetId mints an identity for a bogus provider token, and GetCredentialsForIdentity returns Credentials for a forged token and for a linked identity called with no Logins.
Why: the whole federation trust boundary is open: anything presented as a login is believed, so a client cannot distinguish a valid user from an attacker. Related to 20261025T005354Z-cognito-identity-getid-admits-unvalidatable-login-tokens-setprincipaltagattributemap.md, which does not carry these keys.
Fix: login-token validation in the GetId and GetCredentialsForIdentity handlers (provider configured for the pool, token issuer/audience/signature, Logins required for a linked identity).
Traces: cognito-identity.get-credentials-for-identity-invalid-token-rejected, cognito-identity.get-id-malformed-login-token-format-rejected, cognito-identity.get-id-rejects-unconfigured-login-provider, cognito-identity.deep-cognito-identity-1-unsigned-public-api-linked-identity-credentials-without-logins-rejected.

Cause 5 of 10 — an UNSIGNED request to an operation that requires SigV4 answers the wrong error.
AWS: recorded behavior only: a request with no SigV4 credentials to a signed control-plane operation (CreateIdentityPool, DeleteIdentities) is refused with error code MissingAuthenticationTokenException. (Smithy marks only the public operations — GetId, GetCredentialsForIdentity, GetOpenIdToken, UnlinkIdentity — @optionalAuth, so the rest require auth.)
Emulator: error code 'AccessDenied'.
Why: a client distinguishing "you sent no credentials" from "your credentials were refused" picks the wrong remedy, and the unsigned-vs-signed boundary of the public API is unobservable.
Fix: the request-authentication layer's error mapping for unsigned requests in the cognito-identity service.
Traces: cognito-identity.deep-cognito-identity-1-unsigned-public-api-control-plane-rejected-while-credentials-vend, cognito-identity.deep-cognito-identity-1-unsigned-public-api-delete-identities-rejected.

Cause 6 of 10 — unsigned UnlinkIdentity carrying another identity's login is classified as a conflict.
AWS: NotAuthorizedException (modeled on UnlinkIdentity, @httpError 403) — identity A unlinked with identity B's token is an authorization failure.
Emulator: ResourceConflictException.
Why: a cross-user tampering attempt is reported as a state conflict, so clients retry rather than treat it as a denial.
Fix: the UnlinkIdentity handler's caller-vs-identity authorization check and its error mapping.
Traces: cognito-identity.deep-cognito-identity-1-unsigned-public-api-unlink-identity-with-another-users-login-rejected.

Cause 7 of 10 — ResourceNotFoundException is served with HTTP 404 instead of 400.
AWS: recorded behavior only: DescribeIdentity for a deleted identity answers HTTP 400 with code ResourceNotFoundException and message "Identity '<identity-id>' not found." — note the Smithy model annotates ResourceNotFoundException @httpError(404), but the recorded response status for this JSON-1.1 service is 400, and the suite asserts 400.
Emulator: HTTP 404 (the message and code match).
Why: clients and proxies that branch on the HTTP status (4xx retry policy, 404-means-no-route handling) classify the response differently from AWS.
Fix: the error serializer's status mapping for ResourceNotFoundException in the cognito-identity service.
Traces: cognito-identity.delete-identities-then-not-found.

Cause 8 of 10 — after the identity pool is deleted, identity-scoped calls name the identity instead of the pool.
AWS: ResourceNotFoundException whose message names the POOL: "IdentityPool '<identity-pool-id>' not found." (fixed text with the pool id from the request/state) — the pool is checked before the identity.
Emulator: ResourceNotFoundException with "Identity '<identity-id>' not found.".
Why: the client cannot tell "your pool is gone" (reprovision) from "this identity is gone" (re-GetId), and the pool-before-identity resolution order is observably inverted.
Fix: the lookup order and message construction in the GetId / GetCredentialsForIdentity / DescribeIdentity paths after DeleteIdentityPool.
Traces: cognito-identity.delete-identity-pool-cascades-to-identities, cognito-identity.delete-pool-invalidates-identities, cognito-identity.vended-session-outlives-pool-deletion-until-the-role-policy-is-revoked.

Cause 9 of 10 — the unauthenticated-disallowed refusal message interpolates the pool id.
AWS: NotAuthorizedException with the fixed text "Unauthenticated access is not supported for this identity pool." — no variable parts.
Emulator: "Unauthenticated access is not supported for the identity pool '<identity-pool-id>'.".
Why: the code is right but the message is not AWS's, so clients that match the wording (and the suite) see an unknown refusal.
Fix: the GetId handler's refusal message when AllowUnauthenticatedIdentities is false.
Traces: cognito-identity.get-id-rejects-unauthenticated-when-disallowed.

Cause 10 of 10 — MergeDeveloperIdentities with a SourceUserIdentifier that is not linked.
AWS: ResourceNotFoundException (modeled on MergeDeveloperIdentities) — the named developer user does not exist in the pool.
Emulator: NotAuthorizedException.
Why: a caller cannot distinguish "that developer user was never registered" from "you may not merge it", so provisioning code that creates-on-missing never triggers.
Fix: the MergeDeveloperIdentities handler's source-link lookup and error mapping.
Traces: cognito-identity.merge-developer-identities-self-merge-rejected.

## Evidence
Cause 1: "JOSE header alg must be exactly 'RS512' (never 'none', never merely truthy), got 'none'" and "signature segment must be non-empty".
Cause 2: "unexpected access key form: 'EMULATORASIMYPI6WUDG'" (expected an ASIA-prefixed session key).
Cause 3: "the pre-swap role grants old/ only, so new/obj must be refused with 'AccessDenied', got None: None"; "the unauthenticated role grants readable/ only, so the guest session must not read privileged/obj, but it returned b'sim2real-privileged-...'"; "poll condition not met within 30s (role lacking sts:TagSession rejected while PrincipalTags are configured)" followed by a successful GetCredentialsForIdentity response.
Cause 4: "real AWS rejects a forged login token, but the call SUCCEEDED and returned {'IdentityId': ..., 'Credentials': {...}}"; "expected GetId to be rejected, but it SUCCEEDED and returned {'IdentityId': 'us-west-2:ac9745f5-...'}"; expected ('NotAuthorizedException', 'Identity pool - <pool-id> does not have identity providers configured.') but GetId returned 200; "call was expected to be refused but succeeded: {'IdentityId': ..., 'Credentials': {...}}".
Cause 5: "unsigned CreateIdentityPool answered 'AccessDenied', expected 'MissingAuthenticationTokenException'"; "unsigned DeleteIdentities answered 'AccessDenied', expected 'MissingAuthenticationTokenException'".
Cause 6: "unsigned UnlinkIdentity of A with B's token answered 'ResourceConflictException', expected 'NotAuthorizedException'".
Cause 7: "DescribeIdentity after delete: expected HTTP 400, got 404".
Cause 8: expected ("ResourceNotFoundException", "IdentityPool 'us-west-2:842f2a7d-...' not found."), got ("ResourceNotFoundException", "Identity 'us-west-2:dbd7a5f9-...' not found."); same shape from GetCredentialsForIdentity after the pool was deleted.
Cause 9: expected 'Unauthenticated access is not supported for this identity pool.', got "Unauthenticated access is not supported for the identity pool 'us-west-2:a0f44cfb-...'.".
Cause 10: "MergeDeveloperIdentities with an unlinked SourceUserIdentifier 'sim2real-user-ghost-does-not-exist' raised 'NotAuthorizedException', expected ResourceNotFoundException".

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay exactly the checks in trace_ids; each cause's own checks are listed under its 'Traces:' line.
3. By hand for Cause 1: CreateIdentityPool, GetId, GetOpenIdToken -> base64url-decode the token's first segment; the JOSE header must read {"alg":"RS512",...} and the third segment must be non-empty.
4. By hand for Cause 4: CreateIdentityPool with no SupportedLoginProviders, then GetId(IdentityPoolId=pool, Logins={'graph.facebook.com': 'totally-bogus-provider-token'}) must answer NotAuthorizedException 'Identity pool - <pool-id> does not have identity providers configured.' and mint no identity.
5. By hand for Cause 7: DeleteIdentities then DescribeIdentity on the deleted id — the response must be HTTP 400 with ResourceNotFoundException.
