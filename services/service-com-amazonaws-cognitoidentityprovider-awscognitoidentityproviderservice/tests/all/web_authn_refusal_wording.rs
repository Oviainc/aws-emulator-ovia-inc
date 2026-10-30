// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the passkey surface's refusals, and the user-state
//! precondition every self-service call is served behind, over the wire (`POST
//! /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<operation>`).
//!
//! An error's status and code say what KIND of refusal it is; its message is
//! what a caller reads and branches on, so each of these is one sentence built
//! by a rule rather than freely worded per case. The rules under test:
//!
//! * a challenge that names no answerable registration carries ONE sentence,
//!   the same whether the challenge was never handed out, was already answered,
//!   was handed out to another user, or has been taken past its window by the
//!   clock — so the refusal is no oracle for which of those it was, and names
//!   neither the challenge nor the operation that minted it;
//! * a `Credential` this service cannot read as a registration response carries
//!   ONE sentence for the whole class — an absent member, a member carrying no
//!   value this service can read, and client data collected for another ceremony
//!   alike — naming no member of the presented credential;
//! * a pool that does not admit the passkey sign-in factor reports the feature
//!   is not enabled in ONE sentence, the same from every operation that gates on
//!   it;
//! * a `CredentialId` the signed-in user holds no passkey under carries ONE
//!   sentence, so a credential registered to ANOTHER user is reported exactly as
//!   one nothing ever registered;
//! * a live access token whose user owes a new password serves NO self-service
//!   call: every operation that resolves a signed-in user refuses with 400
//!   `PasswordResetRequiredException` before doing its own work, and serves the
//!   same token again once that password is set.
//!
//! The pools, app clients, users, passkey configurations, access tokens and
//! credentials are all real: they come from `CreateUserPool`,
//! `CreateUserPoolClient`, `SetUserPoolMfaConfig`, `UpdateUserPool`,
//! `AdminCreateUser`, `AdminSetUserPassword`, `AdminResetUserPassword`,
//! `InitiateAuth`, `StartWebAuthnRegistration` and
//! `CompleteWebAuthnRegistration`. Each table row names the case it asserts and
//! every assertion reports the actual value.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
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

/// The password a completed reset sets, so the row that clears a reset chooses a
/// different password than the one the user signed in with.
const NEXT_PASSWORD: &str = "Passw0rd!2025";

/// `AllowedFirstAuthFactors` of a pool that offers passkey sign-in
/// (`smithy_query(query="shape",shape="SignInPolicyType")`, whose values are
/// `AuthFactorType`'s).
const PASSKEY_FACTORS: [&str; 2] = ["PASSWORD", "WEB_AUTHN"];

/// `AllowedFirstAuthFactors` of a pool that offers no passkey sign-in at all.
const PASSWORD_ONLY_FACTORS: [&str; 1] = ["PASSWORD"];

/// The relying party the pools of this suite register passkeys under.
const RELYING_PARTY: &str = "passkeys.example.com";

/// The origin the registrations of this suite are collected at: the relying
/// party's own.
const ORIGIN: &str = "https://passkeys.example.com";

/// The attestation statement a registration response carries. This service asks
/// for none (`attestation: "none"` in the creation options) and reads none, so
/// the value is opaque here.
const ATTESTATION_OBJECT: &str = "o2NmbXRkbm9uZWdhdHRTdG10oGhhdXRoRGF0YUA";

/// The credential id the passkey of a row carries.
const CREDENTIAL_ID: &str = "cmVmdXNhbC13b3JkaW5nLWNyZWRlbnRpYWw";

/// The one sentence a challenge that names no answerable registration is refused
/// with.
const CHALLENGE_NOT_FOUND_MESSAGE: &str = "The challenge does not exist or has expired";

/// The one sentence a `Credential` this service cannot read as a registration
/// response is refused with.
const INVALID_CREDENTIAL_DATA_MESSAGE: &str = "Credential data is not valid";

/// The one sentence a pool that does not admit the passkey factor is reported
/// with.
const WEB_AUTHN_NOT_ENABLED_MESSAGE: &str = "WebAuthn not enabled for this pool.";

