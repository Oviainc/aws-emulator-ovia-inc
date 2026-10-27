// SPDX-License-Identifier: CC0-1.0

//! `CompleteWebAuthnRegistration` over the wire, driven exactly as an SDK drives
//! it (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.CompleteWebAuthnRegistration`).
//!
//! The rule under test: a registration response is resolved against the
//! registration `StartWebAuthnRegistration` stored, by the challenge the
//! response's collected client data carries, and a response that verifies
//! REGISTERS one passkey credential to the stored user — its credential id, the
//! attachment and transports the authenticator reported, and the relying party
//! the registration was started under — after which the credential is read back
//! through the operations that report it (`StartWebAuthnRegistration`'s
//! `excludeCredentials` and `GetUserAuthFactors`' `WEB_AUTHN` factor).
//! Completing a registration CONSUMES its challenge, so one registration
//! registers one passkey; a refused response registers none and leaves the
//! registration answerable. A challenge this emulator did not hand out is refused
//! with 400 `WebAuthnChallengeNotFoundException`, a token of another app client
//! with 400 `WebAuthnClientMismatchException`, a pool that no longer offers
//! passkeys with 400 `WebAuthnNotEnabledException`, a pool whose relying party
//! changed with 400 `WebAuthnRelyingPartyMismatchException`, an origin that is not
//! the relying party's with 400 `WebAuthnOriginNotAllowedException`, a credential
//! that is not a passkey with 400 `WebAuthnCredentialNotSupportedException`, a
//! response this service cannot read with 400 `InvalidParameterException`, a user
//! already holding every passkey one user may hold with 400
//! `LimitExceededException`, and a token that names no signed-in user with 403
//! `NotAuthorizedException`.
//!
//! The pools, app clients, users, passkey configurations and access tokens are
//! all real: they come from `CreateUserPool`, `CreateUserPoolClient`,
//! `SetUserPoolMfaConfig`, `AdminCreateUser`, `AdminSetUserPassword` and
//! `InitiateAuth`. Each table row names the case it asserts and every assertion
//! reports the actual value.

use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// A request's SigV4 credential scope is what routes an administrative call to
/// this service. The signature is never checked here: the suite turns request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// This service's regional endpoint host, which a signed call is addressed to.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The emulator's own endpoint: an address, so an unsigned request addressed to
/// it names no service in its `Host` header and `X-Amz-Target` is the only
/// routing signal left.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// `AllowedFirstAuthFactors` of a pool that offers passkey sign-in
/// (`smithy_query(query="shape",shape="SignInPolicyType")`, whose values are
/// `AuthFactorType`'s).
const PASSKEY_FACTORS: [&str; 2] = ["PASSWORD", "WEB_AUTHN"];

/// The relying party the pools of this suite register passkeys under.
const RELYING_PARTY: &str = "passkeys.example.com";

/// The attestation statement a registration response carries. This service asks
/// for none (`attestation: "none"` in the creation options) and reads none, so
/// the value is opaque here.
const ATTESTATION_OBJECT: &str = "o2NmbXRkbm9uZWdhdHRTdG10oGhhdXRoRGF0YUA";

/// One emulator with just this service mounted, and the engine its state lives
/// in, so a suite can read what a call persisted and plant what a quota needs.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    router.set_request_authentication(false);
    Emulator { engine, router }
}

/// Dispatch `operation` with `body` and return its status and JSON response
/// body. A `signed` call carries a credential scope, as the SDK sends every
/// administrative operation; an unsigned one is addressed to the emulator's own
/// address, which is how a signed-in user's client calls the self-service
/// operations the model marks `@optionalAuth`.
async fn dispatch(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    signed: bool,
) -> (u16, Value) {
    let host = if signed { ENDPOINT_HOST } else { EMULATOR_HOST };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        );
    if signed {
        request = request.header("authorization", AUTHORIZATION);
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call the signed `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body, true).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool whose sign-in policy admits `factors` and return its minted
/// `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str, factors: &[&str]) -> String {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": name,
            "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": factors } },
        }),
    )
    .await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client named `name` that allows the password sign-in this suite
