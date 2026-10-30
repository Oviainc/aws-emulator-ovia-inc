// SPDX-License-Identifier: CC0-1.0

//! `DeleteWebAuthnCredential` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.DeleteWebAuthnCredential`).
//!
//! The rule under test: deleting a passkey removes the ONE stored credential the
//! `CredentialId` names from the user the access token names, leaves every other
//! credential of that user in registration order, and frees the slot that
//! credential took against the per-user passkey bound. The deletion is read back
//! through the operations that report a user's passkeys
//! (`ListWebAuthnCredentials`, and `StartWebAuthnRegistration`'s
//! `excludeCredentials`). A `CredentialId` the signed-in user holds no passkey
//! under names no resource — a second deletion of the same credential, and the
//! credential id of another user's passkey, included — so it is refused with 400
//! `ResourceNotFoundException` and deletes nothing, and a token that names no
//! signed-in user is refused with 400 `NotAuthorizedException`.
//!
//! The pools, app clients, users, passkey configurations, access tokens and
//! credentials are all real: they come from `CreateUserPool`,
//! `CreateUserPoolClient`, `SetUserPoolMfaConfig`, `AdminCreateUser`,
//! `AdminSetUserPassword`, `InitiateAuth`, `StartWebAuthnRegistration` and
//! `CompleteWebAuthnRegistration`. Each table row names the case it asserts and
//! every assertion reports the actual value.

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

/// The origin the registrations of this suite are collected at: the relying
/// party's own.
const ORIGIN: &str = "https://passkeys.example.com";

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

/// A pool that offers passkeys under [`RELYING_PARTY`] and one app client of it:
/// the state every row of this suite starts from.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Build the pool and app client named after `name`, with one confirmed user per
/// entry of `usernames`.
async fn fixture(router: &MultiServiceRouter, name: &str, usernames: &[&str]) -> Fixture {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": format!("{name}-pool"),
            "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": PASSKEY_FACTORS } },
        }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    ok(
        router,
        "SetUserPoolMfaConfig",
        &json!({
            "UserPoolId": user_pool_id,
            "WebAuthnConfiguration": { "RelyingPartyId": RELYING_PARTY },
        }),
    )
    .await;
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{name}-client"),
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    let client_id = created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    for username in usernames {
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
    Fixture {
        user_pool_id,
        client_id,
    }
}

/// Sign `username` in through an unsigned `InitiateAuth` and return that
/// sign-in's `AuthenticationResult`.
async fn sign_in(router: &MultiServiceRouter, client_id: &str, username: &str) -> Value {
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
    payload["AuthenticationResult"].clone()
}

/// Sign `username` in and return the access token of that sign-in.
async fn access_token(router: &MultiServiceRouter, client_id: &str, username: &str) -> String {
    sign_in(router, client_id, username).await["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string()
}

/// The `CollectedClientData` of a registration ceremony, base64url-encoded as
/// `AuthenticatorResponseJSON.clientDataJSON` carries it (W3C Web Authentication
/// Level 2 §5.8.1).
fn client_data_json(challenge: &str) -> String {
    let client_data = json!({
        "type": "webauthn.create",
        "challenge": challenge,
        "origin": ORIGIN,
        "crossOrigin": false,
    });
    state::base64_url(client_data.to_string().as_bytes())
}

/// What one passkey provider reports for one registration: the row of a table
/// whose rows differ along the credential id, the attachment reported and the
/// transports reported.
#[derive(Clone, Copy, Debug)]
struct Passkey {
    /// `PublicKeyCredential.id`, the credential id the authenticator chose.
    credential_id: &'static str,
    /// `PublicKeyCredential.authenticatorAttachment`, when the provider reports
    /// one.
    authenticator_attachment: Option<&'static str>,
    /// `AuthenticatorAttestationResponseJSON.transports`, in the order reported.
    transports: &'static [&'static str],
}

/// Start a passkey registration for the user `token` names, requiring success,
/// and return the `CredentialCreationOptions` it answered.
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

