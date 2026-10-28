// SPDX-License-Identifier: CC0-1.0

//! `ListWebAuthnCredentials` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.ListWebAuthnCredentials`).
//!
//! The rule under test: the listing reports the passkey credentials STORED for
//! the user the access token names, each rendered from the stored credential,
//! ordered by creation instant — registration order, since a completed
//! registration appends — with `MaxResults` capping one page and the `NextToken`
//! of a non-final page resuming the listing strictly after the last credential
//! it served. Only that user's passkeys are reported, a `NextToken` this listing
//! never minted names no page (400 `InvalidParameterException`), a token that
//! names no signed-in user is refused (403 `NotAuthorizedException`), and the
//! read changes nothing a later caller sees.
//!
//! The pools, app clients, users, passkey configurations, access tokens and
//! credentials are all real: they come from `CreateUserPool`,
//! `CreateUserPoolClient`, `SetUserPoolMfaConfig`, `AdminCreateUser`,
//! `AdminSetUserPassword`, `InitiateAuth`, `StartWebAuthnRegistration` and
//! `CompleteWebAuthnRegistration`. Each table row names the case it asserts and
//! every assertion reports the actual value.

use std::sync::Arc;

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
/// in, so a suite can read what a call persisted.
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

/// A pool that offers passkeys under [`RELYING_PARTY`], one app client of it, and
/// the users it holds: the state every row of this suite starts from.
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

/// Register `passkey` for the user `token` names, through
/// `StartWebAuthnRegistration` and `CompleteWebAuthnRegistration`.
async fn register(router: &MultiServiceRouter, token: &str, passkey: Passkey, case: &str) {
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
    let challenge = payload["CredentialCreationOptions"]["challenge"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the options must carry a challenge, got {payload}"))
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

/// A `ListWebAuthnCredentials` request body, carrying only the members a case
/// names.
fn list_request(token: &str, max_results: Option<i32>, next_token: Option<&str>) -> Value {
    let mut body = Map::new();
    body.insert("AccessToken".to_string(), json!(token));
    if let Some(max_results) = max_results {
        body.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(next_token) = next_token {
        body.insert("NextToken".to_string(), json!(next_token));
    }
    Value::Object(body)
}

/// Call `ListWebAuthnCredentials` unsigned, as a signed-in user's client does.
async fn list(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    dispatch(router, "ListWebAuthnCredentials", body, false).await
}

/// Call `ListWebAuthnCredentials`, requiring success, and return its body.
async fn list_ok(router: &MultiServiceRouter, body: &Value, case: &str) -> Value {
    let (status, payload) = list(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListWebAuthnCredentials must succeed, got {status} with {payload}"
    );
    payload
}

/// The `Credentials` list of a response.
fn entries(payload: &Value, case: &str) -> Vec<Value> {
    payload["Credentials"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the response must carry a Credentials list, got {payload}")
        })
        .clone()
}

/// The credential ids a response reports, in the order it reports them.
fn ids_of(payload: &Value, case: &str) -> Vec<String> {
    entries(payload, case)
        .iter()
        .map(|entry| {
            entry["CredentialId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an entry must name a credential"))
                .to_string()
        })
        .collect()
}

/// Page the whole listing under `max_results` and return every entry served, in
/// order, beside the number of pages it took.
async fn walk(
    router: &MultiServiceRouter,
    token: &str,
    max_results: Option<i32>,
    case: &str,
) -> (Vec<Value>, usize) {
    let mut served = Vec::new();
    let mut pages = 0;
    let mut next_token: Option<String> = None;
    loop {
        let payload = list_ok(
            router,
            &list_request(token, max_results, next_token.as_deref()),
            case,
        )
        .await;
        pages += 1;
        served.extend(entries(&payload, case));
        next_token = payload
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            return (served, pages);
        }
        assert!(
            pages <= state::MAX_WEB_AUTHN_CREDENTIALS_PER_USER + 1,
            "case {case:?}: paging must terminate, got {pages} pages serving {} entries",
            served.len()
        );
    }
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

/// Assert that `entry` renders `stored`, member by member.
fn assert_entry(case: &str, entry: &Value, stored: &state::WebAuthnCredential) {
    assert_eq!(
        entry["CredentialId"].as_str(),
        Some(stored.credential_id.as_str()),
        "case {case:?}: the entry reports the stored credential id, got {entry}"
    );
    assert_eq!(
        entry["FriendlyCredentialName"].as_str(),
        Some(stored.friendly_credential_name.as_str()),
        "case {case:?}: the entry reports the stored friendly name, got {entry}"
    );
    assert_eq!(
        entry["RelyingPartyId"].as_str(),
        Some(stored.relying_party_id.as_str()),
        "case {case:?}: the entry reports the stored relying party, got {entry}"
    );
    assert_eq!(
        entry["AuthenticatorAttachment"].as_str(),
        stored.authenticator_attachment.as_deref(),
        "case {case:?}: the entry reports the stored attachment, got {entry}"
    );
    let transports: Vec<String> = entry["AuthenticatorTransports"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the entry must carry its transports, got {entry}"))
        .iter()
        .map(|transport| {
            transport
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a transport is a string, got {entry}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        transports, stored.authenticator_transports,
        "case {case:?}: the entry reports the stored transports in order, got {transports:?}"
    );
    let created_at = entry["CreatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: the entry must carry CreatedAt, got {entry}"));
    let expected = state::to_date_time(stored.created_at).as_secs_f64();
    assert!(
        (created_at - expected).abs() < 1.0,
        "case {case:?}: the entry reports the stored creation instant {expected}, got {created_at}"
    );
}

/// One passkey provider per row, differing along the credential id, the
/// attachment reported and the transports reported. The ids descend
/// alphabetically while the registrations ascend in time, so an order taken
/// from the ids is not the order under test.
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
        credential_id: "alfa-hybrid-credential",
        authenticator_attachment: Some("cross-platform"),
        transports: &["hybrid", "internal", "ble"],
    },
];

