Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.get-credentials-for-identity-full-flow cognito-identity.get-credentials-for-identity-session-duration-bounded cognito-identity.get-credentials-for-identity-sts-session-expires-after-ttl cognito-identity.e2e-anonymous-identity-credentials-to-caller-identity cognito-identity.developer-identity-vends-authenticated-role-credentials cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb cognito-identity.get-identity-pool-roles-rules-based-mapping-selects-role-by-claim cognito-identity.get-identity-pool-roles-rules-evaluated-in-declared-order cognito-identity.role-mapping-rules-claim-match-selects-role cognito-identity.role-mapping-ambiguous-resolution-deny-blocks-credentials cognito-identity.server-side-role-selection-ignores-client-supplied-hints-rejected cognito-identity.server-side-token-check-refuses-a-globally-signed-out-user cognito-identity.allow-classic-flow-disabled-refuses-get-open-id-token cognito-identity.sts-credentials-ttl-bound-by-role-max-session-duration cognito-identity.trust-policy-sub-condition-admits-only-the-named-identity cognito-identity.vended-credentials-bounded-by-role-policy cognito-identity.principal-tag-condition-denies-the-other-tenants-object cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry cognito-identity.e2e-credentials-for-identity-without-roles-rejected cognito-identity.get-credentials-without-roles-configured-fails cognito-identity.get-credentials-ambiguous-role-resolution-denied cognito-identity.untrusted-role-accepted-then-credentials-denied cognito-identity.server-side-sts-role-session-tags-enforced-in-trust-policy cognito-identity.a-token-minted-by-another-pool-is-refused-and-vends-nothing
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.get-credentials-for-identity-full-flow|run#29|AssertionError|assert arn_ok, (
Issue-Key: cognito-identity.get-credentials-for-identity-session-duration-bounded|run#20|AssertionError|assert role_name in caller_arn, (
Issue-Key: cognito-identity.get-credentials-for-identity-sts-session-expires-after-ttl|run#18|AssertionError|assert role_name in caller["Arn"], (
Issue-Key: cognito-identity.e2e-anonymous-identity-credentials-to-caller-identity|run#20|AssertionError|assert f"assumed-role/{role_name}/" in caller["Arn"], caller["Arn"]
Issue-Key: cognito-identity.developer-identity-vends-authenticated-role-credentials|run#20|AssertionError|assert creds["AccessKeyId"].startswith("ASIA"), (
Issue-Key: cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb|run#29|AssertionError|assert vended_arn.split("/")[1] == auth_role_name, (
Issue-Key: cognito-identity.get-identity-pool-roles-rules-based-mapping-selects-role-by-claim|run#20|AssertionError|assert f"assumed-role/{matched_role_name}/" in arn_match, (
Issue-Key: cognito-identity.get-identity-pool-roles-rules-evaluated-in-declared-order|run#20|AssertionError|assert f"assumed-role/{role_a_name}/" in arn1, f"expected role A (first rule) to win, got {arn1}"
Issue-Key: cognito-identity.role-mapping-rules-claim-match-selects-role|_assumed_arn#20|AssertionError|assert creds["AccessKeyId"].startswith("ASIA"), (
Issue-Key: cognito-identity.role-mapping-ambiguous-resolution-deny-blocks-credentials|run#17|AssertionError|assert f":assumed-role/{rule_name}/" in control_arn, (
Issue-Key: cognito-identity.server-side-role-selection-ignores-client-supplied-hints-rejected|run#12|AssertionError|assert f"assumed-role/{role_a_name}/" in arn, f"expected role A by default, got {arn}"
Issue-Key: cognito-identity.server-side-token-check-refuses-a-globally-signed-out-user|run#25|AssertionError|assert f":assumed-role/{checked_role}/" in checked_arn, (
Issue-Key: cognito-identity.allow-classic-flow-disabled-refuses-get-open-id-token|run#17|AssertionError|assert f":assumed-role/{role_name}/" in enhanced_arn, (
Issue-Key: cognito-identity.sts-credentials-ttl-bound-by-role-max-session-duration|run#20|AssertionError|assert role_name in caller["Arn"], (
Issue-Key: cognito-identity.trust-policy-sub-condition-admits-only-the-named-identity|run#16|AssertionError|assert f":assumed-role/{role_name}/" in arn_a, (
Issue-Key: cognito-identity.vended-credentials-bounded-by-role-policy|run#14|AssertionError|assert f":assumed-role/{unauth_name}/{SESSION_NAME}" in caller_arn, (
Issue-Key: cognito-identity.principal-tag-condition-denies-the-other-tenants-object|run#40|AssertionError|assert cross_code is not None, (
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry|run#21|AssertionError|assert role_name in caller["Arn"] and "assumed-role" in caller["Arn"], (
Issue-Key: cognito-identity.e2e-credentials-for-identity-without-roles-rejected|_client_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: cognito-identity.get-credentials-without-roles-configured-fails|_refusal#4|AssertionError|raise AssertionError(
Issue-Key: cognito-identity.get-credentials-ambiguous-role-resolution-denied|_client_error#17|AssertionError|raise AssertionError(
Issue-Key: cognito-identity.untrusted-role-accepted-then-credentials-denied|run#14|AssertionError|raise AssertionError(
Issue-Key: cognito-identity.server-side-sts-role-session-tags-enforced-in-trust-policy|_error#19|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: cognito-identity.a-token-minted-by-another-pool-is-refused-and-vends-nothing|run#15|AssertionError|assert code is not None, (
Issue-Commit: d0dc720ebbf11903c1fa9407d1c82e53eafd4b75

