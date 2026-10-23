// SPDX-License-Identifier: CC0-1.0

//! The device-authorization and refresh-token grants, end to end over the wire.
//!
//! Every case drives the service through the router the production emulator serves,
//! so what is asserted is the HTTP behaviour a real SSO OIDC client (`aws sso login`)
//! sees — never a handler called directly.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ssooidc_awsssooidcservice as ssooidc;
use ssooidc::state::{
    ACCESS_TOKEN_EXPIRES_IN, APPROVAL_DELAY, CLIENT_SECRET_LIFETIME, DEVICE_CODE_EXPIRES_IN,
    DEVICE_CODE_INTERVAL, GRANT_TYPE_AUTHORIZATION_CODE, GRANT_TYPE_DEVICE_CODE,
    GRANT_TYPE_REFRESH_TOKEN, REFRESH_TOKEN_EXPIRES_IN, TOKEN_TYPE_BEARER,
};

/// The region most cases sign for; the region table below varies it.
pub(crate) const REGION: &str = "us-west-2";
/// The portal a device flow starts from.
pub(crate) const START_URL: &str = "https://my-sso-portal.awsapps.com/start";
/// Long enough for the approval the clock owes a pending authorization to come due.
pub(crate) const PAST_APPROVAL: Duration = APPROVAL_DELAY.saturating_add(Duration::from_millis(250));

/// One emulator: the shared engine plus the router that serves this service, the
/// same pair `core/router` builds in production.
pub(crate) struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(ssooidc::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// POST `path` in `region` and return the status and the parsed JSON body.
    pub(crate) async fn post(&self, path: &str, region: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=EMULATORAKTEST/20260101/{region}/sso-oauth/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://oidc.{region}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "{path} answered a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, body)
    }

    async fn register_client(&self, body: &Value) -> (u16, Value) {
        self.post("/client/register", REGION, body).await
    }

    pub(crate) async fn start_device_authorization(&self, body: &Value) -> (u16, Value) {
        self.post("/device_authorization", REGION, body).await
    }

    pub(crate) async fn create_token(&self, body: &Value) -> (u16, Value) {
        self.post("/token", REGION, body).await
    }

    /// Register a client with `extra` merged over the minimal registration, and
    /// return its `clientId`/`clientSecret`.
    pub(crate) async fn registered_client(&self, extra: Value) -> (String, String) {
        let mut body = json!({"clientName": "emulator-test", "clientType": "public"});
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            body[key] = value;
        }
        let (status, response) = self.register_client(&body).await;
        assert_eq!(status, 200, "RegisterClient({body}) answered {response}");
        (text(&response, "clientId"), text(&response, "clientSecret"))
    }

    /// This service's persisted state, for the cases that have to age a record the
    /// wall clock cannot be moved for.
    pub(crate) fn state(
        &self,
    ) -> engine::ServiceState<ssooidc::ComAmazonawsSsooidcAwsssooidcserviceState> {
        ssooidc::state::service_state(&*self.engine).expect("the service state is reachable")
    }
}

/// A string member of a response body, with the body in the failure message.
pub(crate) fn text(body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a string `{member}` in {body}"))
        .to_string()
}

/// The modeled error shape a failure response names (`__type`).
pub(crate) fn error_shape(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a `__type` in {body}"))
        .to_string()
}

fn now_epoch_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("epoch seconds fit in an i64")
}

/// A modeled `expiresIn` as whole seconds, for ageing a stored record by it.
pub(crate) fn seconds(expires_in: i32) -> u64 {
    u64::try_from(expires_in).expect("a modeled lifetime is positive")
}

/// Run the device flow for an already-registered client and return the token
/// response: register -> start -> wait for the approval the clock owes -> poll.
pub(crate) async fn device_flow_token(
    emulator: &Emulator,
    client_id: &str,
    client_secret: &str,
) -> Value {
    let (status, started) = emulator
        .start_device_authorization(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "startUrl": START_URL,
        }))
        .await;
    assert_eq!(status, 200, "StartDeviceAuthorization answered {started}");
    tokio::time::sleep(PAST_APPROVAL).await;
    let (status, token) = emulator
        .create_token(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "grantType": GRANT_TYPE_DEVICE_CODE,
            "deviceCode": text(&started, "deviceCode"),
        }))
        .await;
    assert_eq!(status, 200, "the approved poll answered {token}");
    token
}