/// uses, and return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str, name: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": name,
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Create `username` with a PERMANENT password, so the user is `CONFIRMED` and
/// can sign in.
async fn confirmed_user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
}

/// Set the pool's passkey relying party through `SetUserPoolMfaConfig`.
async fn set_relying_party(router: &MultiServiceRouter, user_pool_id: &str, relying_party: &str) {
    ok(
        router,
        "SetUserPoolMfaConfig",
        &json!({
            "UserPoolId": user_pool_id,
            "WebAuthnConfiguration": { "RelyingPartyId": relying_party },
        }),
    )
    .await;
}

/// A pool that offers passkeys under [`RELYING_PARTY`], one app client of it, and
/// one signed-in user: the state every row of this suite starts from.
struct SignedIn {
    user_pool_id: String,
    client_id: String,
    username: String,
    access_token: String,
}

/// Build the pool, client and user named after `name` and sign the user in.
async fn signed_in(router: &MultiServiceRouter, name: &str) -> SignedIn {
    let user_pool_id = pool(router, &format!("{name}-pool"), &PASSKEY_FACTORS).await;
    set_relying_party(router, &user_pool_id, RELYING_PARTY).await;
    let client_id = client(router, &user_pool_id, &format!("{name}-client")).await;
    let username = format!("{name}-user");
    confirmed_user(router, &user_pool_id, &username).await;
    let access_token = access_token(router, &client_id, &username).await;
    SignedIn {
        user_pool_id,
        client_id,
        username,
        access_token,
    }
}

/// Sign `username` in through an unsigned `InitiateAuth` and return the access
/// token of that sign-in.
async fn access_token(router: &MultiServiceRouter, client_id: &str, username: &str) -> String {
    let (status, payload) = dispatch(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "signing {username} in must succeed, got {status} with {payload}"
    );
    payload["AuthenticationResult"]["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string()
}

/// Start a passkey registration, requiring success, and return the challenge its
/// credential creation options carry.
async fn start_challenge(router: &MultiServiceRouter, token: &str, case: &str) -> String {
    let options = start_options(router, token, case).await;
    options["challenge"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the options must carry a challenge, got {options}"))
        .to_string()
}

/// Start a passkey registration, requiring success, and return the
/// `CredentialCreationOptions` it answered.
async fn start_options(router: &MultiServiceRouter, token: &str, case: &str) -> Value {
    let (status, payload) = dispatch(
        router,
        "StartWebAuthnRegistration",
        &json!({ "AccessToken": token }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartWebAuthnRegistration must succeed, got {status} with {payload}"
    );
    payload["CredentialCreationOptions"].clone()
}

/// Call `CompleteWebAuthnRegistration` unsigned, as a signed-in user's client
/// does, and return its status and body.
async fn complete(router: &MultiServiceRouter, token: &str, credential: &Value) -> (u16, Value) {
    dispatch(
        router,
        "CompleteWebAuthnRegistration",
        &json!({ "AccessToken": token, "Credential": credential }),
        false,
    )
    .await
}

/// Call `CompleteWebAuthnRegistration` unsigned, requiring success.
async fn complete_ok(router: &MultiServiceRouter, token: &str, credential: &Value, case: &str) {
    let (status, payload) = complete(router, token, credential).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CompleteWebAuthnRegistration must succeed, got {status} with {payload}"
    );
}

/// What one passkey provider reports for one registration: the row of a table
/// whose rows differ along the credential id, the origin's form, the attachment
/// reported and the transports reported.
#[derive(Clone, Copy, Debug)]
struct PasskeyResponse {
    case: &'static str,
    /// `PublicKeyCredential.id`, the credential id the authenticator chose.
    credential_id: &'static str,
    /// The serialized origin the registration was collected at.
    origin: &'static str,
    /// `PublicKeyCredential.authenticatorAttachment`, when the provider reports
    /// one.
    authenticator_attachment: Option<&'static str>,
    /// `AuthenticatorAttestationResponseJSON.transports`, in the order reported.
    transports: &'static [&'static str],
}

impl PasskeyResponse {
    /// This response as the `RegistrationResponseJSON` the `Credential` member
    /// carries, answering `challenge`.
    fn document(&self, challenge: &str) -> Value {
        let mut response = Map::new();
        response.insert(
            "clientDataJSON".to_string(),
            json!(client_data_json("webauthn.create", challenge, self.origin)),
        );
        response.insert("attestationObject".to_string(), json!(ATTESTATION_OBJECT));
        if !self.transports.is_empty() {
            response.insert("transports".to_string(), json!(self.transports));
        }
        let mut credential = Map::new();
        credential.insert("id".to_string(), json!(self.credential_id));
        credential.insert("rawId".to_string(), json!(self.credential_id));
        credential.insert("type".to_string(), json!("public-key"));
        credential.insert("response".to_string(), Value::Object(response));
        if let Some(attachment) = self.authenticator_attachment {
            credential.insert("authenticatorAttachment".to_string(), json!(attachment));
        }
        credential.insert("clientExtensionResults".to_string(), json!({}));
        Value::Object(credential)
    }
}

/// The `CollectedClientData` of a ceremony of type `ceremony`, base64url-encoded
/// as `AuthenticatorResponseJSON.clientDataJSON` carries it (W3C Web
/// Authentication Level 2 §5.8.1).
fn client_data_json(ceremony: &str, challenge: &str, origin: &str) -> String {
    let client_data = json!({
        "type": ceremony,
        "challenge": challenge,
        "origin": origin,
        "crossOrigin": false,
    });
    state::base64_url(client_data.to_string().as_bytes())
}

/// The passkey credentials the stored user holds, in the order the store holds
/// them, which is registration order.
fn stored_credentials(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
) -> Vec<state::WebAuthnCredential> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .expect("the created pool")
        .user(username)
        .expect("the created user")
        .web_authn_credentials
        .clone()
}