/// The one sentence a `CredentialId` naming no passkey of the signed-in user is
/// refused with.
const CREDENTIAL_NOT_FOUND_MESSAGE: &str = "The credential does not exist";

/// The one sentence a user owing a new password is reported with.
const PASSWORD_RESET_REQUIRED_MESSAGE: &str = "Password reset required for the user";

/// One emulator with just this service mounted, and the engine its state lives
/// in, so a suite can read what a call persisted and age what the clock would
/// otherwise have to.
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

/// A pool, one app client of it, and one confirmed user per name asked for: the
/// state every row of this suite starts from.
struct Fixture {
    user_pool_id: String,
    client_id: String,
}

/// Build the pool and app client named after `name`, admitting `factors` as its
/// first authentication factors, with one confirmed user per entry of
/// `usernames`. Each user holds a verified email, which is the destination a
/// self-service password reset is delivered to.
async fn fixture(
    router: &MultiServiceRouter,
    name: &str,
    factors: &[&str],
    usernames: &[&str],
) -> Fixture {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": format!("{name}-pool"),
            "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": factors } },
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
            &json!({
                "UserPoolId": user_pool_id,
                "Username": username,
                "UserAttributes": [
                    { "Name": "email", "Value": format!("{username}@example.com") },
                    { "Name": "email_verified", "Value": "true" },
                ],
            }),
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