/// Register `passkey` for the user `token` names, through
/// `StartWebAuthnRegistration` and `CompleteWebAuthnRegistration`.
async fn register(router: &MultiServiceRouter, token: &str, passkey: Passkey, case: &str) {
    let options = start_options(router, token, case).await;
    let challenge = options["challenge"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the options must carry a challenge, got {options}"))
        .to_string();

    let mut response = Map::new();
    response.insert(
        "clientDataJSON".to_string(),
        json!(client_data_json(&challenge)),
    );
    response.insert("attestationObject".to_string(), json!(ATTESTATION_OBJECT));
    if !passkey.transports.is_empty() {
        response.insert("transports".to_string(), json!(passkey.transports));
    }
    let mut credential = Map::new();
    credential.insert("id".to_string(), json!(passkey.credential_id));
    credential.insert("rawId".to_string(), json!(passkey.credential_id));
    credential.insert("type".to_string(), json!("public-key"));
    credential.insert("response".to_string(), Value::Object(response));
    if let Some(attachment) = passkey.authenticator_attachment {
        credential.insert("authenticatorAttachment".to_string(), json!(attachment));
    }

    let (status, payload) = dispatch(
        router,
        "CompleteWebAuthnRegistration",
        &json!({ "AccessToken": token, "Credential": Value::Object(credential) }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: registering {:?} must succeed, got {status} with {payload}",
        passkey.credential_id
    );
}

/// Call `DeleteWebAuthnCredential` unsigned, as a signed-in user's client does.
async fn delete(router: &MultiServiceRouter, token: &str, credential_id: &str) -> (u16, Value) {
    dispatch(
        router,
        "DeleteWebAuthnCredential",
        &json!({ "AccessToken": token, "CredentialId": credential_id }),
        false,
    )
    .await
}

/// Call `DeleteWebAuthnCredential`, requiring success.
async fn delete_ok(router: &MultiServiceRouter, token: &str, credential_id: &str, case: &str) {
    let (status, payload) = delete(router, token, credential_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: deleting {credential_id:?} must succeed, got {status} with {payload}"
    );
    // The operation models no output member, so a success carries nothing but
    // the empty document of this protocol.
    assert_eq!(
        payload,
        json!({}),
        "case {case:?}: a success carries no output member, got {payload}"
    );
}

/// The credential ids `ListWebAuthnCredentials` reports for the user `token`
/// names, in the order it reports them.
async fn listed_ids(router: &MultiServiceRouter, token: &str, case: &str) -> Vec<String> {
    let (status, payload) = dispatch(
        router,
        "ListWebAuthnCredentials",
        &json!({ "AccessToken": token }),
        false,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListWebAuthnCredentials must succeed, got {status} with {payload}"
    );
    payload["Credentials"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the response must carry a Credentials list, got {payload}")
        })
        .iter()
        .map(|entry| {
            entry["CredentialId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an entry must name a credential"))
                .to_string()
        })
        .collect()
}

/// The credential ids a fresh `StartWebAuthnRegistration` excludes for the user
/// `token` names: the passkeys that user still holds (W3C Web Authentication
/// Level 2 §5.4, `excludeCredentials`).
async fn excluded_ids(router: &MultiServiceRouter, token: &str, case: &str) -> Vec<String> {
    let options = start_options(router, token, case).await;
    options["excludeCredentials"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the options must carry excludeCredentials, got {options}")
        })
        .iter()
        .map(|entry| {
            entry["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an excluded credential names an id"))
                .to_string()
        })
        .collect()
}

/// The credential ids the stored user holds, in the order the store holds them,
/// which is registration order.
fn stored_ids(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
) -> Vec<String> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .expect("the created pool")
        .user(username)
        .expect("the created user")
        .web_authn_credentials
        .iter()
        .map(|credential| credential.credential_id.clone())
        .collect()
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

/// One passkey provider per row, differing along the credential id, the
/// attachment reported and the transports reported. The ids descend
/// alphabetically while the registrations ascend in time, so an order taken
/// from the ids is not the order the store holds them in.
const REGISTERED: [Passkey; 4] = [
    Passkey {
        credential_id: "zulu-platform-credential",
        authenticator_attachment: Some("platform"),
        transports: &["internal"],
    },
    Passkey {
        credential_id: "yankee-roaming-credential",
        authenticator_attachment: Some("cross-platform"),
        transports: &["usb", "nfc"],
    },
    Passkey {
        credential_id: "mike-silent-credential",
        authenticator_attachment: None,
        transports: &[],
    },
    Passkey {
        credential_id: "alfa-hybrid-credential-with-a-much-longer-identifier",
        authenticator_attachment: Some("cross-platform"),
        transports: &["hybrid", "internal", "ble"],
    },
];