/// Assert a failure: the HTTP status, the modeled shape, and the OAuth 2.0 `error`
/// code the model documents for that shape.
pub(crate) fn assert_failure(
    case: &str,
    status: u16,
    body: &Value,
    expected_status: u16,
    expected_shape: &str,
    expected_error: &str,
) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
    );
    assert!(
        error_shape(body).contains(expected_shape),
        "case {case:?}: expected the {expected_shape} shape, got {body}"
    );
    assert_eq!(
        body["error"].as_str(),
        Some(expected_error),
        "case {case:?}: expected error {expected_error:?}, got {body}"
    );
}

/// RULE: RegisterClient mints a fresh, persisted `clientId`/`clientSecret` pair for
/// every accepted registration, and only that pair authenticates the registration's
/// later calls. The table varies what the rule generalises over — the name, the case
/// of `clientType`, how many scopes and grant types are named, whether redirect URIs
/// and an issuer URL are named — and every row must hold.
#[tokio::test]
async fn register_client_mints_a_distinct_persisted_pair_for_every_registration() {
    let emulator = Emulator::new();
    let rows: Vec<(&str, Value)> = vec![
        (
            "minimal",
            json!({"clientName": "demo", "clientType": "public"}),
        ),
        (
            "an issuer URL with a trailing slash",
            json!({
                "clientName": "demo-2",
                "clientType": "public",
                "issuerUrl": "https://identitycenter.amazonaws.com/ssoins-0000000000000000/",
            }),
        ),
        (
            "one scope",
            json!({"clientName": "cli", "clientType": "public", "scopes": ["sso:account:access"]}),
        ),
        (
            "three scopes and two grants",
            json!({
                "clientName": "cli-wide",
                "clientType": "public",
                "scopes": ["sso:account:access", "codewhisperer:completions", "openid"],
                "grantTypes": [GRANT_TYPE_DEVICE_CODE, GRANT_TYPE_REFRESH_TOKEN],
            }),
        ),
        (
            "redirect URIs and an issuer URL",
            json!({
                "clientName": "pkce",
                "clientType": "public",
                "grantTypes": [GRANT_TYPE_AUTHORIZATION_CODE, GRANT_TYPE_DEVICE_CODE],
                "redirectUris": ["http://127.0.0.1:52000/oauth/callback", "https://example.test/cb"],
                "issuerUrl": "https://identitycenter.amazonaws.com/ssoins-1234567890abcdef",
            }),
        ),
        (
            "the same name again",
            json!({"clientName": "demo", "clientType": "public"}),
        ),
    ];

    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut seen_secrets: HashSet<String> = HashSet::new();

    for (case, body) in rows {
        let before = now_epoch_seconds();
        let (status, response) = emulator.register_client(&body).await;
        let after = now_epoch_seconds();
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {response}"
        );

        let client_id = text(&response, "clientId");
        let client_secret = text(&response, "clientSecret");
        assert!(
            !client_id.is_empty() && !client_secret.is_empty(),
            "case {case:?}: expected a minted pair, got {response}"
        );
        assert!(
            seen_ids.insert(client_id.clone()),
            "case {case:?}: clientId {client_id} was already minted for another registration"
        );
        assert!(
            seen_secrets.insert(client_secret.clone()),
            "case {case:?}: a clientSecret was minted twice"
        );

        let issued_at = response["clientIdIssuedAt"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected clientIdIssuedAt in {response}"));
        let expires_at = response["clientSecretExpiresAt"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected clientSecretExpiresAt in {response}"));
        assert!(
            (before..=after).contains(&issued_at),
            "case {case:?}: expected clientIdIssuedAt in {before}..={after}, got {issued_at}"
        );
        let lifetime = i64::try_from(CLIENT_SECRET_LIFETIME.as_secs()).expect("lifetime fits");
        assert_eq!(
            expires_at - issued_at,
            lifetime,
            "case {case:?}: expected the secret to expire {lifetime} seconds after it was issued, got {expires_at} - {issued_at}"
        );

        // The endpoints are computed: from the client's own issuer when it named
        // one, else from this service's endpoint in the request's region.
        let base = body["issuerUrl"].as_str().map_or_else(
            || format!("https://oidc.{REGION}.amazonaws.com"),
            |issuer| issuer.trim_end_matches('/').to_string(),
        );
        assert_eq!(
            text(&response, "tokenEndpoint"),
            format!("{base}/token"),
            "case {case:?}: tokenEndpoint was not computed from {base}: {response}"
        );
        assert_eq!(
            text(&response, "authorizationEndpoint"),
            format!("{base}/authorize"),
            "case {case:?}: authorizationEndpoint was not computed from {base}: {response}"
        );

        // The pair is accepted...
        let (accepted, accepted_body) = emulator
            .start_device_authorization(&json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "startUrl": START_URL,
            }))
            .await;
        assert_eq!(
            accepted, 200,
            "case {case:?}: the minted pair was refused: {accepted_body}"
        );
        // ... and nothing else is: neither a tampered secret nor an id that was
        // never registered.
        let (refused, refused_body) = emulator
            .start_device_authorization(&json!({
                "clientId": client_id,
                "clientSecret": format!("{client_secret}-tampered"),
                "startUrl": START_URL,
            }))
            .await;
        assert_failure(
            case,
            refused,
            &refused_body,
            401,
            "InvalidClientException",
            "invalid_client",
        );
        let (unknown, unknown_body) = emulator
            .start_device_authorization(&json!({
                "clientId": "never-registered-client",
                "clientSecret": client_secret,
                "startUrl": START_URL,
            }))
            .await;
        assert_failure(
            case,
            unknown,
            &unknown_body,
            401,
            "InvalidClientException",
            "invalid_client",
        );
    }
}