/// Sign `username` in through an unsigned `InitiateAuth` with `password` and
/// return the access token of that sign-in.
async fn access_token(
    router: &MultiServiceRouter,
    client_id: &str,
    username: &str,
    password: &str,
) -> String {
    let (status, payload) = dispatch(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": password },
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
        .unwrap_or_else(|| panic!("the sign-in issues an access token, got {payload}"))
        .to_string()
}

/// Call `StartWebAuthnRegistration` unsigned, as a signed-in user's client does.
async fn start(router: &MultiServiceRouter, token: &str) -> (u16, Value) {
    dispatch(
        router,
        "StartWebAuthnRegistration",
        &json!({ "AccessToken": token }),
        false,
    )
    .await
}

/// Start a registration, requiring success, and return the challenge its
/// credential creation options carry.
async fn start_challenge(router: &MultiServiceRouter, token: &str, case: &str) -> String {
    let (status, payload) = start(router, token).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartWebAuthnRegistration must succeed, got {status} with {payload}"
    );
    payload["CredentialCreationOptions"]["challenge"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the options must carry a challenge, got {payload}"))
        .to_string()
}

/// Call `CompleteWebAuthnRegistration` unsigned, as a signed-in user's client
/// does.
async fn complete(
    router: &MultiServiceRouter,
    token: &str,
    credential: &Value,
) -> (u16, Value) {
    dispatch(
        router,
        "CompleteWebAuthnRegistration",
        &json!({ "AccessToken": token, "Credential": credential }),
        false,
    )
    .await
}

/// Register one passkey under `credential_id` for the user `token` names,
/// through both registration operations, and return the challenge it answered.
async fn register(
    router: &MultiServiceRouter,
    token: &str,
    credential_id: &str,
    case: &str,
) -> String {
    let challenge = start_challenge(router, token, case).await;
    let (status, payload) =
        complete(router, token, &credential_document(credential_id, &challenge)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: registering {credential_id:?} must succeed, got {status} with {payload}"
    );
    challenge
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

/// The `CollectedClientData` of a registration ceremony answering `challenge`
/// (W3C Web Authentication Level 2 §5.8.1).
fn registration_client_data(challenge: &str) -> Value {
    json!({
        "type": "webauthn.create",
        "challenge": challenge,
        "origin": ORIGIN,
        "crossOrigin": false,
    })
}

/// The `RegistrationResponseJSON` a passkey provider reports for a registration
/// of `credential_id` answering `challenge`: the well-formed response every
/// malformed row below is derived from.
fn credential_document(credential_id: &str, challenge: &str) -> Value {
    json!({
        "id": credential_id,
        "rawId": credential_id,
        "type": "public-key",
        "authenticatorAttachment": "platform",
        "response": {
            "clientDataJSON": state::base64_url(
                registration_client_data(challenge).to_string().as_bytes(),
            ),
            "attestationObject": ATTESTATION_OBJECT,
            "transports": ["internal", "hybrid"],
        },
    })
}

/// The well-formed response with its collected client data replaced by
/// `client_data`, base64url-encoded as `clientDataJSON` carries it.
fn credential_with_client_data(challenge: &str, client_data: &Value) -> Value {
    let mut credential = credential_document(CREDENTIAL_ID, challenge);
    credential["response"]["clientDataJSON"] =
        json!(state::base64_url(client_data.to_string().as_bytes()));
    credential
}

/// Drop the member `name` of `credential`, which the caller knows it carries.
fn without(credential: &mut Value, name: &str) {
    credential
        .as_object_mut()
        .expect("the response is an object")
        .remove(name)
        .unwrap_or_else(|| panic!("the response carries {name}"));
}

/// The passkey credential ids the stored user holds, in the order the store
/// holds them, which is registration order.
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

/// Age the stored registration `challenge` belongs to past its own window, so
/// the clock has taken it out of reach without the suite waiting the whole
/// registration timeout out.
fn expire_registration(engine: &Arc<engine::InMemoryEngine>, challenge: &str) {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let mut guard = service_state.lock();
    let registration = guard
        .web_authn_registrations
        .get_mut(challenge)
        .expect("the started registration");
    registration.expires_at = SystemTime::now()
        .checked_sub(Duration::from_secs(1))
        .expect("an instant before now");
}

/// How one row presents a challenge that names no answerable registration: it
/// gets the challenge of a registration started for the signed-in user and
/// answers with the challenge the row chooses.
type ChallengeCase = fn(&Arc<engine::InMemoryEngine>, &str, &str) -> String;

/// A challenge value this emulator never handed out at all.
fn never_handed_out(
    _engine: &Arc<engine::InMemoryEngine>,
    _mine: &str,
    _other_users: &str,
) -> String {
    "bm90LWEtY2hhbGxlbmdlLXRoaXMtZW11bGF0b3ItaGFuZGVkLW91dA".to_string()
}

/// The challenge of a registration the clock has taken past its window.
fn taken_past_its_window(
    engine: &Arc<engine::InMemoryEngine>,
    mine: &str,
    _other_users: &str,
) -> String {
    expire_registration(engine, mine);
    mine.to_string()
}

/// The challenge of a registration started for ANOTHER user of the same pool,
/// which is no registration this caller can answer.
fn started_for_another_user(
    _engine: &Arc<engine::InMemoryEngine>,
    _mine: &str,
    other_users: &str,
) -> String {
    other_users.to_string()
}

/// Each row is one challenge that names no answerable registration, differing
/// along WHY it names none: never minted, minted and aged out, minted for
/// somebody else.
const UNANSWERABLE: [(&str, ChallengeCase); 3] = [
    (
        "a challenge this emulator never handed out",
        never_handed_out,
    ),
    (
        "a challenge the clock has taken past its window",
        taken_past_its_window,
    ),
    (
        "a challenge started for another user of the same pool",
        started_for_another_user,
    ),
];

/// Every challenge that names no answerable registration is refused with the
/// same sentence, so the refusal tells an expired challenge from an unknown one
/// in neither its code nor its message — and a challenge already ANSWERED, which
/// the last step of this test replays, is reported the same way.
#[tokio::test]
async fn every_unanswerable_challenge_is_reported_with_the_same_sentence() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "unanswerable", &PASSKEY_FACTORS, &["ana", "ben"]).await;
    let mine = access_token(&router, &fixture.client_id, "ana", GOOD_PASSWORD).await;
    let theirs = access_token(&router, &fixture.client_id, "ben", GOOD_PASSWORD).await;

    for (case, challenge_of) in UNANSWERABLE {
        let my_challenge = start_challenge(&router, &mine, case).await;
        let their_challenge = start_challenge(&router, &theirs, case).await;
        let presented = challenge_of(&engine, &my_challenge, &their_challenge);

        let (status, payload) =
            complete(&router, &mine, &credential_document(CREDENTIAL_ID, &presented)).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "WebAuthnChallengeNotFoundException"),
            "case {case:?}: the response is refused as naming no challenge, got {status} with \
             {payload}"
        );
        assert_eq!(
            error_message(&payload),
            CHALLENGE_NOT_FOUND_MESSAGE,
            "case {case:?}: the one sentence for an unanswerable challenge, got {:?}",
            error_message(&payload)
        );
        let stored = stored_ids(&engine, &fixture.user_pool_id, "ana");
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused response registers no passkey, got {stored:?}"
        );
    }

    // A challenge the emulator DID hand out to this very user, and which a
    // completed registration has already consumed, is reported with the same
    // sentence: one registration registers one passkey.
    let case = "a challenge a completed registration already answered";
    let challenge = register(&router, &mine, CREDENTIAL_ID, case).await;
    let (status, payload) =
        complete(&router, &mine, &credential_document(CREDENTIAL_ID, &challenge)).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "WebAuthnChallengeNotFoundException"),
        "case {case:?}: the replay is refused, got {status} with {payload}"
    );
    assert_eq!(
        error_message(&payload),
        CHALLENGE_NOT_FOUND_MESSAGE,
        "case {case:?}: the one sentence for an unanswerable challenge, got {:?}",
        error_message(&payload)
    );
    let stored = stored_ids(&engine, &fixture.user_pool_id, "ana");
    assert_eq!(
        stored,
        vec![CREDENTIAL_ID.to_string()],
        "case {case:?}: the replay registers no second passkey, got {stored:?}"
    );
}