/// Whether the store still holds an answerable registration for `challenge`.
fn holds_registration(engine: &Arc<engine::InMemoryEngine>, challenge: &str) -> bool {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard.web_authn_registrations.contains_key(challenge)
}

/// Plant `count` passkey credentials on the stored user, standing for ones
/// completed registrations stored, so a row can drive a quota this suite would
/// otherwise need that many registrations to reach.
fn plant_credentials(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    count: usize,
) {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let mut guard = service_state.lock();
    let user = guard
        .pool_mut(user_pool_id)
        .expect("the created pool")
        .user_mut(username)
        .expect("the created user");
    for ordinal in 0..count {
        user.web_authn_credentials.push(state::WebAuthnCredential {
            credential_id: format!("planted-credential-{ordinal}"),
            friendly_credential_name: RELYING_PARTY.to_string(),
            relying_party_id: RELYING_PARTY.to_string(),
            authenticator_attachment: None,
            authenticator_transports: vec!["internal".to_string()],
            created_at: SystemTime::now(),
        });
    }
}

/// One passkey provider per row, differing along the credential id, the form of
/// the origin the registration was collected at, the attachment reported and the
/// transports reported.
const REGISTERED: [PasskeyResponse; 4] = [
    PasskeyResponse {
        case: "a platform authenticator at the relying party's own origin",
        credential_id: "Y3JlZGVudGlhbC1vbmU",
        origin: "https://passkeys.example.com",
        authenticator_attachment: Some("platform"),
        transports: &["internal"],
    },
    PasskeyResponse {
        case: "a roaming authenticator at an origin carrying a port",
        credential_id: "cm9hbWluZy1hdXRoZW50aWNhdG9yLWNyZWRlbnRpYWwtaWQ",
        origin: "https://passkeys.example.com:8443",
        authenticator_attachment: Some("cross-platform"),
        transports: &["usb", "nfc"],
    },
    PasskeyResponse {
        case: "an authenticator reporting neither attachment nor transports, at a subdomain origin",
        credential_id: "c3ViZG9tYWluLWNyZWRlbnRpYWw",
        origin: "https://app.passkeys.example.com",
        authenticator_attachment: None,
        transports: &[],
    },
    PasskeyResponse {
        case: "an authenticator reporting three transports at a plaintext origin",
        credential_id: "dGhyZWUtdHJhbnNwb3J0cy1jcmVkZW50aWFsLWlkLXRoYXQtaXMtbG9uZ2VyLXRoYW4tdGhlLW90aGVyLXJvd3M",
        origin: "http://passkeys.example.com",
        authenticator_attachment: Some("platform"),
        transports: &["hybrid", "internal", "ble"],
    },
];