/// RULE: RegisterClient refuses a registration whose metadata is outside what the
/// model documents — `clientType` other than `public`, a grant type the service does
/// not serve, a redirect or issuer URI that is not absolute, a scope that is not a
/// scope token — each with the modeled error for that member.
#[tokio::test]
async fn register_client_refuses_metadata_outside_the_documented_rules() {
    let emulator = Emulator::new();
    let rows: Vec<(&str, Value, u16, &str, &str)> = vec![
        (
            "a confidential client",
            json!({"clientName": "demo", "clientType": "confidential"}),
            400,
            "InvalidClientMetadataException",
            "invalid_client_metadata",
        ),
        (
            "a native client",
            json!({"clientName": "demo", "clientType": "native"}),
            400,
            "InvalidClientMetadataException",
            "invalid_client_metadata",
        ),
        (
            "an empty clientType",
            json!({"clientName": "demo", "clientType": ""}),
            400,
            "InvalidClientMetadataException",
            "invalid_client_metadata",
        ),
        (
            "clientType in another case",
            json!({"clientName": "demo", "clientType": "PUBLIC"}),
            400,
            "InvalidClientMetadataException",
            "invalid_client_metadata",
        ),
        (
            "an empty clientName",
            json!({"clientName": "   ", "clientType": "public"}),
            400,
            "InvalidClientMetadataException",
            "invalid_client_metadata",
        ),
        (
            "the client-credentials grant",
            json!({"clientName": "demo", "clientType": "public", "grantTypes": ["client_credentials"]}),
            400,
            "UnsupportedGrantTypeException",
            "unsupported_grant_type",
        ),
        (
            "a supported grant beside an unsupported one",
            json!({
                "clientName": "demo",
                "clientType": "public",
                "grantTypes": [GRANT_TYPE_DEVICE_CODE, "implicit"],
            }),
            400,
            "UnsupportedGrantTypeException",
            "unsupported_grant_type",
        ),
        (
            "a redirect URI with no scheme",
            json!({"clientName": "demo", "clientType": "public", "redirectUris": ["/oauth/callback"]}),
            400,
            "InvalidRedirectUriException",
            "invalid_redirect_uri",
        ),
        (
            "a second redirect URI that is not a URI",
            json!({
                "clientName": "demo",
                "clientType": "public",
                "redirectUris": ["http://127.0.0.1:1234/cb", "not a uri at all"],
            }),
            400,
            "InvalidRedirectUriException",
            "invalid_redirect_uri",
        ),
        (
            "a scope carrying whitespace",
            json!({"clientName": "demo", "clientType": "public", "scopes": ["sso:account:access openid"]}),
            400,
            "InvalidScopeException",
            "invalid_scope",
        ),
        (
            "an empty scope",
            json!({"clientName": "demo", "clientType": "public", "scopes": [""]}),
            400,
            "InvalidScopeException",
            "invalid_scope",
        ),
        (
            "an issuer URL that is not absolute",
            json!({"clientName": "demo", "clientType": "public", "issuerUrl": "identitycenter"}),
            400,
            "InvalidRequestException",
            "invalid_request",
        ),
    ];

    for (case, body, status, shape, error) in rows {
        let (actual_status, response) = emulator.register_client(&body).await;
        assert_failure(case, actual_status, &response, status, shape, error);
    }
}