#[tokio::test]
async fn a_listing_reports_the_registered_passkeys_in_registration_order() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "order", &["ada"]).await;
    let token = access_token(&router, &fixture.client_id, "ada").await;

    // Nothing is registered yet, so the required `Credentials` member is present
    // and empty rather than absent.
    let payload = list_ok(&router, &list_request(&token, None, None), "no passkey yet").await;
    assert_eq!(
        ids_of(&payload, "no passkey yet"),
        Vec::<String>::new(),
        "case {:?}: a user holding no passkey is listed as an empty list, got {payload}",
        "no passkey yet"
    );
    assert!(
        payload.get("NextToken").is_none(),
        "case {:?}: a final page carries no NextToken, got {payload}",
        "no passkey yet"
    );

    for passkey in REGISTERED {
        register(&router, &token, passkey, "registration order").await;
    }

    let case = "every registered passkey, in registration order";
    let payload = list_ok(&router, &list_request(&token, None, None), case).await;
    let expected: Vec<String> = REGISTERED
        .iter()
        .map(|passkey| passkey.credential_id.to_string())
        .collect();
    let actual = ids_of(&payload, case);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );

    // Each entry is the STORED credential, member by member, and the creation
    // instants ascend with the registrations.
    let stored = stored_credentials(&engine, &fixture.user_pool_id, "ada");
    let served = entries(&payload, case);
    assert_eq!(
        served.len(),
        stored.len(),
        "case {case:?}: the listing reports every stored credential, got {} of {}",
        served.len(),
        stored.len()
    );
    for (entry, credential) in served.iter().zip(stored.iter()) {
        assert_entry(case, entry, credential);
    }
    let instants: Vec<f64> = served
        .iter()
        .map(|entry| entry["CreatedAt"].as_f64().unwrap_or_default())
        .collect();
    assert!(
        instants.windows(2).all(|pair| pair[0] <= pair[1]),
        "case {case:?}: the listing ascends by creation instant, got {instants:?}"
    );

    // The listing is a READ: asking again answers the same, and the store still
    // holds exactly what the registrations put there.
    let again = list_ok(&router, &list_request(&token, None, None), "a second read").await;
    assert_eq!(
        ids_of(&again, "a second read"),
        actual,
        "case {:?}: a read changes nothing a later caller sees, got {again}",
        "a second read"
    );
    let stored_again = stored_credentials(&engine, &fixture.user_pool_id, "ada");
    let stored_ids: Vec<String> = stored_again
        .iter()
        .map(|credential| credential.credential_id.clone())
        .collect();
    assert_eq!(
        stored_ids, expected,
        "case {:?}: the store still holds the registered passkeys, got {stored_ids:?}",
        "a second read"
    );
}