/// Every row of [`REGISTERED`] registers the passkey it answers, in registration
/// order, and the credential is then read back through the operations that report
/// it.
#[tokio::test]
async fn completing_a_registration_registers_the_passkey_it_answers() {
    let Emulator { engine, router } = emulator();
    let signed_in = signed_in(&router, "registered").await;

    for (ordinal, row) in REGISTERED.iter().enumerate() {
        let case = row.case;
        let challenge = start_challenge(&router, &signed_in.access_token, case).await;
        complete_ok(
            &router,
            &signed_in.access_token,
            &row.document(&challenge),
            case,
        )
        .await;

        // The challenge is answered ONCE: completing the registration consumes
        // it.
        assert!(
            !holds_registration(&engine, &challenge),
            "case {case:?}: the completed registration must be consumed, the store still holds \
             its challenge"
        );

        let stored = stored_credentials(&engine, &signed_in.user_pool_id, &signed_in.username);
        assert_eq!(
            stored.len(),
            ordinal + 1,
            "case {case:?}: the user must hold one passkey per completed registration, got {}",
            stored.len()
        );
        let credential = &stored[ordinal];
        assert_eq!(
            credential.credential_id, row.credential_id,
            "case {case:?}: the registered credential id, got {:?}",
            credential.credential_id
        );
        assert_eq!(
            credential.relying_party_id, RELYING_PARTY,
            "case {case:?}: the relying party the passkey is registered under, got {:?}",
            credential.relying_party_id
        );
        assert!(
            !credential.friendly_credential_name.is_empty(),
            "case {case:?}: a passkey carries an automatically-generated friendly name, got {:?}",
            credential.friendly_credential_name
        );
        assert_eq!(
            credential.authenticator_attachment.as_deref(),
            row.authenticator_attachment,
            "case {case:?}: the attachment the authenticator reported, got {:?}",
            credential.authenticator_attachment
        );
        assert_eq!(
            credential.authenticator_transports,
            row.transports
                .iter()
                .map(|transport| transport.to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: the transports the authenticator reported, got {:?}",
            credential.authenticator_transports
        );
    }

    // READ BACK through an implemented reader: the passkeys a user has already
    // registered are what `StartWebAuthnRegistration` answers as
    // `excludeCredentials`, in registration order.
    let options = start_options(&router, &signed_in.access_token, "read back").await;
    let expected = Value::Array(
        REGISTERED
            .iter()
            .map(|row| {
                json!({
                    "type": "public-key",
                    "id": row.credential_id,
                    "transports": row.transports,
                })
            })
            .collect(),
    );
    assert_eq!(
        options["excludeCredentials"], expected,
        "the registered passkeys, in registration order, got {}",
        options["excludeCredentials"]
    );

    // And a user holding a passkey can present the passkey factor, which is what
    // `GetUserAuthFactors` reports.
    let factors = dispatch(
        &router,
        "GetUserAuthFactors",
        &json!({ "AccessToken": signed_in.access_token }),
        false,
    )
    .await;
    assert_eq!(
        factors.0, 200,
        "GetUserAuthFactors must succeed, got {} with {}",
        factors.0, factors.1
    );
    assert_eq!(
        factors.1["ConfiguredUserAuthFactors"],
        json!(["PASSWORD", "WEB_AUTHN"]),
        "a user holding a passkey can sign in with it, got {}",
        factors.1["ConfiguredUserAuthFactors"]
    );
}

/// A registration response carrying the challenge of a registration already
/// completed resolves nothing: one registration registers ONE passkey.
#[tokio::test]
async fn a_challenge_is_answered_once() {
    let Emulator { engine, router } = emulator();
    let signed_in = signed_in(&router, "answered-once").await;
    let row = REGISTERED[0];
    let challenge = start_challenge(&router, &signed_in.access_token, row.case).await;

    complete_ok(
        &router,
        &signed_in.access_token,
        &row.document(&challenge),
        row.case,
    )
    .await;
    let (status, payload) = complete(
        &router,
        &signed_in.access_token,
        &PasskeyResponse {
            credential_id: "YS1zZWNvbmQtY3JlZGVudGlhbC1pZA",
            ..row
        }
        .document(&challenge),
    )
    .await;

    assert_eq!(
        status, 400,
        "answering a consumed challenge is refused, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "WebAuthnChallengeNotFoundException",
        "answering a consumed challenge is refused, got {}",
        error_code(&payload)
    );
    let stored = stored_credentials(&engine, &signed_in.user_pool_id, &signed_in.username);
    assert_eq!(
        stored.len(),
        1,
        "the consumed challenge registered no second passkey, got {stored:?}"
    );
}

/// A registration response this service refuses, built for a live registration's
/// `challenge`.
type RefusedResponse = fn(&str) -> Value;

/// The client data of an AUTHENTICATION ceremony, which is no registration
/// response (W3C Web Authentication Level 2 §7.1, step 7).
fn authentication_ceremony(challenge: &str) -> Value {
    let mut credential = REGISTERED[0].document(challenge);
    credential["response"]["clientDataJSON"] = json!(client_data_json(
        "webauthn.get",
        challenge,
        REGISTERED[0].origin
    ));
    credential
}

/// Client data that is not a base64url encoding of anything.
fn unreadable_client_data(challenge: &str) -> Value {
    let mut credential = REGISTERED[0].document(challenge);
    credential["response"]["clientDataJSON"] = json!("not base64url at all!!");
    credential
}

/// Client data that decodes to JSON carrying no `challenge`.
fn client_data_without_a_challenge(challenge: &str) -> Value {
    let mut credential = REGISTERED[0].document(challenge);
    credential["response"]["clientDataJSON"] = json!(state::base64_url(
        json!({ "type": "webauthn.create", "origin": REGISTERED[0].origin })
            .to_string()
            .as_bytes()
    ));
    credential
}

/// A credential document carrying no authenticator response at all.
fn no_authenticator_response(challenge: &str) -> Value {
    let mut credential = REGISTERED[0].document(challenge);
    credential
        .as_object_mut()
        .expect("a credential object")
        .remove("response");
    credential
}

/// A credential whose reported type is not the one the Web Authentication API
/// defines, so it is no passkey.
fn not_a_public_key_credential(challenge: &str) -> Value {
    let mut credential = REGISTERED[0].document(challenge);
    credential["type"] = json!("secret-key");
    credential
}

/// A registration collected at the origin of another relying party.
fn origin_of_another_relying_party(challenge: &str) -> Value {
    PasskeyResponse {
        origin: "https://passkeys.example.net",
        ..REGISTERED[0]
    }
    .document(challenge)
}

/// A registration collected at a host that merely ENDS IN the relying party id
/// without being a subdomain of it.
fn origin_merely_ending_in_the_relying_party(challenge: &str) -> Value {
    PasskeyResponse {
        origin: "https://notpasskeys.example.com",
        ..REGISTERED[0]
    }
    .document(challenge)
}

/// A registration collected at a host that is an ADDRESS LITERAL rather than a
/// domain, which is a registrable domain suffix of no relying party id (the
/// bracketed IPv6 form, whose own colons are not a port).
fn origin_at_an_address_literal(challenge: &str) -> Value {
    PasskeyResponse {
        origin: "https://[::1]:8443",
        ..REGISTERED[0]
    }
    .document(challenge)
}

/// A response answering a challenge this emulator never handed out.
fn an_unknown_challenge(_challenge: &str) -> Value {
    REGISTERED[0].document("bm90LWEtY2hhbGxlbmdlLXRoaXMtZW11bGF0b3ItaGFuZGVkLW91dA")
}

/// Each row is one refused registration response and the error code it is refused
/// with; all of them are refused with HTTP 400, register nothing, and leave the
/// registration they answered answerable.
const REFUSED: [(&str, RefusedResponse, &str); 9] = [
    (
        "client data collected for an authentication ceremony",
        authentication_ceremony,
        "InvalidParameterException",
    ),
    (
        "client data that is not base64url-encoded JSON",
        unreadable_client_data,
        "InvalidParameterException",
    ),
    (
        "client data carrying no challenge",
        client_data_without_a_challenge,
        "InvalidParameterException",
    ),
    (
        "a credential carrying no authenticator response",
        no_authenticator_response,
        "InvalidParameterException",
    ),
    (
        "a credential that is not a public-key credential",
        not_a_public_key_credential,
        "WebAuthnCredentialNotSupportedException",
    ),
    (
        "an origin of another relying party",
        origin_of_another_relying_party,
        "WebAuthnOriginNotAllowedException",
    ),
    (
        "an origin merely ending in the relying party id",
        origin_merely_ending_in_the_relying_party,
        "WebAuthnOriginNotAllowedException",
    ),
    (
        "an origin whose host is an address literal, not a domain",
        origin_at_an_address_literal,
        "WebAuthnOriginNotAllowedException",
    ),
    (
        "a challenge this emulator never handed out",
        an_unknown_challenge,
        "WebAuthnChallengeNotFoundException",
    ),
];

/// Every refused response registers NO passkey and leaves the registration it
/// answered answerable, so the provider can answer it again — which the last step
/// of this test does.
#[tokio::test]
async fn a_refused_response_registers_nothing_and_leaves_the_registration_answerable() {
    let Emulator { engine, router } = emulator();
    let signed_in = signed_in(&router, "refused").await;

    for (case, build, expected_code) in REFUSED {
        let challenge = start_challenge(&router, &signed_in.access_token, case).await;
        let (status, payload) = complete(&router, &signed_in.access_token, &build(&challenge)).await;

        assert_eq!(
            status, 400,
            "case {case:?}: the response is refused, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            expected_code,
            "case {case:?}: the error code, got {} with message {:?}",
            error_code(&payload),
            error_message(&payload)
        );
        assert!(
            !error_message(&payload).is_empty(),
            "case {case:?}: the refusal carries a message, got {payload}"
        );
        let stored = stored_credentials(&engine, &signed_in.user_pool_id, &signed_in.username);
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused response registers no passkey, got {stored:?}"
        );
        assert!(
            holds_registration(&engine, &challenge),
            "case {case:?}: a refused response leaves the registration answerable, the store no \
             longer holds its challenge"
        );

        // The registration is still answerable, which the provider proves by
        // answering it correctly.
        complete_ok(
            &router,
            &signed_in.access_token,
            &REGISTERED[0].document(&challenge),
            case,
        )
        .await;
        let stored = stored_credentials(&engine, &signed_in.user_pool_id, &signed_in.username);
        assert_eq!(
            stored.len(),
            1,
            "case {case:?}: answering the same registration correctly registers the passkey, got \
             {stored:?}"
        );

        // Forget the passkey again, so the next row starts from a user holding
        // none and the credential id is free.
        let service_state = state::service_state(&*engine).expect("this service's state");
        service_state
            .lock()
            .pool_mut(&signed_in.user_pool_id)
            .expect("the created pool")
            .user_mut(&signed_in.username)
            .expect("the created user")
            .web_authn_credentials
            .clear();
    }
}