/// RULE: StartDeviceAuthorization persists a device authorization and returns the
/// verification codes computed from it: the verification URI of the region the
/// request is addressed to, that URI plus the user code as
/// `verificationUriComplete`, a fresh opaque `deviceCode`, and the service's
/// `expiresIn`/`interval`.
#[tokio::test]
async fn start_device_authorization_computes_the_codes_from_the_region_and_persists_them() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;

    let mut seen_device_codes: HashSet<String> = HashSet::new();
    // Each row is a region the request is signed for; the verification endpoint is
    // regional, so the answer must follow the region rather than a fixed value.
    for region in ["us-east-1", "us-west-2", "eu-west-1", "ap-southeast-2"] {
        let (status, response) = emulator
            .post(
                "/device_authorization",
                region,
                &json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    "startUrl": START_URL,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with {response}"
        );

        let device_code = text(&response, "deviceCode");
        let user_code = text(&response, "userCode");
        assert!(
            seen_device_codes.insert(device_code.clone()),
            "case {region:?}: the same deviceCode was returned twice"
        );
        assert!(
            device_code.len() >= 32,
            "case {region:?}: expected an opaque deviceCode, got {device_code:?}"
        );

        let groups: Vec<&str> = user_code.split('-').collect();
        assert!(
            groups.len() == 2
                && groups.iter().all(|group| group.len() == 4
                    && group
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())),
            "case {region:?}: expected a user code of two four-character groups, got {user_code:?}"
        );

        let expected_uri = format!("https://device.sso.{region}.amazonaws.com/");
        assert_eq!(
            text(&response, "verificationUri"),
            expected_uri,
            "case {region:?}: expected {expected_uri}, got {response}"
        );
        assert_eq!(
            text(&response, "verificationUriComplete"),
            format!("{expected_uri}?user_code={user_code}"),
            "case {region:?}: verificationUriComplete does not carry the user code: {response}"
        );
        assert_eq!(
            response["expiresIn"].as_i64(),
            Some(i64::from(DEVICE_CODE_EXPIRES_IN)),
            "case {region:?}: unexpected expiresIn in {response}"
        );
        assert_eq!(
            response["interval"].as_i64(),
            Some(i64::from(DEVICE_CODE_INTERVAL)),
            "case {region:?}: unexpected interval in {response}"
        );

        // The record is persisted, indexed by its user code, and bound to the
        // client and start URL that asked for it.
        let state = emulator.state();
        let store = state.lock();
        let stored = store
            .device_auths
            .get(&device_code)
            .unwrap_or_else(|| panic!("case {region:?}: the deviceCode was not persisted"));
        assert_eq!(
            (stored.client_id.as_str(), stored.start_url.as_str()),
            (client_id.as_str(), START_URL),
            "case {region:?}: the stored record is bound to the wrong client: {stored:?}"
        );
        assert_eq!(
            store.user_codes.get(&user_code),
            Some(&device_code),
            "case {region:?}: the user code was not indexed to the device code"
        );
    }

    // A startUrl that is not a URI is refused rather than stored.
    let (status, response) = emulator
        .start_device_authorization(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "startUrl": "my-sso-portal",
        }))
        .await;
    assert_failure(
        "a startUrl that is not absolute",
        status,
        &response,
        400,
        "InvalidRequestException",
        "invalid_request",
    );
}

