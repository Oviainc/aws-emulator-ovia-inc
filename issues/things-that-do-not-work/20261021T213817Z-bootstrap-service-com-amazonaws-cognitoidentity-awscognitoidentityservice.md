Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the Cognito Identity emulator

_Filed: 2026-10-21T21:38:17Z_
_Operation: service-com-amazonaws-cognitoidentity-awscognitoidentityservice | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 23 operations in `services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST, SMALL stateful slice: the identity-pool lifecycle plus the unauthenticated-identity credential flow, which is how virtually every real Cognito Identity client is used.

Wire protocol (README/OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: AWSCognitoIdentityService.<OperationName>`, sigv4 signing name `cognito-identity`.

### Happy path to implement, in order

1. **CreateIdentityPool** — input `IdentityPoolName` (required, len 1..128, pattern `^[\w\s+=,.@-]+$`), `AllowUnauthenticatedIdentities` (required, default false), plus optional `AllowClassicFlow`, `SupportedLoginProviders`, `DeveloperProviderName`, `OpenIdConnectProviderARNs`, `CognitoIdentityProviders`, `SamlProviderARNs`, `IdentityPoolTags`. Must MINT and persist a new `IdentityPoolId` matching the modeled pattern `^[\w-]+:[0-9a-f-]+$` (real AWS form: `<region>:<uuid>`, e.g. `us-east-1:1b2c3d4e-...`) and echo back the full stored pool record.
2. **DescribeIdentityPool** — input `IdentityPoolId` (required). Must return the pool EXACTLY as stored by Create/Update (name, AllowUnauthenticatedIdentities, AllowClassicFlow, provider maps/lists, IdentityPoolTags). Unknown id → `ResourceNotFoundException` (HTTP 404). Id that fails the modeled pattern → `InvalidParameterException` (400).
3. **GetId** — input `IdentityPoolId` (required), optional `AccountId`, optional `Logins` (map, 0..10). Must verify the pool exists (else `ResourceNotFoundException` 404), then mint and PERSIST a new `IdentityId` (`<region>:<uuid>`, modeled pattern `^[\w-]+:[0-9a-f-]+$`) associated with that pool, recording the `Logins` keys as the identity's logins and the creation timestamp. Calling GetId twice with the SAME non-empty `Logins` entry must return the SAME IdentityId (identity lookup is keyed by provider login); an unauthenticated call (no Logins) mints a fresh identity each time. If `AllowUnauthenticatedIdentities` is false and no `Logins` are supplied, fail with `NotAuthorizedException` (403).
4. **GetCredentialsForIdentity** — input `IdentityId` (required), optional `Logins`, optional `CustomRoleArn`. Must look up the persisted identity (unknown → `ResourceNotFoundException` 404) and return `IdentityId` plus a `Credentials` structure with `AccessKeyId`, `SecretKey`, `SessionToken` and a future `Expiration` timestamp, generated per identity and stored so repeated calls within the validity window are consistent. Credentials must NOT be a fixed constant shared by all identities.
5. **ListIdentities** — input `IdentityPoolId` (required), `MaxResults` (required, 1..60), optional `NextToken`, `HideDisabled` (default false). Must return `IdentityPoolId` and the `Identities` list of `IdentityDescription` (`IdentityId`, `Logins`, `CreationDate`, `LastModifiedDate`) for identities actually created by GetId in that pool, truncated to `MaxResults` with an opaque `NextToken` that resumes exactly where the previous page ended (and is absent on the last page). Unknown pool → 404.
6. **DeleteIdentityPool** — input `IdentityPoolId` (required), empty output. Must remove the pool AND its identities from state; a subsequent DescribeIdentityPool / ListIdentities / GetId for that id must fail with `ResourceNotFoundException` (404), and a second DeleteIdentityPool must also 404.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request back, no minting an id that is never stored: a `CreateIdentityPool` followed by `DescribeIdentityPool` must return what was created, and a `GetId` followed by `ListIdentities` must show the identity.

### State Model

In `src/state.rs` (`ComAmazonawsCognitoidentityAwscognitoidentityserviceState`, reached via `crate::state::service_state(&*self.engine)` — never a fresh engine):