# cognito-identity: GetCredentialsForIdentity never performs a real AssumeRoleWithWebIdentity — it vends the emulator's static IAM-user credentials and refuses nothing (5 causes)

> _Note: this issue was identified against the code base at commit `d0dc720ebbf11903c1fa9407d1c82e53eafd4b75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
This issue bundles FIVE distinct root causes in the cognito-identity enhanced-flow credential path. All five must be fixed and re-replayed before closing.

=== Cause 1 of 5: the vended Credentials are not an STS session of the pool's resolved role ===
AWS: GetCredentialsForIdentity resolves the identity's role (Roles['authenticated'|'unauthenticated'], a RoleMappings rule, or CustomRoleArn) and returns the Credentials of an sts:AssumeRoleWithWebIdentity session for exactly that role. Smithy (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model, GetCredentialsForIdentity): output Credentials {AccessKeyId, SecretKey, SessionToken, Expiration}, auth [] / optionalAuth true. Recorded behavior: the AccessKeyId is a temporary-session key (ASIA… prefix) and sts:GetCallerIdentity under those credentials answers Arn = arn:aws:sts::<account>:assumed-role/<resolved-role-name>/CognitoIdentityCredentials.
Emulator: the response carries a long-lived-looking key with an 'EMULATORAS…' prefix, and GetCallerIdentity under it answers arn:aws:iam::000000000000:user/emulator — i.e. the emulator's own ambient principal. No role is assumed, the pool's role configuration/role mapping is never applied to the session, and the Expiration is cosmetic.
Why: every client that federates through Cognito gets the emulator's blanket principal instead of the scoped role, so all downstream authorization (role inline policies, principal tags, resource policies, KMS grants) is silently bypassed; code that parses ':assumed-role/<role>/' out of the caller ARN crashes, and session-TTL behavior cannot be exercised.
Fix: the GetCredentialsForIdentity handler (ops/get_credentials_for_identity.rs) must resolve the role for the identity and mint the session through the emulator's STS AssumeRoleWithWebIdentity path, so the credentials map to an assumed-role principal.
Traces: cognito-identity.get-credentials-for-identity-full-flow, cognito-identity.get-credentials-for-identity-session-duration-bounded, cognito-identity.get-credentials-for-identity-sts-session-expires-after-ttl, cognito-identity.e2e-anonymous-identity-credentials-to-caller-identity, cognito-identity.developer-identity-vends-authenticated-role-credentials, cognito-identity.developer-authenticated-credentials-write-and-read-dynamodb, cognito-identity.get-identity-pool-roles-rules-based-mapping-selects-role-by-claim, cognito-identity.get-identity-pool-roles-rules-evaluated-in-declared-order, cognito-identity.role-mapping-rules-claim-match-selects-role, cognito-identity.role-mapping-ambiguous-resolution-deny-blocks-credentials, cognito-identity.server-side-role-selection-ignores-client-supplied-hints-rejected, cognito-identity.server-side-token-check-refuses-a-globally-signed-out-user, cognito-identity.allow-classic-flow-disabled-refuses-get-open-id-token, cognito-identity.sts-credentials-ttl-bound-by-role-max-session-duration, cognito-identity.trust-policy-sub-condition-admits-only-the-named-identity, cognito-identity.vended-credentials-bounded-by-role-policy, cognito-identity.principal-tag-condition-denies-the-other-tenants-object, cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry.

=== Cause 2 of 5: a pool with no roles configured still vends credentials ===
AWS: GetCredentialsForIdentity models InvalidIdentityPoolConfigurationException (HTTP 400). When the pool has no Roles set for the identity's auth class, the call is refused — no credentials exist to vend.
Emulator: HTTP 200 with a full Credentials block for an identity in a pool that has never had SetIdentityPoolRoles applied.
Why: the single most common Cognito misconfiguration (pool deployed without role attachment) looks healthy against the emulator and fails only in production; the client's handler for InvalidIdentityPoolConfigurationException is dead code.
Fix: GetCredentialsForIdentity handler — refuse with InvalidIdentityPoolConfigurationException (400) when role resolution yields no role.
Traces: cognito-identity.e2e-credentials-for-identity-without-roles-rejected, cognito-identity.get-credentials-without-roles-configured-fails.

=== Cause 3 of 5: a Deny role resolution is not honored ===
AWS: when a RoleMapping's rules do not match and AmbiguousRoleResolution/Deny applies, GetCredentialsForIdentity is refused (HTTP 400 InvalidIdentityPoolConfigurationException / NotAuthorizedException per the recorded response) and nothing is vended.
Emulator: HTTP 200 with credentials for a login whose claims match no rule under a Deny resolution.
Why: role mapping is the authorization decision point of the enhanced flow; a Deny that vends credentials is a privilege-escalation gap.
Fix: the role-resolution step of GetCredentialsForIdentity must propagate a Deny outcome into the modeled refusal instead of falling back to a default principal.
Traces: cognito-identity.get-credentials-ambiguous-role-resolution-denied.

=== Cause 4 of 5: the resolved role's trust policy is never evaluated ===
AWS: because the session is minted through sts:AssumeRoleWithWebIdentity, a role that does not trust cognito-identity.amazonaws.com, or whose trust policy carries a condition the request does not satisfy (e.g. an sts:TagKeys / session-tag condition), cannot be assumed, so the call is refused rather than vending.
Emulator: credentials are vended for a role whose trust policy names no Cognito federated principal, and for a role whose trust policy requires session tags that are not supplied.
Why: trust-policy mistakes — the most common cause of real GetCredentialsForIdentity failures — are invisible here, and 'who may federate into this role' is unenforced.
Fix: route role assumption through the STS trust-policy evaluation path (same evaluation STS AssumeRoleWithWebIdentity uses) and surface its refusal from GetCredentialsForIdentity.
Traces: cognito-identity.untrusted-role-accepted-then-credentials-denied, cognito-identity.server-side-sts-role-session-tags-enforced-in-trust-policy.

=== Cause 5 of 5: an OpenID token minted by one identity pool is accepted for an identity in another pool ===
AWS: the token presented in Logins['cognito-identity.amazonaws.com'] is bound to the pool that minted it; presenting it for an IdentityId belonging to a different pool is refused (NotAuthorizedException, HTTP 403) and nothing is vended.
Emulator: HTTP 200 with credentials — the token's issuing pool is not checked against the IdentityId's pool.
Why: cross-tenant isolation between identity pools is absent; any pool's token unlocks any other pool's identities.
Fix: GetCredentialsForIdentity handler — validate the Logins token's issuer/aud against the pool that owns the IdentityId before resolving a role.
Traces: cognito-identity.a-token-minted-by-another-pool-is-refused-and-vends-nothing.

Related but NOT covering: open issue 20261024T065715Z-cloudtrail-cognito-identity-sts-unsigned-cognito-identity-requests-are-answered-unknownservice-so.md recorded the earlier state of this path (unsigned calls answered UnknownService); routing now works and the call answers 200, so the defects above are what is left. None of the open cognito-identity issues carries these checks' replay keys.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 (all details quoted from the failing-check list):

Cause 1 — the vended session is the emulator's ambient IAM user:
- get-credentials-for-identity-full-flow: "vended creds resolved to 'arn:aws:iam::000000000000:user/emulator', expected an assumed-role session ARN under the unauthenticated role in this account".
- get-credentials-for-identity-session-duration-bounded: "credentials with a plausible Expiration did not actually assume sim2real-cog-unauth-<id>: arn:aws:iam::000000000000:user/emulator".
- get-credentials-for-identity-sts-session-expires-after-ttl: "credentials resolve to 'arn:aws:iam::000000000000:user/emulator', expected the trusted role 'sim2real-cid-ststtl-role-<id>'".
- developer-identity-vends-authenticated-role-credentials: "vended AccessKeyId EMULATORAS5V7QW3BW5G is not an ASIA session key"; role-mapping-rules-claim-match-selects-role: "vended AccessKeyId EMULATORASFI3ADFXM4I is not an ASIA session key".
- vended-credentials-bounded-by-role-policy: "vended credentials must be a session of the unauthenticated role sim2real-behav-bounded-<id> named CognitoIdentityCredentials, got arn:aws:iam::000000000000:user/emulator".
- e2e-anonymous-identity-credentials-to-caller-identity: "AssertionError: arn:aws:iam::000000000000:user/emulator".
- get-identity-pool-roles-rules-based-mapping-selects-role-by-claim: "matching identity did not assume the rule-specific role: 'arn:aws:iam::000000000000:user/emulator'"; -rules-evaluated-in-declared-order: "expected role A (first rule) to win, got arn:aws:iam::000000000000:user/emulator".
- principal-tag-condition-denies-the-other-tenants-object: "tenant-a's session read tenant-b's object; the ${aws:PrincipalTag/tenant} scope granted nothing it should have".
- Same ARN in the control legs of allow-classic-flow-disabled-refuses-get-open-id-token, server-side-token-check-refuses-a-globally-signed-out-user, trust-policy-sub-condition-admits-only-the-named-identity, role-mapping-ambiguous-resolution-deny-blocks-credentials, server-side-role-selection-ignores-client-supplied-hints-rejected, sts-credentials-ttl-bound-by-role-max-session-duration, developer-authenticated-credentials-write-and-read-dynamodb, get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry.

Cause 2 — get-credentials-without-roles-configured-fails / e2e-credentials-for-identity-without-roles-rejected: "expected the call to be refused, but it SUCCEEDED and returned {'IdentityId': 'us-west-2:…', 'Credentials': {'AccessKeyId': 'EMULATORASZBDIXJ7KL7', …}, 'ResponseMetadata': {'HTTPStatusCode': 200, …}}".

Cause 3 — get-credentials-ambiguous-role-resolution-denied: "expected GetCredentialsForIdentity to be denied for a Deny-resolved, non-matching login, but it SUCCEEDED and returned {… 'HTTPStatusCode': 200 …}".

Cause 4 — untrusted-role-accepted-then-credentials-denied: "GetCredentialsForIdentity must fail when the mapped role does not trust cognito-identity, but it vended 'EMULATORAS77WQIEJ7X2' for role sim2real-behav-untrusted-<id>"; server-side-sts-role-session-tags-enforced-in-trust-policy: "expected a ClientError, but the call succeeded and returned {… 'HTTPStatusCode': 200 …}".

Cause 5 — a-token-minted-by-another-pool-is-refused-and-vends-nothing: "a token minted by pool 'us-west-2:03d016cb-…' must not vend credentials for an identity in pool 'us-west-2:e7a82751-…', but the call returned {'IdentityId': …, 'Credentials': {…}, 'ResponseMetadata': {'HTTPStatusCode': 200 …}}".

Model (smithy_query operation=GetCredentialsForIdentity): errors ExternalServiceException 400, InvalidIdentityPoolConfigurationException 400, InvalidParameterException 400, NotAuthorizedException 403, ResourceConflictException 409, ResourceNotFoundException 404, TooManyRequestsException 429; output Credentials.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay the check ids in trace_ids.
2. Cause 1: CreateIdentityPool(AllowUnauthenticatedIdentities=True); create an IAM role trusting cognito-identity.amazonaws.com for sts:AssumeRoleWithWebIdentity; SetIdentityPoolRoles(Roles={'unauthenticated': <role arn>}); GetId; GetCredentialsForIdentity(IdentityId=<id>) -> expect an ASIA… AccessKeyId whose sts:GetCallerIdentity Arn is arn:aws:sts::<account>:assumed-role/<role>/CognitoIdentityCredentials; observe an 'EMULATORAS…' key resolving to arn:aws:iam::000000000000:user/emulator.
3. Cause 2: same pool WITHOUT any SetIdentityPoolRoles call, then GetCredentialsForIdentity -> expect 400 InvalidIdentityPoolConfigurationException; observe 200 with Credentials.
4. Cause 3: SetIdentityPoolRoles with a RoleMapping Type=Rules, AmbiguousRoleResolution=Deny, and a rule that the presented claim does not match; GetCredentialsForIdentity -> expect the refusal; observe 200.
5. Cause 4: point Roles at a role whose trust policy names an unrelated principal (or requires session tags that are not supplied) -> expect the refusal; observe 200 with credentials.
6. Cause 5: create pools A and B; mint an OpenID token in pool A and present it in Logins for an IdentityId from pool B -> expect 403 NotAuthorizedException; observe 200 with Credentials.