/// RULE: the device flow runs off persisted state — the authorization is pending
/// until the approval the clock owes comes due, the device code is redeemed exactly
/// once, and the refresh token rotates into a new access token.
#[tokio::test]
async fn the_device_flow_runs_end_to_end_from_persisted_state() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator
        .registered_client(json!({"scopes": ["sso:account:access"]}))
        .await;

    let (status, started) = emulator
        .start_device_authorization(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "startUrl": START_URL,
        }))
        .await;
    assert_eq!(status, 200, "StartDeviceAuthorization answered {started}");
    let device_code = text(&started, "deviceCode");

    let poll = json!({
        "clientId": client_id,
        "clientSecret": client_secret,
        "grantType": GRANT_TYPE_DEVICE_CODE,
        "deviceCode": device_code,
    });

    // The first poll comes before the approval is due, so the caller sees the
    // transient state rather than a token.
    let (status, pending) = emulator.create_token(&poll).await;
    assert_failure(
        "a poll before the approval is due",
        status,
        &pending,
        400,
        "AuthorizationPendingException",
        "authorization_pending",
    );
    assert_eq!(
        emulator
            .state()
            .lock()
            .device_auths
            .get(&device_code)
            .map(|record| record.status),
        Some(ssooidc::state::DeviceAuthStatus::Pending),
        "the record must still be pending after a pending poll"
    );

    // Once it is due, the transition is recorded and the token is minted.
    tokio::time::sleep(PAST_APPROVAL).await;
    let (status, token) = emulator.create_token(&poll).await;
    assert_eq!(status, 200, "the approved poll answered {token}");
    let access_token = text(&token, "accessToken");
    let refresh_token = text(&token, "refreshToken");
    assert_eq!(
        token["tokenType"].as_str(),
        Some(TOKEN_TYPE_BEARER),
        "expected a Bearer token, got {token}"
    );
    assert_eq!(
        token["expiresIn"].as_i64(),
        Some(i64::from(ACCESS_TOKEN_EXPIRES_IN)),
        "unexpected expiresIn in {token}"
    );

    {
        let state = emulator.state();
        let store = state.lock();
        let stored = store
            .access_tokens
            .get(&access_token)
            .expect("the access token must be persisted");
        assert_eq!(
            (stored.client_id.as_str(), stored.scopes.as_slice()),
            (client_id.as_str(), ["sso:account:access".to_string()].as_slice()),
            "the stored token carries the wrong client or scopes: {stored:?}"
        );
        assert_eq!(
            store
                .refresh_tokens
                .get(&refresh_token)
                .map(|grant| grant.bound_access_token.clone()),
            Some(access_token.clone()),
            "the refresh token must be bound to the access token it was issued with"
        );
        assert_eq!(
            store.device_auths.get(&device_code).map(|r| r.status),
            Some(ssooidc::state::DeviceAuthStatus::Consumed),
            "the redeemed device code must be recorded as consumed"
        );
    }

    // The device code is spent: polling again is an invalid grant, not a second token.
    let (status, replayed) = emulator.create_token(&poll).await;
    assert_failure(
        "a second exchange of the same device code",
        status,
        &replayed,
        400,
        "InvalidGrantException",
        "invalid_grant",
    );

    // The refresh token rotates: a NEW access token, a NEW refresh token, and the
    // spent one is refused.
    let refresh = json!({
        "clientId": client_id,
        "clientSecret": client_secret,
        "grantType": GRANT_TYPE_REFRESH_TOKEN,
        "refreshToken": refresh_token,
    });
    let (status, refreshed) = emulator.create_token(&refresh).await;
    assert_eq!(status, 200, "the refresh answered {refreshed}");
    let rotated_access = text(&refreshed, "accessToken");
    let rotated_refresh = text(&refreshed, "refreshToken");
    assert_ne!(
        rotated_access, access_token,
        "the refresh must mint a new access token, got {refreshed}"
    );
    assert_ne!(
        rotated_refresh, refresh_token,
        "the refresh must rotate the refresh token, got {refreshed}"
    );
    assert_eq!(
        emulator
            .state()
            .lock()
            .access_tokens
            .get(&rotated_access)
            .map(|token| token.scopes.clone()),
        Some(vec!["sso:account:access".to_string()]),
        "the rotated token must keep the scopes of the token it replaced"
    );

    let (status, reused) = emulator.create_token(&refresh).await;
    assert_failure(
        "a refresh token that was already rotated",
        status,
        &reused,
        400,
        "InvalidGrantException",
        "invalid_grant",
    );

    // A refresh token belongs to the client it was issued to: another registered
    // client cannot spend it.
    let (other_id, other_secret) = emulator.registered_client(json!({})).await;
    let (status, stolen) = emulator
        .create_token(&json!({
            "clientId": other_id,
            "clientSecret": other_secret,
            "grantType": GRANT_TYPE_REFRESH_TOKEN,
            "refreshToken": rotated_refresh,
        }))
        .await;
    assert_failure(
        "another client's refresh token",
        status,
        &stolen,
        400,
        "InvalidGrantException",
        "invalid_grant",
    );
}

