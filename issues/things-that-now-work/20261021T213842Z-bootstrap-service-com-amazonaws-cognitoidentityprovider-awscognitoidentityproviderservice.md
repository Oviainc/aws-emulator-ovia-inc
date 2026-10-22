Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the Amazon Cognito Identity Provider (cognito-idp) emulator

_Filed: 2026-10-21T21:38:42Z_
_Operation: service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold: `src/ops/*.rs` each carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and return `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, and `src/state.rs` holds no Cognito data. There is no working emulator yet.

Implement a FIRST, SMALL happy-path slice: the admin user-pool lifecycle that every Cognito integration test starts with. Wire protocol is `awsJson1_1`, `POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<OperationName>` (OPERATIONS.md "Wire protocol").

The six operations, in order, with the fields each must honour (names from OPERATIONS.md):

1. **CreateUserPool** — in: `PoolName` (required), optional `Policies`, `MfaConfiguration`, `UsernameAttributes`, `AutoVerifiedAttributes`, `Schema`, `UserPoolTags`, `AdminCreateUserConfig`, `DeletionProtection`, `UserPoolTier`. out: `UserPool` (UserPoolType). Must mint a real pool id matching the modeled pattern `^[\w-]+_[0-9a-zA-Z]+$` (e.g. `us-east-1_aB3dEf9Xy`), store the pool, and return `UserPool.Id`, `Name`, `Arn`, `CreationDate`, `LastModifiedDate`, plus the configuration as persisted (defaults filled in, not merely echoed).

2. **CreateUserPoolClient** — in: `UserPoolId` (required), `ClientName` (required), `GenerateSecret`, `ExplicitAuthFlows`, `RefreshTokenValidity`/`AccessTokenValidity`/`IdTokenValidity`, `ReadAttributes`/`WriteAttributes`. out: `UserPoolClient` (UserPoolClientType) with `ClientId` (pattern `^[\w+]+$`), `ClientSecret` only when `GenerateSecret=true`, `UserPoolId`, `ClientName`, `CreationDate`, `LastModifiedDate`. Unknown `UserPoolId` -> `ResourceNotFoundException` (HTTP 404).

3. **AdminCreateUser** — in: `UserPoolId` + `Username` (required), `UserAttributes`, `TemporaryPassword`, `MessageAction`, `DesiredDeliveryMediums`. out: `User` (UserType) with `Username`, `Attributes` (including a generated `sub`), `UserCreateDate`, `UserLastModifiedDate`, `Enabled=true`, `UserStatus=FORCE_CHANGE_PASSWORD`. Creating the same username twice in one pool -> `UsernameExistsException` (HTTP 400); unknown pool -> `ResourceNotFoundException`.

4. **AdminSetUserPassword** — in: `UserPoolId`, `Username`, `Password` (required), `Permanent`. out: empty. Must store the password on the stored user and, when `Permanent=true`, flip `UserStatus` to `CONFIRMED`. Unknown user -> `UserNotFoundException` (HTTP 404).

5. **AdminInitiateAuth** — in: `UserPoolId`, `ClientId`, `AuthFlow` (required), `AuthParameters`. out: `AuthenticationResult` / `ChallengeName` / `Session` / `ChallengeParameters`. For `AuthFlow=ADMIN_USER_PASSWORD_AUTH` with `AuthParameters{USERNAME, PASSWORD}`: verify the username exists in the pool and the password matches what AdminSetUserPassword stored, then return `AuthenticationResult{AccessToken, IdToken, RefreshToken, TokenType="Bearer", ExpiresIn}` whose tokens are issued and recorded by the emulator (JWT-shaped, with `sub`/`username`/`client_id` claims taken from stored state). Wrong password -> `NotAuthorizedException` (HTTP 403); a user still in `FORCE_CHANGE_PASSWORD` -> `ChallengeName=NEW_PASSWORD_REQUIRED` with a `Session` persisted for a later RespondToAuthChallenge; unknown client/pool -> `ResourceNotFoundException`.

6. **AdminGetUser** — in: `UserPoolId`, `Username` (required). out: `Username`, `UserAttributes`, `UserCreateDate`, `UserLastModifiedDate`, `Enabled`, `UserStatus`, `MFAOptions`, `PreferredMfaSetting`, `UserMFASettingList`. Must return exactly what AdminCreateUser stored plus any later mutations (e.g. `UserStatus=CONFIRMED` after step 4). Unknown user -> `UserNotFoundException` (404).

Teardown (`AdminDeleteUser`, `DeleteUserPool`, both taking only the ids and returning empty bodies) is a thin extension of the same state and should be implemented with this slice so the test can verify "gone": a deleted user makes AdminGetUser answer `UserNotFoundException`, and a deleted pool makes CreateUserPoolClient/AdminCreateUser answer `ResourceNotFoundException`.

NEVER STUBS: each of these must compute its response from persisted state. No canned `UserPool`/`UserPoolClient`/`User` bodies, no echoing the request back, no random id that is not stored and looked up again. A create followed by a describe/get must return what was created, and auth must actually check a stored credential.

## State Model

In `src/state.rs`, on the shared service state:

- `user_pools: HashMap<String /*UserPoolId*/, UserPool>`
  - `UserPool { id: String, name: String, arn: String, creation_date: SystemTime, last_modified_date: SystemTime, policies: Option<UserPoolPolicyType>, mfa_configuration: Option<String>, username_attributes: Vec<String>, auto_verified_attributes: Vec<String>, schema: Vec<SchemaAttributeType>, admin_create_user_config: Option<AdminCreateUserConfigType>, deletion_protection: Option<String>, user_pool_tier: Option<String>, tags: HashMap<String,String>, estimated_number_of_users: i32, clients: HashMap<String /*ClientId*/, UserPoolClient>, users: HashMap<String /*Username*/, User> }`
- `UserPoolClient { client_id: String, client_secret: Option<String>, user_pool_id: String, client_name: String, explicit_auth_flows: Vec<String>, read_attributes: Vec<String>, write_attributes: Vec<String>, refresh_token_validity: i32, access_token_validity: Option<i32>, id_token_validity: Option<i32>, creation_date: SystemTime, last_modified_date: SystemTime }`
- `User { username: String, sub: String /*uuid*/, attributes: BTreeMap<String,String> /*includes sub*/, password: Option<String>, password_permanent: bool, status: UserStatus /*UNCONFIRMED | FORCE_CHANGE_PASSWORD | CONFIRMED | RESET_REQUIRED | ARCHIVED*/, enabled: bool, mfa_options: Vec<MFAOptionType>, user_create_date: SystemTime, user_last_modified_date: SystemTime, groups: BTreeSet<String> }`
- `auth_sessions: HashMap<String /*Session token, len 20..4096*/, AuthSession{ user_pool_id, client_id, username, challenge_name, expires_at }>`
- `issued_tokens: HashMap<String /*AccessToken or RefreshToken*/, TokenRecord{ user_pool_id, client_id, username, kind: Access|Id|Refresh, issued_at, expires_at, revoked: bool }>` — so follow-up ops (GetUser, GlobalSignOut, RevokeToken) can resolve a token back to the stored user.
- A monotonic id/secret generator helper so pool ids match `^[\w-]+_[0-9a-zA-Z]+$`, client ids match `^[\w+]+$`, and client secrets are 24..64 chars.

Done = a realistic integration test runs CreateUserPool -> CreateUserPoolClient -> AdminCreateUser -> AdminSetUserPassword(Permanent=true) -> AdminInitiateAuth(ADMIN_USER_PASSWORD_AUTH) -> AdminGetUser -> AdminDeleteUser -> DeleteUserPool and asserts the response BODIES (ids round-trip, UserStatus transitions, tokens non-empty and resolvable, wrong password rejected with NotAuthorizedException, post-delete lookups 404) — not just 2xx.

Every other operation (SignUp/ConfirmSignUp, InitiateAuth, groups, identity providers, MFA, WebAuthn, domains, import jobs, tagging, list/describe of the remaining resources) stays scaffolded at 501 for now and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md (crate root) documents the wire protocol (`awsJson1_1`, `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`, sigv4 name `cognito-idp`) and the exact signatures cited above: CreateUserPool (`PoolName` required -> `UserPool`), CreateUserPoolClient (`UserPoolId`+`ClientName` required, `GenerateSecret` -> `UserPoolClient`), AdminCreateUser (`UserPoolId`+`Username` required -> `User`, errors include `UsernameExistsException` 400), AdminSetUserPassword (`UserPoolId`/`Username`/`Password` required, `Permanent`, errors include `UserNotFoundException` 404), AdminInitiateAuth (`AuthFlow` incl. ADMIN_USER_PASSWORD_AUTH, `AuthParameters` -> `AuthenticationResult`/`ChallengeName`/`Session`, errors include `NotAuthorizedException` 403), AdminGetUser (-> `Username`, `UserAttributes`, `Enabled`, `UserStatus`, errors include `UserNotFoundException` 404), AdminDeleteUser and DeleteUserPool (ids in, empty out). Pool-id pattern `^[\w-]+_[0-9a-zA-Z]+$` and client-id pattern `^[\w+]+$` are modeled constraints from the same file. All-stub status: every file under src/ops/ (e.g. src/ops/create_user_pool.rs lines 4 and 326) carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Scaffolded::not_implemented("CreateUserPool")`, i.e. HTTP 501; no tests assert any Cognito behaviour.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSCognitoIdentityProviderService.CreateUserPool and body {"PoolName":"demo-pool"} — expect 200 with UserPool.Id matching ^[\w-]+_[0-9a-zA-Z]+$; today: 501 NotImplemented.
2. CreateUserPoolClient with {"UserPoolId":"<id>","ClientName":"demo-client","GenerateSecret":false,"ExplicitAuthFlows":["ADMIN_USER_PASSWORD_AUTH"]} — expect 200 with UserPoolClient.ClientId and UserPoolId echoing the stored pool.
3. AdminCreateUser with {"UserPoolId":"<id>","Username":"alice","UserAttributes":[{"Name":"email","Value":"alice@example.com"}]} — expect 200 with User.Username="alice", a generated sub attribute, Enabled=true, UserStatus=FORCE_CHANGE_PASSWORD.
4. Repeat AdminCreateUser for "alice" — expect 400 UsernameExistsException.
5. AdminSetUserPassword with {"UserPoolId":"<id>","Username":"alice","Password":"Passw0rd!2024","Permanent":true} — expect 200 empty body.
6. AdminGetUser {"UserPoolId":"<id>","Username":"alice"} — expect UserStatus=CONFIRMED and the email attribute stored in step 3.
7. AdminInitiateAuth {"UserPoolId":"<id>","ClientId":"<clientId>","AuthFlow":"ADMIN_USER_PASSWORD_AUTH","AuthParameters":{"USERNAME":"alice","PASSWORD":"Passw0rd!2024"}} — expect 200 with AuthenticationResult.AccessToken/IdToken/RefreshToken non-empty and TokenType=Bearer.
8. Same AdminInitiateAuth with PASSWORD="wrong" — expect 403 NotAuthorizedException.
9. AdminGetUser for "bob" — expect 404 UserNotFoundException.
10. AdminDeleteUser {"UserPoolId":"<id>","Username":"alice"} then AdminGetUser "alice" — expect 404 UserNotFoundException.
11. DeleteUserPool {"UserPoolId":"<id>"} then AdminCreateUser into that pool — expect 404 ResourceNotFoundException.