- `identity_pools: HashMap<String /* IdentityPoolId */, IdentityPool>` where
  `IdentityPool { identity_pool_id: String, identity_pool_name: String, allow_unauthenticated_identities: bool, allow_classic_flow: Option<bool>, supported_login_providers: BTreeMap<String,String>, developer_provider_name: Option<String>, open_id_connect_provider_arns: Vec<String>, cognito_identity_providers: Vec<CognitoIdentityProvider>, saml_provider_arns: Vec<String>, identity_pool_tags: BTreeMap<String,String>, roles: BTreeMap<String,String>, role_mappings: BTreeMap<String,RoleMapping> }`
  (`roles`/`role_mappings` reserved for the later Set/GetIdentityPoolRoles slice.)
- `identities: HashMap<String /* IdentityId */, Identity>` where
  `Identity { identity_id: String, identity_pool_id: String, logins: Vec<String> /* provider names */, creation_date: SystemTime, last_modified_date: SystemTime }`
- `identities_by_pool: HashMap<String /* IdentityPoolId */, Vec<String> /* IdentityId, insertion-ordered for stable pagination */>`
- `login_index: HashMap<(String /* IdentityPoolId */, String /* "provider:userid" from Logins */), String /* IdentityId */>` so GetId is idempotent per federated login.
- `credentials: HashMap<String /* IdentityId */, IssuedCredentials>` where
  `IssuedCredentials { access_key_id: String, secret_key: String, session_token: String, expiration: SystemTime }`

All of it lives in the single shared engine so every handler in the crate observes the same store across requests.

The remaining 17 operations (GetOpenIdToken, GetOpenIdTokenForDeveloperIdentity, DescribeIdentity, DeleteIdentities, ListIdentityPools, UpdateIdentityPool, Set/GetIdentityPoolRoles, Set/GetPrincipalTagAttributeMap, Tag/Untag/ListTagsForResource, Lookup/Merge/UnlinkDeveloperIdentity, UnlinkIdentity) stay stubbed at 501 for now and will be filed as individual gap issues.

## Evidence

OPERATIONS.md for this crate documents all 23 operations with the exact members cited above: CreateIdentityPool (`IdentityPoolName`, `AllowUnauthenticatedIdentities` required; output `IdentityPoolId` required, pattern `^[\w-]+:[0-9a-f-]+$`), DescribeIdentityPool, GetId (`IdentityPoolId` required, optional `AccountId`/`Logins`; output `IdentityId`), GetCredentialsForIdentity (`IdentityId` required; output `IdentityId` + `Credentials`), ListIdentities (`IdentityPoolId`+`MaxResults` required, `NextToken`, `HideDisabled`; output `Identities` of `IdentityDescription` with `CreationDate`/`LastModifiedDate`), DeleteIdentityPool (no output members) — each declaring `ResourceNotFoundException (HTTP 404)`, `InvalidParameterException (HTTP 400)` and `NotAuthorizedException (HTTP 403)` as used above. All 24 files under src/ops/ (minus mod.rs) carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header on line 4 and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501, so no state exists today. The GetId-idempotency-per-login and GetCredentialsForIdentity credential-shape behaviors are well-established Cognito Identity semantics (the standard unauthenticated/federated identity flow used by the AWS mobile and JS SDKs). No tests assert any of this; only the generated `tests/state_roundtrip.rs` example exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI at it with endpoint override for `cognito-identity`.
2. `aws cognito-identity create-identity-pool --identity-pool-name demo-pool --allow-unauthenticated-identities` → expect 200 with a minted `IdentityPoolId` of the form `<region>:<uuid>`; today it returns HTTP 501 NotImplemented.
3. `aws cognito-identity describe-identity-pool --identity-pool-id <id>` → expect the pool name and `AllowUnauthenticatedIdentities: true` exactly as created.
4. `aws cognito-identity get-id --identity-pool-id <id>` → expect 200 with a minted `IdentityId`; repeat with `--logins graph.facebook.com=tok1` twice and expect the SAME IdentityId both times.
5. `aws cognito-identity get-credentials-for-identity --identity-id <identityId>` → expect `Credentials` with non-empty `AccessKeyId`, `SecretKey`, `SessionToken` and a future `Expiration`, and the echoed `IdentityId`.
6. `aws cognito-identity list-identities --identity-pool-id <id> --max-results 1` → expect exactly one `IdentityDescription` plus a `NextToken`; follow the token and expect the remaining identities with no duplicates and no token on the last page.
7. `aws cognito-identity get-id --identity-pool-id us-east-1:00000000-0000-0000-0000-000000000000` for a pool never created → expect `ResourceNotFoundException` with HTTP 404.
8. `aws cognito-identity delete-identity-pool --identity-pool-id <id>`, then re-run describe-identity-pool and list-identities for that id → expect `ResourceNotFoundException` (404) for both, and a second delete to 404 as well.