/// A response presented with the access token of ANOTHER app client of the same
/// pool is refused: the registration recorded the client it was started on.
#[tokio::test]
async fn a_response_presented_by_another_app_client_is_refused() {
    let Emulator { engine, router } = emulator();
    let signed_in = signed_in(&router, "other-client").await;
    let other_client = client(&router, &signed_in.user_pool_id, "other-client").await;
    let other_token = access_token(&router, &other_client, &signed_in.username).await;
    let case = "a token of another app client of the same pool";

    let challenge = start_challenge(&router, &signed_in.access_token, case).await;
    let (status, payload) = complete(
        &router,
        &other_token,
        &REGISTERED[0].document(&challenge),
    )
    .await;

    assert_eq!(
        status, 400,
        "case {case:?}: the response is refused, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "WebAuthnClientMismatchException",
        "case {case:?}: the error code, got {}",
        error_code(&payload)
    );
    assert!(
        stored_credentials(&engine, &signed_in.user_pool_id, &signed_in.username).is_empty(),
        "case {case:?}: no passkey is registered"
    );
}

/// A registration started under one relying party is not completed under another:
/// a pool whose relying party changed while the registration was in flight
/// registers nothing.
#[tokio::test]
async fn a_registration_of_another_relying_party_is_refused() {
    let Emulator { engine, router } = emulator();
    let signed_in = signed_in(&router, "other-relying-party").await;
    let case = "the pool's relying party changed while the registration was in flight";

    let challenge = start_challenge(&router, &signed_in.access_token, case).await;
    set_relying_party(&router, &signed_in.user_pool_id, "passkeys.example.org").await;
    let (status, payload) = complete(
        &router,
        &signed_in.access_token,
        &REGISTERED[0].document(&challenge),
    )
    .await;

    assert_eq!(
        status, 400,
        "case {case:?}: the response is refused, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "WebAuthnRelyingPartyMismatchException",
        "case {case:?}: the error code, got {}",
        error_code(&payload)
    );
    assert!(
        stored_credentials(&engine, &signed_in.user_pool_id, &signed_in.username).is_empty(),
        "case {case:?}: no passkey is registered"
    );
}