/// RULE: CreateToken answers every grant from the state the request resolves to —
/// an unknown client, an unsupported grant, a missing member for the grant, and a
/// grant value that resolves to nothing each get their own modeled error.
#[tokio::test]
async fn create_token_answers_each_grant_from_the_state_it_resolves() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;

    let rows: Vec<(&str, Value, u16, &str, &str)> = vec![
        (
            "an unregistered client",
            json!({
                "clientId": "never-registered",
                "clientSecret": "whatever",
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "anything",
            }),
            401,
            "InvalidClientException",
            "invalid_client",
        ),
        (
            "a wrong client secret",
            json!({
                "clientId": client_id,
                "clientSecret": "not-the-secret",
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "anything",
            }),
            401,
            "InvalidClientException",
            "invalid_client",
        ),
        (
            "the client-credentials grant",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": "client_credentials",
            }),
            400,
            "UnsupportedGrantTypeException",
            "unsupported_grant_type",
        ),
        (
            "the password grant",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": "password",
            }),
            400,
            "UnsupportedGrantTypeException",
            "unsupported_grant_type",
        ),
        (
            "an empty grant type",
            json!({"clientId": client_id, "clientSecret": client_secret, "grantType": ""}),
            400,
            "UnsupportedGrantTypeException",
            "unsupported_grant_type",
        ),
        (
            "the device-code grant with no device code",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
        ),
        (
            "the refresh-token grant with no refresh token",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
        ),
        (
            "the authorization-code grant with no code",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_AUTHORIZATION_CODE,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
        ),
        (
            "a device code this service never issued",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "e3b0c44298fc1c149afbf4c8996fb924",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
        ),
        (
            "a refresh token this service never issued",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": "da39a3ee5e6b4b0d3255bfef95601890",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
        ),
        (
            "an authorization code this service never issued",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_AUTHORIZATION_CODE,
                "code": "5baa61e4c9b93f3f0682250b6cf8331b",
                "redirectUri": "http://127.0.0.1:52000/oauth/callback",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
        ),
        (
            "a scope the client did not register",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "anything",
                "scope": ["sso:account:access"],
            }),
            400,
            "InvalidScopeException",
            "invalid_scope",
        ),
    ];

    // The scope row needs a client that registered a scope to be outside of.
    let (scoped_id, scoped_secret) = emulator
        .registered_client(json!({"scopes": ["openid"]}))
        .await;
    for (case, body, status, shape, error) in rows {
        let body = if case == "a scope the client did not register" {
            json!({
                "clientId": scoped_id,
                "clientSecret": scoped_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "anything",
                "scope": ["sso:account:access"],
            })
        } else {
            body
        };
        let (actual_status, response) = emulator.create_token(&body).await;
        assert_failure(case, actual_status, &response, status, shape, error);
    }
}

/// RULE: a device authorization past its `expiresIn` is an expired token, whatever
/// its status — the lifetime is read off the stored record, not off the request.
#[tokio::test]
async fn a_device_authorization_past_its_lifetime_is_an_expired_token() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;

    // Each row ages the stored record by a different amount: one second past the
    // lifetime, and far past it.
    for overshoot in [1_u64, 60, 86_400] {
        let (status, started) = emulator
            .start_device_authorization(&json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "startUrl": START_URL,
            }))
            .await;
        assert_eq!(status, 200, "StartDeviceAuthorization answered {started}");
        let device_code = text(&started, "deviceCode");

        // The wall clock cannot be moved, so the record is aged instead: the
        // handler must read the lifetime off what is stored.
        let aged = Duration::from_secs(u64::from(u32::try_from(DEVICE_CODE_EXPIRES_IN).expect("positive")) + overshoot);
        {
            let state = emulator.state();
            let mut store = state.lock();
            let record = store
                .device_auths
                .get_mut(&device_code)
                .expect("the record is persisted");
            record.created_at -= aged;
            record.approval_due_at -= aged;
        }

        let (status, response) = emulator
            .create_token(&json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": device_code,
            }))
            .await;
        assert_failure(
            &format!("{overshoot} seconds past the lifetime"),
            status,
            &response,
            400,
            "ExpiredTokenException",
            "expired_token",
        );
    }
}