#[tokio::test]
async fn a_listing_pages_over_the_registration_order_under_every_max_results() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "paging", &["grace"]).await;
    let token = access_token(&router, &fixture.client_id, "grace").await;

    // Seven registrations: a count no page size of this suite divides evenly.
    for ordinal in 0..7 {
        let credential_id: &'static str = [
            "paged-credential-g",
            "paged-credential-f",
            "paged-credential-e",
            "paged-credential-d",
            "paged-credential-c",
            "paged-credential-b",
            "paged-credential-a",
        ][ordinal];
        register(
            &router,
            &token,
            Passkey {
                credential_id,
                authenticator_attachment: Some("platform"),
                transports: &["internal"],
            },
            "paging fixture",
        )
        .await;
    }
    let stored = stored_credentials(&engine, &fixture.user_pool_id, "grace");
    let expected: Vec<String> = stored
        .iter()
        .map(|credential| credential.credential_id.clone())
        .collect();

    // The rows span the published bounds of `MaxResults` (0 and 20,
    // `smithy_query(query="operation",operation="ListWebAuthnCredentials")`), the
    // sizes that divide and do not divide the seven credentials, the size that
    // holds them all exactly, and an absent `MaxResults`.
    for (case, max_results, expected_pages) in [
        ("no MaxResults at all", None, 1),
        (
            "MaxResults 1, the smallest page that makes progress",
            Some(1),
            7,
        ),
        ("MaxResults 2, which does not divide seven", Some(2), 4),
        ("MaxResults 7, exactly the credential count", Some(7), 1),
        ("MaxResults 6, one short of the credential count", Some(6), 2),
        ("MaxResults 20, the modelled maximum", Some(20), 1),
        ("MaxResults 0, the modelled minimum", Some(0), 1),
    ] {
        let (served, pages) = walk(&router, &token, max_results, case).await;
        let actual: Vec<String> = served
            .iter()
            .map(|entry| {
                entry["CredentialId"]
                    .as_str()
                    .expect("an entry names a credential")
                    .to_string()
            })
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: paging the whole listing serves every passkey exactly once in \
             registration order: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
        // Every entry a page served is the stored credential, whatever page it
        // came on.
        for entry in &served {
            let id = entry["CredentialId"]
                .as_str()
                .expect("an entry names a credential");
            let credential = stored
                .iter()
                .find(|credential| credential.credential_id == id)
                .unwrap_or_else(|| panic!("case {case:?}: {id:?} was registered above"));
            assert_entry(case, entry, credential);
        }
    }
}