/// A pool that no longer admits the passkey sign-in factor has not enabled the
/// feature, so it registers no passkey.
#[tokio::test]
async fn a_pool_that_no_longer_offers_passkeys_registers_nothing() {
    let Emulator { engine, router } = emulator();
    let signed_in = signed_in(&router, "not-enabled").await;
    let case = "the pool stopped offering the passkey factor";

    let challenge = start_challenge(&router, &signed_in.access_token, case).await;
    ok(
        &router,
        "UpdateUserPool",
        &json!({
            "UserPoolId": signed_in.user_pool_id,
            "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": ["PASSWORD"] } },
        }),
    )
    .await;
    let (status, payload) = complete(
        &router,
        &signed_in.access_token,
        &REGISTERED[0].document(&challenge),
    )
    .await;

    assert_eq!(
        status, 400,
        "case {case:?}: the response is refused, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "WebAuthnNotEnabledException",
        "case {case:?}: the error code, got {}",
        error_code(&payload)
    );
    assert!(
        stored_credentials(&engine, &signed_in.user_pool_id, &signed_in.username).is_empty(),
        "case {case:?}: no passkey is registered"
    );
}

/// A user already holding every passkey one user may hold registers no more, and
/// the bound is the one the store enforces.
#[tokio::test]
async fn a_user_holding_every_passkey_registers_no_more() {
    let Emulator { engine, router } = emulator();
    let signed_in = signed_in(&router, "limit").await;
    let case = "a user already holding every passkey one user may hold";

    // The registration is started while the user is still under the bound; the
    // passkeys that take it to the bound are planted, so this row needs one
    // registration rather than the whole quota's worth.
    let challenge = start_challenge(&router, &signed_in.access_token, case).await;
    plant_credentials(
        &engine,
        &signed_in.user_pool_id,
        &signed_in.username,
        state::MAX_WEB_AUTHN_CREDENTIALS_PER_USER,
    );
    let (status, payload) = complete(
        &router,
        &signed_in.access_token,
        &REGISTERED[0].document(&challenge),
    )
    .await;

    assert_eq!(
        status, 400,
        "case {case:?}: the response is refused, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "LimitExceededException",
        "case {case:?}: the error code, got {}",
        error_code(&payload)
    );
    let stored = stored_credentials(&engine, &signed_in.user_pool_id, &signed_in.username);
    assert_eq!(
        stored.len(),
        state::MAX_WEB_AUTHN_CREDENTIALS_PER_USER,
        "case {case:?}: the bound is not passed, got {} credentials",
        stored.len()
    );
}