/// RULE: `grantTypes` on the registration restricts the flows available to the
/// client — a grant it did not register is refused, and it is handed a refresh token
/// only when it registered the refresh-token grant.
#[tokio::test]
async fn a_registration_is_restricted_to_the_grants_it_named() {
    let emulator = Emulator::new();

    // Each row is a registration naming a different set of grants, with what the
    // device flow must then answer.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("no grants named (unrestricted)", json!({}), true),
        (
            "the device-code grant only",
            json!({"grantTypes": [GRANT_TYPE_DEVICE_CODE]}),
            false,
        ),
        (
            "the device-code and refresh-token grants",
            json!({"grantTypes": [GRANT_TYPE_DEVICE_CODE, GRANT_TYPE_REFRESH_TOKEN]}),
            true,
        ),
    ];

    for (case, registration, expects_refresh_token) in rows {
        let (client_id, client_secret) = emulator.registered_client(registration).await;
        let (status, started) = emulator
            .start_device_authorization(&json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "startUrl": START_URL,
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartDeviceAuthorization answered {started}"
        );
        tokio::time::sleep(PAST_APPROVAL).await;

        let (status, token) = emulator
            .create_token(&json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": text(&started, "deviceCode"),
            }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a token, got {status} with {token}"
        );
        assert_eq!(
            token["refreshToken"].is_string(),
            expects_refresh_token,
            "case {case:?}: expected refreshToken present = {expects_refresh_token}, got {token}"
        );

        if !expects_refresh_token {
            // The grant it did not register is refused as unauthorized, not as a
            // bad request.
            let (status, refused) = emulator
                .create_token(&json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    "grantType": GRANT_TYPE_REFRESH_TOKEN,
                    "refreshToken": "whatever-was-never-issued",
                }))
                .await;
            assert_failure(
                case,
                status,
                &refused,
                400,
                "UnauthorizedClientException",
                "unauthorized_client",
            );
        }
    }
}

/// RULE: a record that can no longer be used is reclaimed rather than held for the
/// life of the process — a redeemed device authorization (and its user-code index
/// entry) is dropped by the next StartDeviceAuthorization, and a refresh drops the
/// access token it supersedes — so a caller cannot grow the store without bound.
#[tokio::test]
async fn spent_records_are_reclaimed_rather_than_held_for_ever() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;

    let (status, started) = emulator
        .start_device_authorization(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "startUrl": START_URL,
        }))
        .await;
    assert_eq!(status, 200, "StartDeviceAuthorization answered {started}");
    let spent_device_code = text(&started, "deviceCode");
    let spent_user_code = text(&started, "userCode");

    tokio::time::sleep(PAST_APPROVAL).await;
    let (status, token) = emulator
        .create_token(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "grantType": GRANT_TYPE_DEVICE_CODE,
            "deviceCode": spent_device_code,
        }))
        .await;
    assert_eq!(status, 200, "the approved poll answered {token}");
    let mut refresh_token = text(&token, "refreshToken");
    let superseded_access_token = text(&token, "accessToken");

    // The next device authorization reclaims the redeemed one and its index entry.
    let (status, second) = emulator
        .start_device_authorization(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "startUrl": START_URL,
        }))
        .await;
    assert_eq!(status, 200, "StartDeviceAuthorization answered {second}");
    {
        let state = emulator.state();
        let store = state.lock();
        assert!(
            !store.device_auths.contains_key(&spent_device_code),
            "case \"redeemed device authorization\": the record was not reclaimed"
        );
        assert!(
            !store.user_codes.contains_key(&spent_user_code),
            "case \"redeemed device authorization\": the user-code index entry was not reclaimed"
        );
        assert_eq!(
            store.device_auths.len(),
            1,
            "case \"redeemed device authorization\": expected only the live record, got {}",
            store.device_auths.len()
        );
    }

    // Each refresh supersedes exactly one token, so a refresh loop holds one.
    for round in 1..=3 {
        let (status, refreshed) = emulator
            .create_token(&json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": refresh_token,
            }))
            .await;
        assert_eq!(
            status, 200,
            "case \"refresh round {round}\": expected a token, got {status} with {refreshed}"
        );
        refresh_token = text(&refreshed, "refreshToken");

        let state = emulator.state();
        let store = state.lock();
        assert!(
            !store.access_tokens.contains_key(&superseded_access_token),
            "case \"refresh round {round}\": the superseded access token is still resident"
        );
        assert_eq!(
            (store.access_tokens.len(), store.refresh_tokens.len()),
            (1, 1),
            "case \"refresh round {round}\": expected one live token and one live refresh token, got {} and {}",
            store.access_tokens.len(),
            store.refresh_tokens.len()
        );
    }
}