/// How one row malforms the well-formed registration response answering the
/// challenge it is given.
type MalformedCase = fn(&str) -> Value;

fn without_a_credential_type(challenge: &str) -> Value {
    let mut credential = credential_document(CREDENTIAL_ID, challenge);
    without(&mut credential, "type");
    credential
}

fn without_a_credential_id(challenge: &str) -> Value {
    let mut credential = credential_document(CREDENTIAL_ID, challenge);
    without(&mut credential, "id");
    without(&mut credential, "rawId");
    credential
}

fn with_an_empty_credential_id(challenge: &str) -> Value {
    let mut credential = credential_document(CREDENTIAL_ID, challenge);
    credential["id"] = json!("");
    credential["rawId"] = json!("");
    credential
}

fn without_an_authenticator_response(challenge: &str) -> Value {
    let mut credential = credential_document(CREDENTIAL_ID, challenge);
    without(&mut credential, "response");
    credential
}

fn without_client_data(challenge: &str) -> Value {
    let mut credential = credential_document(CREDENTIAL_ID, challenge);
    without(&mut credential["response"], "clientDataJSON");
    credential
}

fn client_data_that_is_not_base64url(challenge: &str) -> Value {
    let mut credential = credential_document(CREDENTIAL_ID, challenge);
    credential["response"]["clientDataJSON"] = json!("not base64url at all!!");
    credential
}

fn client_data_that_is_not_json(challenge: &str) -> Value {
    let mut credential = credential_document(CREDENTIAL_ID, challenge);
    credential["response"]["clientDataJSON"] = json!(state::base64_url(b"these are not JSON bytes"));
    credential
}

fn client_data_of_an_authentication_ceremony(challenge: &str) -> Value {
    let mut client_data = registration_client_data(challenge);
    // "webauthn.get" is the ceremony type of an AUTHENTICATION, which is no
    // registration response (W3C Web Authentication Level 2 §5.8.1).
    client_data["type"] = json!("webauthn.get");
    credential_with_client_data(challenge, &client_data)
}

fn client_data_of_no_ceremony(challenge: &str) -> Value {
    let mut client_data = registration_client_data(challenge);
    without(&mut client_data, "type");
    credential_with_client_data(challenge, &client_data)
}

fn client_data_without_a_challenge(challenge: &str) -> Value {
    let mut client_data = registration_client_data(challenge);
    without(&mut client_data, "challenge");
    credential_with_client_data(challenge, &client_data)
}