#[tokio::test]
async fn a_listing_reports_the_passkeys_of_the_tokens_own_user_alone() {
    let Emulator { router, .. } = emulator();
    let fixture = fixture(&router, "one-pool", &["alice", "bob", "carol"]).await;

    let alices_token = access_token(&router, &fixture.client_id, "alice").await;
    let bobs_token = access_token(&router, &fixture.client_id, "bob").await;
    let carols_token = access_token(&router, &fixture.client_id, "carol").await;

    for passkey in &REGISTERED[..2] {
        register(&router, &alices_token, *passkey, "alice's passkeys").await;
    }
    for passkey in &REGISTERED[2..] {
        register(&router, &bobs_token, *passkey, "bob's passkeys").await;
    }

    for (case, token, expected) in [
        (
            "the user the first passkeys were registered for",
            &alices_token,
            REGISTERED[..2]
                .iter()
                .map(|passkey| passkey.credential_id.to_string())
                .collect::<Vec<String>>(),
        ),
        (
            "another user of the same pool",
            &bobs_token,
            REGISTERED[2..]
                .iter()
                .map(|passkey| passkey.credential_id.to_string())
                .collect::<Vec<String>>(),
        ),
        ("a user holding no passkey at all", &carols_token, Vec::new()),
    ] {
        let payload = list_ok(&router, &list_request(token, None, None), case).await;
        let actual = ids_of(&payload, case);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // A `NextToken` is a page of ONE user's listing: alice's names no page of
    // bob's.
    let case = "alice's NextToken presented on bob's listing";
    let first_page = list_ok(
        &router,
        &list_request(&alices_token, Some(1), None),
        "alice's first page",
    )
    .await;
    let cursor = first_page["NextToken"]
        .as_str()
        .expect("a non-final page carries a NextToken")
        .to_string();
    let (status, payload) = list(
        &router,
        &list_request(&bobs_token, Some(1), Some(&cursor)),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case {case:?}: a token of another listing names no page, got {status} with {payload}"
    );
}

#[tokio::test]
async fn a_token_that_names_no_signed_in_user_is_refused() {
    let Emulator { router, .. } = emulator();
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
        let (status, payload) = list(&router, &list_request(token, None, None)).await;
        assert_eq!(
            (status, error_code(&payload)),
            (403, "NotAuthorizedException"),
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
    let (status, payload) = dispatch(
        &router,
        "AdminUserGlobalSignOut",
        &json!({ "UserPoolId": fixture.user_pool_id, "Username": "dana" }),
        true,
    )
    .await;
    assert_eq!(
        status, 200,
        "AdminUserGlobalSignOut must succeed, got {status} with {payload}"
    );
    let case = "the access token of a session that signed out";
    let (status, payload) = list(&router, &list_request(&access, None, None)).await;
    assert_eq!(
        (status, error_code(&payload)),
        (403, "NotAuthorizedException"),
        "case {case:?}: a revoked token is refused, got {status} with {payload}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let Emulator { router, .. } = emulator();
    let fixture = fixture(&router, "constraints", &["erin"]).await;
    let token = access_token(&router, &fixture.client_id, "erin").await;
    register(&router, &token, REGISTERED[0], "the constraint fixture").await;

    // One value past each published bound of this operation's input
    // (`smithy_query(query="operation",operation="ListWebAuthnCredentials")`):
    // `MaxResults` range 0..20, `AccessToken` pattern `^[A-Za-z0-9-_=.]+$`, and
    // `NextToken` length 1..131072 with pattern `^[\S]+$`.
    for (case, body) in [
        (
            "MaxResults 21, one past the range maximum 20",
            list_request(&token, Some(21), None),
        ),
        (
            "MaxResults -1, one below the range minimum 0",
            list_request(&token, Some(-1), None),
        ),
        (
            "an access token holding a character the pattern excludes",
            list_request("two words", None, None),
        ),
        (
            "an empty NextToken, below the length minimum 1",
            list_request(&token, None, Some("")),
        ),
        (
            "a NextToken holding a space, which the pattern excludes",
            list_request(&token, None, Some("two words")),
        ),
        ("a request missing the required AccessToken", json!({})),
    ] {
        let (status, payload) = list(&router, &body).await;
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

    // A well-formed `NextToken` this listing never minted names no page.
    let case = "a NextToken this listing never minted";
    let (status, payload) = list(
        &router,
        &list_request(&token, None, Some("bm90LWEtcGFnZS10b2tlbg")),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case {case:?}: an unminted token names no page, got {status} with {payload}"
    );
}