/// RULE: a refresh token has its own lifetime, independent of the access token it
/// was issued with — refreshing after the access token has expired is the ordinary
/// case and mints a new pair, while a refresh token past its OWN lifetime is an
/// expired token.
#[tokio::test]
async fn a_refresh_token_outlives_the_access_token_it_was_issued_with() {
    // Each row ages a different stored record, so the two lifetimes cannot be
    // conflated: ageing the access token must still allow a refresh, ageing the
    // refresh token must not.
    for (case, age_access_token, age_refresh_token, expected) in [
        ("the access token expired", true, false, 200_u16),
        ("neither expired", false, false, 200),
        ("the refresh token expired", false, true, 400),
    ] {
        let emulator = Emulator::new();
        let (client_id, client_secret) = emulator.registered_client(json!({})).await;
        let token = device_flow_token(&emulator, &client_id, &client_secret).await;
        let access_token = text(&token, "accessToken");
        let refresh_token = text(&token, "refreshToken");

        {
            let state = emulator.state();
            let mut store = state.lock();
            if age_access_token {
                let aged = Duration::from_secs(seconds(ACCESS_TOKEN_EXPIRES_IN) + 1);
                let record = store
                    .access_tokens
                    .get_mut(&access_token)
                    .expect("the access token is persisted");
                record.issued_at -= aged;
            }
            if age_refresh_token {
                let aged = Duration::from_secs(seconds(REFRESH_TOKEN_EXPIRES_IN) + 1);
                let grant = store
                    .refresh_tokens
                    .get_mut(&refresh_token)
                    .expect("the refresh token is persisted");
                grant.issued_at -= aged;
            }
        }

        let (status, response) = emulator
            .create_token(&json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": refresh_token,
            }))
            .await;
        if expected == 200 {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a new token pair, got {status} with {response}"
            );
            assert_ne!(
                text(&response, "accessToken"),
                access_token,
                "case {case:?}: the refresh must mint a new access token"
            );
        } else {
            assert_failure(
                case,
                status,
                &response,
                400,
                "ExpiredTokenException",
                "expired_token",
            );
        }
    }
}

/// RULE: a refresh is granted no broader a scope than the grant it spends
/// (RFC 6749 §6) — the grant's own scopes carry over, a narrower request is
/// honoured, and a scope the grant does not carry is refused even when the
/// registration itself allows it.
#[tokio::test]
async fn a_refresh_cannot_widen_the_scope_it_was_granted() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator
        .registered_client(json!({"scopes": ["sso:account:access", "openid"]}))
        .await;

    // The device-code exchange asks for only one of the two registered scopes,
    // so the grant it leaves behind carries only that one.
    let (status, started) = emulator
        .start_device_authorization(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "startUrl": START_URL,
        }))
        .await;
    assert_eq!(status, 200, "StartDeviceAuthorization answered {started}");
    tokio::time::sleep(PAST_APPROVAL).await;
    let (status, token) = emulator
        .create_token(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "grantType": GRANT_TYPE_DEVICE_CODE,
            "deviceCode": text(&started, "deviceCode"),
            "scope": ["openid"],
        }))
        .await;
    assert_eq!(status, 200, "the narrowly-scoped exchange answered {token}");
    let narrow_refresh_token = text(&token, "refreshToken");

    // Asking for the scope the grant carries is honoured.
    let (status, same_scope) = emulator
        .create_token(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "grantType": GRANT_TYPE_REFRESH_TOKEN,
            "refreshToken": narrow_refresh_token,
            "scope": ["openid"],
        }))
        .await;
    assert_eq!(
        status, 200,
        "case \"the scope the grant carries\": expected a token, got {status} with {same_scope}"
    );
    let carried_refresh_token = text(&same_scope, "refreshToken");

    // Asking for a scope the registration allows but the GRANT does not carry is
    // refused rather than silently widened.
    let (status, widened) = emulator
        .create_token(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "grantType": GRANT_TYPE_REFRESH_TOKEN,
            "refreshToken": carried_refresh_token,
            "scope": ["sso:account:access"],
        }))
        .await;
    assert_failure(
        "a scope the grant does not carry",
        status,
        &widened,
        400,
        "InvalidScopeException",
        "invalid_scope",
    );

    // And a refresh naming no scope carries the grant's own scope over.
    let (status, carried) = emulator
        .create_token(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "grantType": GRANT_TYPE_REFRESH_TOKEN,
            "refreshToken": carried_refresh_token,
        }))
        .await;
    assert_eq!(
        status, 200,
        "case \"no scope requested\": expected a token, got {status} with {carried}"
    );
    assert_eq!(
        emulator
            .state()
            .lock()
            .access_tokens
            .get(&text(&carried, "accessToken"))
            .map(|token| token.scopes.clone()),
        Some(vec!["openid".to_string()]),
        "case \"no scope requested\": the refreshed token must keep the grant's scope"
    );
}

// `CreateTokenWithIAM` has its own suite (`tests/all/create_token_with_iam.rs`): it is
// served on the same `/token` path with `?aws_iam=t`, and no longer answers the
// scaffold's 501, so the case that pinned that 501 moved there as the real contract.