#[tokio::test]
async fn deleting_a_passkey_removes_that_credential_alone() {
    // One row per position the deleted credential takes among the ones the user
    // holds, and one for a user holding a single passkey: the rule holds
    // wherever the credential sits and however many there are.
    for (case, registered, deleted) in [
        ("the first of four registered passkeys", &REGISTERED[..], 0),
        ("one in the middle of four registered passkeys", &REGISTERED[..], 1),
        ("the last of four registered passkeys", &REGISTERED[..], 3),
        ("the only passkey the user holds", &REGISTERED[..1], 0),
        ("the second of two registered passkeys", &REGISTERED[..2], 1),
    ] {
        let Emulator { engine, router } = emulator();
        let fixture = fixture(&router, "deleted", &["ada"]).await;
        let token = access_token(&router, &fixture.client_id, "ada").await;
        for passkey in registered {
            register(&router, &token, *passkey, case).await;
        }

        let target = registered[deleted].credential_id;
        let expected: Vec<String> = registered
            .iter()
            .filter(|passkey| passkey.credential_id != target)
            .map(|passkey| passkey.credential_id.to_string())
            .collect();

        delete_ok(&router, &token, target, case).await;

        let stored = stored_ids(&engine, &fixture.user_pool_id, "ada");
        assert_eq!(
            stored, expected,
            "case {case:?}: the store holds the other passkeys in registration order: \
             expected {expected:?}, got {stored:?}"
        );
        let listed = listed_ids(&router, &token, case).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: the listing reports the other passkeys: expected {expected:?}, \
             got {listed:?}"
        );
        let excluded = excluded_ids(&router, &token, case).await;
        assert_eq!(
            excluded, expected,
            "case {case:?}: a fresh registration excludes the passkeys that remain: \
             expected {expected:?}, got {excluded:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_passkey_frees_the_slot_it_took() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "limit", &["grace"]).await;
    let token = access_token(&router, &fixture.client_id, "grace").await;

    // One short of the per-user bound is registered for real, so the credential
    // deleted below is one a registration stored; the rest stand in for
    // registrations this suite would otherwise have to drive.
    register(&router, &token, REGISTERED[0], "at the bound").await;
    plant_credentials(
        &engine,
        &fixture.user_pool_id,
        "grace",
        state::MAX_WEB_AUTHN_CREDENTIALS_PER_USER - 1,
    );

    let case = "a user already holding every passkey one user may hold";
    let (status, payload) = dispatch(
        &router,
        "StartWebAuthnRegistration",
        &json!({ "AccessToken": token }),
        false,
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "LimitExceededException"),
        "case {case:?}: a user at the bound registers no more, got {status} with {payload}"
    );

    let case = "the bound after one passkey is deleted";
    delete_ok(&router, &token, REGISTERED[0].credential_id, case).await;
    let stored = stored_ids(&engine, &fixture.user_pool_id, "grace");
    assert_eq!(
        stored.len(),
        state::MAX_WEB_AUTHN_CREDENTIALS_PER_USER - 1,
        "case {case:?}: one passkey fewer is held, got {stored:?}"
    );
    // The slot the deleted credential took is free, so the registration the
    // bound refused above is answerable again.
    let options = start_options(&router, &token, case).await;
    assert!(
        options["challenge"].is_string(),
        "case {case:?}: a freed slot admits a registration, got {options}"
    );
}

#[tokio::test]
async fn a_credential_id_naming_no_passkey_of_the_user_is_refused() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "not-found", &["bob", "eve"]).await;
    let bobs_token = access_token(&router, &fixture.client_id, "bob").await;
    let eves_token = access_token(&router, &fixture.client_id, "eve").await;
    for passkey in &REGISTERED[..2] {
        register(&router, &bobs_token, *passkey, "bob's passkeys").await;
    }
    register(&router, &eves_token, REGISTERED[2], "eve's passkey").await;

    // Deleting one of bob's passkeys first, so the SAME id is then an id he
    // holds no passkey under.
    delete_ok(
        &router,
        &bobs_token,
        REGISTERED[0].credential_id,
        "bob's first passkey",
    )
    .await;

    // Each row is a `CredentialId` the signed-in user holds no passkey under:
    // one already deleted, one never registered anywhere, the empty id the
    // modelled length minimum admits, and the id of ANOTHER user's passkey.
    for (case, token, credential_id) in [
        (
            "a credential this user already deleted",
            &bobs_token,
            REGISTERED[0].credential_id,
        ),
        (
            "a credential id nothing ever registered",
            &bobs_token,
            "never-registered-credential",
        ),
        (
            "the empty credential id, the modelled length minimum 0",
            &bobs_token,
            "",
        ),
        (
            "another user's credential id",
            &bobs_token,
            REGISTERED[2].credential_id,
        ),
    ] {
        let (status, payload) = delete(&router, token, credential_id).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: an id naming no passkey of this user is refused, got {status} \
             with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            "The credential does not exist",
            "case {case:?}: the refusal reports the credential was not found, got {payload}"
        );
    }

    // A refused deletion deletes nothing: bob keeps his remaining passkey and
    // eve keeps hers.
    let case = "the state after every refused deletion";
    let bobs = stored_ids(&engine, &fixture.user_pool_id, "bob");
    assert_eq!(
        bobs,
        vec![REGISTERED[1].credential_id.to_string()],
        "case {case:?}: the signed-in user keeps the passkeys it holds, got {bobs:?}"
    );
    let eves = stored_ids(&engine, &fixture.user_pool_id, "eve");
    assert_eq!(
        eves,
        vec![REGISTERED[2].credential_id.to_string()],
        "case {case:?}: another user's passkey is untouched, got {eves:?}"
    );
}

