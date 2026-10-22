Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the AWS SSO OIDC emulator

_Filed: 2026-10-21T21:53:10Z_
_Operation: service-com-amazonaws-ssooidc-awsssooidcservice | Class: stub_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 4 operations in `src/ops/` are unmodified scaffolds answering HTTP 501, so no AWS SSO OIDC client (notably the `aws sso login` / device-authorization flow used by the AWS CLI and SDK token providers) can complete a single call. Implement the device-authorization happy path statefully, in this order:

1. **RegisterClient** (`POST /client/register`) — input `clientName` (required), `clientType` (required), optional `scopes`, `redirectUris`, `grantTypes`, `issuerUrl`. Must MINT and PERSIST a new client: generate a `clientId` and `clientSecret`, store them with the supplied metadata, and return `clientId`, `clientSecret`, `clientIdIssuedAt` (now, epoch seconds), `clientSecretExpiresAt` (now + lifetime, e.g. 90 days). The returned `clientId`/`clientSecret` pair MUST be accepted by the two following operations and an unknown pair MUST be rejected — so the credentials cannot be echoed or faked.

2. **StartDeviceAuthorization** (`POST /device_authorization`) — input `clientId`, `clientSecret`, `startUrl` (all required). Look the client up by `clientId` and verify `clientSecret`; on mismatch/unknown client return `InvalidClientException` (HTTP 401). On success create and PERSIST a device-authorization record and return `deviceCode` (opaque, unguessable), `userCode` (short human code), `verificationUri` (e.g. `https://device.sso.<region>.amazonaws.com/`), `verificationUriComplete` (`verificationUri?user_code=<userCode>`), `expiresIn` (e.g. 600) and `interval` (e.g. 5). The `deviceCode` MUST be resolvable by CreateToken.

3. **CreateToken** (`POST /token`) with `grantType=urn:ietf:params:oauth:grant-type:device_code` — input `clientId`, `clientSecret`, `grantType` (required) plus `deviceCode`. Must resolve the device-authorization record by (`clientId`, `deviceCode`) and compute the answer from its persisted status:
   - unknown/mismatched client → `InvalidClientException` (401)
   - unknown `deviceCode` → `InvalidGrantException` (400)
   - record past `expiresIn` → `ExpiredTokenException` (400)
   - record not yet approved → `AuthorizationPendingException` (400)
   - approved → mint and PERSIST an access token, returning `accessToken`, `tokenType: "Bearer"`, `expiresIn` (e.g. 3600), and `refreshToken` when the client registered that grant.
   An unsupported `grantType` value → `UnsupportedGrantTypeException` (400); a missing `deviceCode` for the device-code grant → `InvalidRequestException` (400).
   Because an emulator has no browser, the device authorization must become approvable deterministically — e.g. auto-approve on first poll after the first `AuthorizationPendingException`, or auto-approve immediately — but the state transition must be recorded on the stored record, NOT faked per-request.

4. **CreateToken** with `grantType=refresh_token` — resolve the persisted `refreshToken`, verify it belongs to the authenticating client, mint a NEW `accessToken` (different value from the previous one) and return it; an unknown or already-rotated refresh token → `InvalidGrantException` (400).

Every response field above must be computed from persisted state. Hardcoded tokens, echoed inputs, or random IDs that are never stored are failures: a `deviceCode` returned by StartDeviceAuthorization must be recognised by CreateToken, and a `clientId` that was never registered must be rejected.

`CreateTokenWithIAM` (`POST /token?aws_iam=t`) is explicitly OUT OF SCOPE for this first slice and stays 501; it will be filed as its own gap issue.

## State Model

In `ComAmazonawsSsooidcAwsssooidcserviceState` (src/state.rs), reached via `crate::state::service_state(&*self.engine)`:

- `clients: HashMap<String /*clientId*/, OidcClient>` where
  `OidcClient { client_id: String, client_secret: String, client_name: String, client_type: String, scopes: Vec<String>, redirect_uris: Vec<String>, grant_types: Vec<String>, issuer_url: Option<String>, client_id_issued_at: i64, client_secret_expires_at: i64 }`
- `device_auths: HashMap<String /*deviceCode*/, DeviceAuthorization>` where
  `DeviceAuthorization { device_code: String, user_code: String, client_id: String, start_url: String, verification_uri: String, verification_uri_complete: String, created_at: SystemTime, expires_in: i32, interval: i32, status: DeviceAuthStatus, poll_count: u32 }` and
  `enum DeviceAuthStatus { Pending, Approved, Denied, Consumed }`
- `user_codes: HashMap<String /*userCode*/, String /*deviceCode*/>` (index for approval lookup)
- `access_tokens: HashMap<String /*accessToken*/, IssuedToken>` where
  `IssuedToken { access_token: String, refresh_token: Option<String>, id_token: Option<String>, client_id: String, scopes: Vec<String>, token_type: String, issued_at: SystemTime, expires_in: i32 }`
- `refresh_tokens: HashMap<String /*refreshToken*/, String /*accessToken currently bound*/>`

Done = a realistic client script (register → start device auth → poll token → refresh) succeeds end to end with correct, stateful bodies, and the negative cases above return the modeled error shapes/status codes rather than 501 or a hollow 200.

## Evidence

`services/service-com-amazonaws-ssooidc-awsssooidcservice/OPERATIONS.md` lists exactly 4 operations (RegisterClient, StartDeviceAuthorization, CreateToken, CreateTokenWithIAM) over `restJson1` with the routes, required input members, output members and modeled errors quoted above (e.g. RegisterClient output `clientId`/`clientSecret`/`clientIdIssuedAt`/`clientSecretExpiresAt`; CreateToken errors include `AuthorizationPendingException`, `ExpiredTokenException`, `InvalidGrantException`, `InvalidClientException` (401), `UnsupportedGrantTypeException`). `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-ssooidc-awsssooidcservice/src/ops` matches all four handler files (create_token.rs, create_token_with_iam.rs, register_client.rs, start_device_authorization.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501. No state structures exist yet in src/state.rs and there are no behavioral tests beyond the generated `tests/state_roundtrip.rs`. The error-to-flow mapping is the well-established OAuth 2.0 device authorization grant (RFC 8628) as implemented by AWS SSO OIDC for `aws sso login`.

## Steps to Reproduce

1. Start the emulator and point an SSO OIDC client at it (endpoint prefix `oidc`, signing name `sso-oauth`).
2. POST /client/register with {"clientName":"demo","clientType":"public","scopes":["sso:account:access"]} — expect 200 with a fresh clientId/clientSecret and clientIdIssuedAt/clientSecretExpiresAt; today returns 501.
3. POST /device_authorization with the clientId/clientSecret from step 2 and startUrl="https://my-sso-portal.awsapps.com/start" — expect 200 with deviceCode, userCode, verificationUri, verificationUriComplete, expiresIn and interval.
4. Repeat step 3 with a bogus clientSecret — expect InvalidClientException (HTTP 401), not 200 and not 501.
5. POST /token with grantType="urn:ietf:params:oauth:grant-type:device_code", the clientId/clientSecret and the deviceCode from step 3 — expect AuthorizationPendingException (400) while the stored record is Pending, then 200 with accessToken, tokenType="Bearer", expiresIn and refreshToken once approved.
6. POST /token with grantType="refresh_token" and the refreshToken from step 5 — expect 200 with a NEW accessToken value different from the previous one.
7. POST /token with grantType="refresh_token" and a made-up refresh token — expect InvalidGrantException (400).
8. POST /token with grantType="client_credentials" — expect UnsupportedGrantTypeException (400).