fn client_data_with_an_empty_challenge(challenge: &str) -> Value {
    let mut client_data = registration_client_data(challenge);
    client_data["challenge"] = json!("");
    credential_with_client_data(challenge, &client_data)
}

fn client_data_without_an_origin(challenge: &str) -> Value {
    let mut client_data = registration_client_data(challenge);
    without(&mut client_data, "origin");
    credential_with_client_data(challenge, &client_data)
}

fn a_credential_that_is_no_object(_challenge: &str) -> Value {
    json!("not an object at all")
}

/// Each row is one `Credential` this service cannot read as a registration
/// response: a member that is absent, one carrying no value it can read, and
/// client data collected for something other than a registration.
const MALFORMED: [(&str, MalformedCase); 13] = [
    ("no credential type", without_a_credential_type),
    ("no credential id under either name", without_a_credential_id),
    ("an empty credential id", with_an_empty_credential_id),
    (
        "no authenticator response",
        without_an_authenticator_response,
    ),
    ("no collected client data", without_client_data),
    (
        "client data that is not base64url",
        client_data_that_is_not_base64url,
    ),
    (
        "client data that is base64url of something that is not JSON",
        client_data_that_is_not_json,
    ),
    (
        "client data collected for an authentication ceremony",
        client_data_of_an_authentication_ceremony,
    ),
    ("client data naming no ceremony", client_data_of_no_ceremony),
    (
        "client data carrying no challenge",
        client_data_without_a_challenge,
    ),
    (
        "client data carrying an empty challenge",
        client_data_with_an_empty_challenge,
    ),
    (
        "client data carrying no origin",
        client_data_without_an_origin,
    ),
    (
        "a credential that is no object at all",
        a_credential_that_is_no_object,
    ),
];

/// Every `Credential` this service cannot read as a registration response is
/// refused with the same sentence, which names no member of what was presented.
#[tokio::test]
async fn every_unreadable_credential_is_reported_with_the_same_sentence() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "unreadable", &PASSKEY_FACTORS, &["cara"]).await;
    let token = access_token(&router, &fixture.client_id, "cara", GOOD_PASSWORD).await;

    for (case, malform) in MALFORMED {
        // The registration is live and the challenge is the one it handed out,
        // so what the row malforms is the ONLY thing wrong with the response.
        let challenge = start_challenge(&router, &token, case).await;
        let (status, payload) = complete(&router, &token, &malform(&challenge)).await;

        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {case:?}: the response is refused as unreadable, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            INVALID_CREDENTIAL_DATA_MESSAGE,
            "case {case:?}: the one sentence for a credential this service cannot read, got {:?}",
            error_message(&payload)
        );
        let stored = stored_ids(&engine, &fixture.user_pool_id, "cara");
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused response registers no passkey, got {stored:?}"
        );
    }
}

/// A pool that does not admit the passkey sign-in factor reports the feature is
/// not enabled in the same sentence from both registration operations: the one
/// that hands a challenge out, and the one that answers it.
#[tokio::test]
async fn a_pool_without_the_passkey_factor_reports_the_same_sentence_everywhere() {
    let Emulator { engine, router } = emulator();

    // The pool that never admitted the factor: the operation that would mint a
    // challenge has nothing to hand out.
    let case = "a pool that never admitted the passkey factor";
    let password_only = fixture(&router, "no-passkeys", &PASSWORD_ONLY_FACTORS, &["dina"]).await;
    let dinas_token = access_token(&router, &password_only.client_id, "dina", GOOD_PASSWORD).await;
    let (status, payload) = start(&router, &dinas_token).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "WebAuthnNotEnabledException"),
        "case {case:?}: StartWebAuthnRegistration is refused, got {status} with {payload}"
    );
    assert_eq!(
        error_message(&payload),
        WEB_AUTHN_NOT_ENABLED_MESSAGE,
        "case {case:?}: the one sentence for a pool without the factor, got {:?}",
        error_message(&payload)
    );

    // The pool that dropped the factor while a registration was in flight: the
    // challenge is answerable, the feature is not enabled, and the response
    // registers nothing.
    let case = "a pool that dropped the passkey factor while a registration was in flight";
    let passkeys = fixture(&router, "dropped", &PASSKEY_FACTORS, &["elia"]).await;
    let elias_token = access_token(&router, &passkeys.client_id, "elia", GOOD_PASSWORD).await;
    let challenge = start_challenge(&router, &elias_token, case).await;
    ok(
        &router,
        "UpdateUserPool",
        &json!({
            "UserPoolId": passkeys.user_pool_id,
            "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": PASSWORD_ONLY_FACTORS } },
        }),
    )
    .await;
    let (status, payload) = complete(
        &router,
        &elias_token,
        &credential_document(CREDENTIAL_ID, &challenge),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "WebAuthnNotEnabledException"),
        "case {case:?}: CompleteWebAuthnRegistration is refused, got {status} with {payload}"
    );
    assert_eq!(
        error_message(&payload),
        WEB_AUTHN_NOT_ENABLED_MESSAGE,
        "case {case:?}: the one sentence for a pool without the factor, got {:?}",
        error_message(&payload)
    );
    let stored = stored_ids(&engine, &passkeys.user_pool_id, "elia");
    assert!(
        stored.is_empty(),
        "case {case:?}: a refused response registers no passkey, got {stored:?}"
    );
}