#[tokio::test]
async fn a_token_that_names_no_signed_in_user_deletes_nothing() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "tokens", &["dana"]).await;
    let result = sign_in(&router, &fixture.client_id, "dana").await;
    let access = result["AccessToken"]
        .as_str()
        .expect("an access token")
        .to_string();
    let id_token = result["IdToken"].as_str().expect("an id token").to_string();
    let refresh_token = result["RefreshToken"]
        .as_str()
        .expect("a refresh token")
        .to_string();
    register(&router, &access, REGISTERED[0], "the signed-in user").await;

    for (case, token) in [
        ("a value this emulator never issued", "not-a-token-at-all"),
        ("the id token of the same sign-in", id_token.as_str()),
        (
            "the refresh token of the same sign-in",
            refresh_token.as_str(),
        ),
    ] {
        let (status, payload) = delete(&router, token, REGISTERED[0].credential_id).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "NotAuthorizedException"),
            "case {case:?}: a token naming no signed-in user is refused, got {status} with \
             {payload}"
        );
        assert_eq!(
            error_message(&payload),
            "Invalid Access Token",
            "case {case:?}: the refusal reports the token was not accepted, got {payload}"
        );
    }

    // The access token of a session an administrator signed out names no
    // signed-in user either.
    ok(
        &router,
        "AdminUserGlobalSignOut",
        &json!({ "UserPoolId": fixture.user_pool_id, "Username": "dana" }),
    )
    .await;
    let case = "the access token of a session that signed out";
    let (status, payload) = delete(&router, &access, REGISTERED[0].credential_id).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "NotAuthorizedException"),
        "case {case:?}: a revoked token is refused, got {status} with {payload}"
    );

    let stored = stored_ids(&engine, &fixture.user_pool_id, "dana");
    assert_eq!(
        stored,
        vec![REGISTERED[0].credential_id.to_string()],
        "case {case:?}: a refused request deletes no passkey, got {stored:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "constraints", &["erin"]).await;
    let token = access_token(&router, &fixture.client_id, "erin").await;
    register(&router, &token, REGISTERED[0], "the constraint fixture").await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="DeleteWebAuthnCredential")`):
    // `AccessToken` pattern `^[A-Za-z0-9-_=.]+$`, `CredentialId` length
    // 0..131072, both required.
    let past_the_maximum = "c".repeat(131_073);
    for (case, body) in [
        (
            "an access token holding a character the pattern excludes",
            json!({ "AccessToken": "two words", "CredentialId": "any-credential" }),
        ),
        (
            "a request missing the required AccessToken",
            json!({ "CredentialId": "any-credential" }),
        ),
        (
            "a request missing the required CredentialId",
            json!({ "AccessToken": token }),
        ),
        (
            "a CredentialId one character past the length maximum 131072",
            json!({ "AccessToken": token, "CredentialId": past_the_maximum }),
        ),
    ] {
        let (status, payload) = dispatch(&router, "DeleteWebAuthnCredential", &body, false).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside a modelled constraint is a 400, got {status} \
             with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {case:?}: a bad request is never answered with the scaffold's 501, got \
             {payload}"
        );
    }

    // A `CredentialId` AT the length maximum satisfies every constraint, so it
    // reaches the lookup and is refused for naming no passkey of this user.
    let case = "a CredentialId of exactly the length maximum 131072";
    let at_the_maximum = "c".repeat(131_072);
    let (status, payload) = delete(&router, &token, &at_the_maximum).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "case {case:?}: a well-formed id naming no passkey is refused as not found, got \
         {status} with {payload}"
    );

    let stored = stored_ids(&engine, &fixture.user_pool_id, "erin");
    assert_eq!(
        stored,
        vec![REGISTERED[0].credential_id.to_string()],
        "case {case:?}: a refused request deletes no passkey, got {stored:?}"
    );
}