/// A credential id the user already holds a passkey under registers no second
/// one: a credential id identifies ONE passkey.
#[tokio::test]
async fn a_credential_id_already_registered_is_refused() {
    let Emulator { engine, router } = emulator();
    let signed_in = signed_in(&router, "duplicate").await;
    let row = REGISTERED[0];
    let case = "a credential id the user already holds a passkey under";

    let first = start_challenge(&router, &signed_in.access_token, case).await;
    complete_ok(&router, &signed_in.access_token, &row.document(&first), case).await;
    let second = start_challenge(&router, &signed_in.access_token, case).await;
    let (status, payload) = complete(
        &router,
        &signed_in.access_token,
        &row.document(&second),
    )
    .await;

    assert_eq!(
        status, 400,
        "case {case:?}: the response is refused, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "InvalidParameterException",
        "case {case:?}: the error code, got {}",
        error_code(&payload)
    );
    let stored = stored_credentials(&engine, &signed_in.user_pool_id, &signed_in.username);
    assert_eq!(
        stored.len(),
        1,
        "case {case:?}: the user still holds one passkey, got {stored:?}"
    );
}

/// Every token that names no signed-in user is refused with 403
/// `NotAuthorizedException`, whatever the response carries.
#[tokio::test]
async fn a_token_that_names_no_signed_in_user_registers_nothing() {
    let Emulator { engine, router } = emulator();
    let signed_in = signed_in(&router, "unauthorized").await;
    let challenge = start_challenge(&router, &signed_in.access_token, "unauthorized").await;
    let id_token = {
        let (status, payload) = dispatch(
            &router,
            "InitiateAuth",
            &json!({
                "ClientId": signed_in.client_id,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": {
                    "USERNAME": signed_in.username,
                    "PASSWORD": GOOD_PASSWORD,
                },
            }),
            false,
        )
        .await;
        assert_eq!(status, 200, "signing in must succeed, got {payload}");
        payload["AuthenticationResult"]["IdToken"]
            .as_str()
            .expect("an id token")
            .to_string()
    };

    // The rows differ along WHAT the presented token is: one this emulator never
    // issued, and one it issued for another purpose than naming a signed-in user.
    for (case, token) in [
        (
            "a token this emulator never issued",
            "not-a-token-this-emulator-issued".to_string(),
        ),
        ("an id token rather than an access token", id_token),
    ] {
        let (status, payload) = complete(&router, &token, &REGISTERED[0].document(&challenge)).await;
        assert_eq!(
            status, 403,
            "case {case:?}: the response is refused, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "NotAuthorizedException",
            "case {case:?}: the error code, got {}",
            error_code(&payload)
        );
        assert_eq!(
            error_message(&payload),
            "Invalid Access Token",
            "case {case:?}: the refusal message, got {:?}",
            error_message(&payload)
        );
        assert!(
            stored_credentials(&engine, &signed_in.user_pool_id, &signed_in.username).is_empty(),
            "case {case:?}: no passkey is registered"
        );
    }
}