/// Every `CredentialId` the signed-in user holds no passkey under is reported
/// with the same sentence — the id of ANOTHER user's passkey included, so the
/// refusal says nothing about who holds that credential.
#[tokio::test]
async fn every_credential_id_of_no_passkey_of_this_user_reports_the_same_sentence() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "not-found", &PASSKEY_FACTORS, &["fay", "gil"]).await;
    let fays_token = access_token(&router, &fixture.client_id, "fay", GOOD_PASSWORD).await;
    let gils_token = access_token(&router, &fixture.client_id, "gil", GOOD_PASSWORD).await;
    let kept = "cmVmdXNhbC13b3JkaW5nLWtlcHQ";
    register(&router, &fays_token, CREDENTIAL_ID, "fay's first passkey").await;
    register(&router, &fays_token, kept, "fay's second passkey").await;
    register(&router, &gils_token, "Z2lscy1wYXNza2V5", "gil's passkey").await;

    // Deleting one of fay's passkeys first, so the SAME id is then an id she
    // holds no passkey under.
    let (status, payload) = delete(&router, &fays_token, CREDENTIAL_ID).await;
    assert_eq!(
        status, 200,
        "deleting fay's first passkey must succeed, got {status} with {payload}"
    );

    for (case, credential_id) in [
        ("a credential this user already deleted", CREDENTIAL_ID),
        (
            "a credential id nothing ever registered",
            "bmV2ZXItcmVnaXN0ZXJlZA",
        ),
        ("the empty credential id, the modelled minimum", ""),
        ("another user's credential id", "Z2lscy1wYXNza2V5"),
    ] {
        let (status, payload) = delete(&router, &fays_token, credential_id).await;
        // Every refusal of the caller this endpoint answers carries HTTP 400 and
        // names its error class in the body, whatever status the error shape
        // declares.
        assert_eq!(
            (status, error_code(&payload)),
            (400, "ResourceNotFoundException"),
            "case {case:?}: the deletion is refused, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            CREDENTIAL_NOT_FOUND_MESSAGE,
            "case {case:?}: the one sentence for a credential this user does not hold, got {:?}",
            error_message(&payload)
        );
    }

    // Every refused deletion deleted nothing: fay keeps her remaining passkey
    // and gil keeps his.
    let case = "the state after every refused deletion";
    let fays = stored_ids(&engine, &fixture.user_pool_id, "fay");
    assert_eq!(
        fays,
        vec![kept.to_string()],
        "case {case:?}: the signed-in user keeps the passkeys it holds, got {fays:?}"
    );
    let gils = stored_ids(&engine, &fixture.user_pool_id, "gil");
    assert_eq!(
        gils,
        vec!["Z2lscy1wYXNza2V5".to_string()],
        "case {case:?}: another user's passkey is untouched, got {gils:?}"
    );
}

/// Every self-service operation a signed-in user's access token authorizes,
/// one per row, with the body it is called with. The rows differ along the
/// operation and so along the work the refusal comes BEFORE: a listing, a
/// registration, a deletion and a read of the user itself.
fn self_service_rows(token: &str, challenge: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "ListWebAuthnCredentials",
            json!({ "AccessToken": token, "MaxResults": 20 }),
        ),
        ("StartWebAuthnRegistration", json!({ "AccessToken": token })),
        (
            "CompleteWebAuthnRegistration",
            json!({
                "AccessToken": token,
                "Credential": credential_document("cGVuZGluZy1jcmVkZW50aWFs", challenge),
            }),
        ),
        (
            "DeleteWebAuthnCredential",
            json!({ "AccessToken": token, "CredentialId": CREDENTIAL_ID }),
        ),
        ("GetUser", json!({ "AccessToken": token })),
        ("GetUserAuthFactors", json!({ "AccessToken": token })),
        ("ListDevices", json!({ "AccessToken": token })),
    ]
}

/// The credential ids `ListWebAuthnCredentials` reports for the user `token`
/// names, requiring success.
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
        .unwrap_or_else(|| panic!("case {case:?}: the response carries a Credentials list"))
        .iter()
        .map(|entry| {
            entry["CredentialId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an entry names a credential"))
                .to_string()
        })
        .collect()
}

/// A user Amazon Cognito is waiting on a new password from is served by NO
/// self-service operation its live access token authorizes, each reporting the
/// one sentence for that state — and the same token is served again once the
/// password is set, so the precondition is read from the stored user and not
/// from the token.
#[tokio::test]
async fn a_user_owing_a_new_password_is_served_by_no_self_service_operation() {
    let Emulator { engine, router } = emulator();
    let fixture = fixture(&router, "reset-required", &PASSKEY_FACTORS, &["hana"]).await;
    let token = access_token(&router, &fixture.client_id, "hana", GOOD_PASSWORD).await;

    // Before the reset the token serves the whole surface: one passkey is
    // registered and listed, and a second registration is left in flight so the
    // completing call below has a challenge to answer.
    let case = "before the reset";
    register(&router, &token, CREDENTIAL_ID, case).await;
    let listed = listed_ids(&router, &token, case).await;
    assert_eq!(
        listed,
        vec![CREDENTIAL_ID.to_string()],
        "case {case:?}: the listing reports the registered passkey, got {listed:?}"
    );
    let challenge = start_challenge(&router, &token, case).await;

    // "Sets the requested user's account into a RESET_REQUIRED status"
    // (`AdminResetUserPassword` in this service's model). The token the user
    // already holds is untouched by it: it is still live and still carries the
    // self-service scope.
    ok(
        &router,
        "AdminResetUserPassword",
        &json!({ "UserPoolId": fixture.user_pool_id, "Username": "hana" }),
    )
    .await;

    for (operation, body) in self_service_rows(&token, &challenge) {
        let (status, payload) = dispatch(&router, operation, &body, false).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "PasswordResetRequiredException"),
            "case {operation:?}: a user owing a new password is refused, got {status} with \
             {payload}"
        );
        assert_eq!(
            error_message(&payload),
            PASSWORD_RESET_REQUIRED_MESSAGE,
            "case {operation:?}: the one sentence for that state, got {:?}",
            error_message(&payload)
        );
    }

    // Every refusal above did the work of no operation: the passkey the deletion
    // named is still held, and the response the registration presented
    // registered nothing.
    let case = "the state while the reset is outstanding";
    let stored = stored_ids(&engine, &fixture.user_pool_id, "hana");
    assert_eq!(
        stored,
        vec![CREDENTIAL_ID.to_string()],
        "case {case:?}: no refused call changed what the user holds, got {stored:?}"
    );

    // Setting a password settles the user back into `CONFIRMED`, and the SAME
    // token — never reissued — is served again.
    let case = "after the password is set";
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": fixture.user_pool_id,
            "Username": "hana",
            "Password": NEXT_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let listed = listed_ids(&router, &token, case).await;
    assert_eq!(
        listed,
        vec![CREDENTIAL_ID.to_string()],
        "case {case:?}: the listing reports the passkey again, got {listed:?}"
    );
}
