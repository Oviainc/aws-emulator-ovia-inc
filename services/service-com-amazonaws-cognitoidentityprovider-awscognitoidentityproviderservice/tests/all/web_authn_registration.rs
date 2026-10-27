// SPDX-License-Identifier: CC0-1.0

//! `StartWebAuthnRegistration` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target:
//! AWSCognitoIdentityProviderService.StartWebAuthnRegistration`).
//!
//! The rule under test: a self-service call whose only subject is the
//! `AccessToken` starts a passkey registration for the user that access token was
//! issued to, and ANSWERS credential creation options computed from stored state —
//! the relying party in force for the pool (its configured relying party id, else
//! its own authentication domain), that pool's user-verification requirement, the
//! stored user's name, display name and immutable id, and the passkeys the user
//! has already registered as `excludeCredentials` — beside a FRESH challenge it
//! stores, so a registration started twice is two registrations and each one is
//! answerable on its own. A pool that does not admit the passkey sign-in factor is
//! refused with 400 `WebAuthnNotEnabledException`, one with neither a relying
//! party id nor a domain with 400 `WebAuthnConfigurationMissingException`, a user
//! already holding every passkey one user may hold with 400
//! `LimitExceededException`, and a token that names no signed-in user — one this
//! emulator never issued, one that is not an ACCESS token, a revoked one, one the
//! clock has expired, and one whose pool or user is gone — with 403
//! `NotAuthorizedException`.
//!
//! A passkey credential comes into existence through
//! `CompleteWebAuthnRegistration`, which is still a scaffold in this emulator, so
//! the rows that need a user with passkeys already registered PLANT the credential
//! state that call will write, the way the other suites of this crate plant state
//! for writers that are not implemented yet. The pools, app clients, users,
//! domains, passkey configurations and access tokens are all real: they come from
//! `CreateUserPool`, `CreateUserPoolClient`, `CreateUserPoolDomain`,
//! `SetUserPoolMfaConfig`, `AdminCreateUser` and `InitiateAuth`.
//!
//! Each table row names the case it asserts and every assertion reports the actual
//! value.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// A request's SigV4 credential scope is what routes an administrative call to
/// this service, and its Region is the one a minted pool belongs to. The
/// signature is never checked here: the suite turns request authentication off.
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

/// An ACM certificate ARN in the Region `CreateUserPoolDomain` requires one from,
/// so a custom domain can be created.
const CERTIFICATE_ARN: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/11111111-2222-3333-4444-555555555555";

/// `AllowedFirstAuthFactors` of a pool that offers passkey sign-in
/// (`smithy_query(query="shape",shape="SignInPolicyType")`, whose values are
/// `AuthFactorType`'s).
const PASSKEY_FACTORS: [&str; 2] = ["PASSWORD", "WEB_AUTHN"];

/// The `timeout` the creation options advertise, in milliseconds, which is also
/// how long the registration is held
/// (`state::WEB_AUTHN_REGISTRATION_TIMEOUT`).
const TIMEOUT_MILLIS: u64 = 300_000;

/// One emulator with just this service mounted, and the engine its state lives
/// in, so a suite can plant the state a writer that is still a scaffold will
/// write.
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
/// address, which is how a signed-in user's client calls
/// `StartWebAuthnRegistration` — the model marks the operation `@optionalAuth`.
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

/// Create a pool named `name` whose sign-in policy admits `factors`, or, when
/// `factors` is empty, that configures no sign-in policy at all. Returns its
/// minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str, factors: &[&str]) -> String {
    let mut request = Map::new();
    request.insert("PoolName".to_string(), json!(name));
    if !factors.is_empty() {
        request.insert(
            "Policies".to_string(),
            json!({ "SignInPolicy": { "AllowedFirstAuthFactors": factors } }),
        );
    }
    let created = ok(router, "CreateUserPool", &Value::Object(request)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client that allows the password sign-in this suite uses, and
/// return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "passkey-client",
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
/// can sign in, carrying `attributes`.
async fn confirmed_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) {
    let mut request = Map::new();
    request.insert("UserPoolId".to_string(), json!(user_pool_id));
    request.insert("Username".to_string(), json!(username));
    if !attributes.is_empty() {
        request.insert(
            "UserAttributes".to_string(),
            Value::Array(
                attributes
                    .iter()
                    .map(|(name, value)| json!({ "Name": name, "Value": value }))
                    .collect(),
            ),
        );
    }
    ok(router, "AdminCreateUser", &Value::Object(request)).await;
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

/// Set the pool's passkey configuration through `SetUserPoolMfaConfig`.
async fn set_web_authn_configuration(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    relying_party_id: Option<&str>,
    user_verification: Option<&str>,
) {
    let mut configuration = Map::new();
    if let Some(relying_party_id) = relying_party_id {
        configuration.insert("RelyingPartyId".to_string(), json!(relying_party_id));
    }
    if let Some(user_verification) = user_verification {
        configuration.insert("UserVerification".to_string(), json!(user_verification));
    }
    ok(
        router,
        "SetUserPoolMfaConfig",
        &json!({
            "UserPoolId": user_pool_id,
            "WebAuthnConfiguration": Value::Object(configuration),
        }),
    )
    .await;
}

/// Create a prefix or custom domain for the pool.
async fn domain(router: &MultiServiceRouter, user_pool_id: &str, domain: &str, custom: bool) {
    let mut request = Map::new();
    request.insert("UserPoolId".to_string(), json!(user_pool_id));
    request.insert("Domain".to_string(), json!(domain));
    if custom {
        request.insert(
            "CustomDomainConfig".to_string(),
            json!({ "CertificateArn": CERTIFICATE_ARN }),
        );
    }
    ok(router, "CreateUserPoolDomain", &Value::Object(request)).await;
}

/// Sign `username` in through an unsigned `InitiateAuth` and return the
/// `AuthenticationResult` it answered.
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

/// Call `StartWebAuthnRegistration` unsigned and return its status and body.
async fn start(router: &MultiServiceRouter, token: &str) -> (u16, Value) {
    dispatch(
        router,
        "StartWebAuthnRegistration",
        &json!({ "AccessToken": token }),
        false,
    )
    .await
}

/// Call `StartWebAuthnRegistration` unsigned, requiring success, and return the
/// `CredentialCreationOptions` it answered.
async fn start_ok(router: &MultiServiceRouter, token: &str, case: &str) -> Value {
    let (status, payload) = start(router, token).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartWebAuthnRegistration must succeed, got {status} with {payload}"
    );
    payload["CredentialCreationOptions"].clone()
}

/// The stored `sub` of `username`, which is the user handle the options carry
/// base64url-encoded.
fn stored_sub(engine: &Arc<engine::InMemoryEngine>, user_pool_id: &str, username: &str) -> String {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard
        .pool(user_pool_id)
        .expect("the created pool")
        .user(username)
        .expect("the created user")
        .sub
        .clone()
}

/// Every passkey registration the store holds, keyed by its challenge.
fn stored_registrations(
    engine: &Arc<engine::InMemoryEngine>,
) -> std::collections::HashMap<String, state::WebAuthnRegistration> {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let guard = service_state.lock();
    guard.web_authn_registrations.clone()
}

/// One passkey credential to plant, standing for one a completed registration
/// stored. The rows that use these differ along the COUNT of credentials and the
/// transports each reports.
#[derive(Clone, Copy, Debug)]
struct PlantedCredential {
    credential_id: &'static str,
    /// `WebAuthnAuthenticatorTransportType` values, as
    /// `WebAuthnCredentialDescription.AuthenticatorTransports` carries them.
    transports: &'static [&'static str],
}

/// Plant `credentials` on the stored user, in the order given, which is the
/// registration order a completed registration would have appended them in.
fn plant_credentials(
    engine: &Arc<engine::InMemoryEngine>,
    user_pool_id: &str,
    username: &str,
    credentials: &[PlantedCredential],
) {
    let service_state = state::service_state(&**engine).expect("this service's state");
    let mut guard = service_state.lock();
    let pool = guard.pool_mut(user_pool_id).expect("the created pool");
    let relying_party_id = pool.relying_party_id().unwrap_or_default();
    let user = pool.user_mut(username).expect("the created user");
    for (ordinal, credential) in credentials.iter().enumerate() {
        user.web_authn_credentials.push(state::WebAuthnCredential {
            credential_id: credential.credential_id.to_string(),
            friendly_credential_name: format!("planted passkey {ordinal}"),
            relying_party_id: relying_party_id.clone(),
            authenticator_attachment: None,
            authenticator_transports: credential
                .transports
                .iter()
                .map(|transport| transport.to_string())
                .collect(),
            created_at: SystemTime::now(),
        });
    }
}

/// The `pubKeyCredParams` every registration offers: ES256 (`-7`) then RS256
/// (`-257`), the COSE algorithm identifiers of the IANA "COSE Algorithms"
/// registry.
fn expected_algorithms() -> Value {
    json!([
        { "type": "public-key", "alg": -7 },
        { "type": "public-key", "alg": -257 },
    ])
}

/// Assert the parts of `options` every successful registration carries, whatever
/// the pool and user are.
fn assert_invariant_options(case: &str, options: &Value) {
    assert_eq!(
        options["pubKeyCredParams"],
        expected_algorithms(),
        "case {case:?}: the offered algorithms, got {}",
        options["pubKeyCredParams"]
    );
    assert_eq!(
        options["timeout"].as_u64(),
        Some(TIMEOUT_MILLIS),
        "case {case:?}: the advertised timeout, got {}",
        options["timeout"]
    );
    assert_eq!(
        options["attestation"].as_str(),
        Some("none"),
        "case {case:?}: the attestation preference, got {}",
        options["attestation"]
    );
    assert_eq!(
        options["authenticatorSelection"]["residentKey"].as_str(),
        Some("required"),
        "case {case:?}: a passkey is a discoverable credential, got {}",
        options["authenticatorSelection"]["residentKey"]
    );
    assert_eq!(
        options["authenticatorSelection"]["requireResidentKey"].as_bool(),
        Some(true),
        "case {case:?}: the deprecated resident-key companion, got {}",
        options["authenticatorSelection"]["requireResidentKey"]
    );
    let challenge = options["challenge"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a challenge, got {options}"));
    assert!(
        !challenge.is_empty()
            && challenge
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "case {case:?}: the challenge is base64url, got {challenge:?}"
    );
}

/// One row of the creation-options table. The rows differ along where the relying
/// party in force comes from, what user verification the pool requires and
/// whether the user carries a display name of its own.
#[derive(Clone, Copy, Debug)]
struct OptionsCase {
    case: &'static str,
    /// The pool's `WebAuthnConfiguration`, when it sets one: its
    /// `RelyingPartyId` and its `UserVerification`.
    configuration: Option<(Option<&'static str>, Option<&'static str>)>,
    /// The pool's domains, each with whether it is a CUSTOM domain: a pool may
    /// hold a prefix domain and a custom domain at once.
    domains: &'static [(&'static str, bool)],
    /// The `name` attribute of the user, when it carries one.
    name_attribute: Option<&'static str>,
    expected_relying_party_id: &'static str,
    expected_user_verification: &'static str,
    expected_display_name: &'static str,
}

#[tokio::test]
async fn creation_options_are_rendered_from_the_stored_pool_and_user() {
    // The rows span every source of the relying party in force, both
    // `UserVerificationType` values plus the pool that sets none, and a user
    // with and without a display name of its own.
    let cases = [
        OptionsCase {
            case: "a configured relying party id and user verification required",
            configuration: Some((Some("passkeys.example.com"), Some("required"))),
            domains: &[],
            name_attribute: None,
            expected_relying_party_id: "passkeys.example.com",
            expected_user_verification: "required",
            expected_display_name: "ada",
        },
        OptionsCase {
            case: "a configured relying party id and no user-verification requirement",
            configuration: Some((Some("passkeys.example.net"), None)),
            domains: &[],
            name_attribute: Some("Ada Lovelace"),
            expected_relying_party_id: "passkeys.example.net",
            expected_user_verification: "preferred",
            expected_display_name: "Ada Lovelace",
        },
        OptionsCase {
            case: "no passkey configuration and a prefix domain",
            configuration: None,
            domains: &[("passkey-prefix", false)],
            name_attribute: None,
            expected_relying_party_id: "passkey-prefix.auth.us-east-1.amazoncognito.com",
            expected_user_verification: "preferred",
            expected_display_name: "ada",
        },
        OptionsCase {
            case: "no passkey configuration and a custom domain",
            configuration: None,
            domains: &[("passkey-custom", true)],
            name_attribute: Some("Grace Hopper"),
            expected_relying_party_id: "passkey-custom",
            expected_user_verification: "preferred",
            expected_display_name: "Grace Hopper",
        },
        OptionsCase {
            // "When you create a custom domain, the passkey RP ID defaults to
            // the custom domain. If you had a prefix domain active, this will
            // cause passkey integration for your prefix domain to stop working
            // due to a mismatch in RP ID"
            // (`CreateUserPoolDomain.CustomDomainConfig`).
            case: "a custom domain beside a prefix domain",
            configuration: None,
            domains: &[("shadowed-prefix", false), ("preferred-custom", true)],
            name_attribute: None,
            expected_relying_party_id: "preferred-custom",
            expected_user_verification: "preferred",
            expected_display_name: "ada",
        },
        OptionsCase {
            case: "a configured relying party id beside a prefix domain",
            configuration: Some((Some("chosen.example.com"), Some("preferred"))),
            domains: &[("ignored-prefix", false)],
            name_attribute: None,
            expected_relying_party_id: "chosen.example.com",
            expected_user_verification: "preferred",
            expected_display_name: "ada",
        },
        OptionsCase {
            case: "user verification required with the domain as the relying party",
            configuration: Some((None, Some("required"))),
            domains: &[("verified-prefix", false)],
            name_attribute: None,
            expected_relying_party_id: "verified-prefix.auth.us-east-1.amazoncognito.com",
            expected_user_verification: "required",
            expected_display_name: "ada",
        },
    ];

    for case in cases {
        let Emulator { engine, router } = emulator();
        let pool_name = format!("passkey-pool-{}", case.expected_user_verification);
        let user_pool_id = pool(&router, &pool_name, &PASSKEY_FACTORS).await;
        if let Some((relying_party_id, user_verification)) = case.configuration {
            set_web_authn_configuration(
                &router,
                &user_pool_id,
                relying_party_id,
                user_verification,
            )
            .await;
        }
        for (name, custom) in case.domains {
            domain(&router, &user_pool_id, name, *custom).await;
        }
        let client_id = client(&router, &user_pool_id).await;
        let attributes = case
            .name_attribute
            .map(|name| vec![("name", name)])
            .unwrap_or_default();
        confirmed_user(&router, &user_pool_id, "ada", &attributes).await;
        let token = access_token(&router, &client_id, "ada").await;

        let options = start_ok(&router, &token, case.case).await;
        assert_invariant_options(case.case, &options);

        assert_eq!(
            options["rp"]["id"].as_str(),
            Some(case.expected_relying_party_id),
            "case {:?}: the relying party in force, got {}",
            case.case,
            options["rp"]["id"]
        );
        assert_eq!(
            options["rp"]["name"].as_str(),
            Some(pool_name.as_str()),
            "case {:?}: the relying party name is the pool's, got {}",
            case.case,
            options["rp"]["name"]
        );
        assert_eq!(
            options["authenticatorSelection"]["userVerification"].as_str(),
            Some(case.expected_user_verification),
            "case {:?}: the pool's user-verification requirement, got {}",
            case.case,
            options["authenticatorSelection"]["userVerification"]
        );
        assert_eq!(
            options["user"]["name"].as_str(),
            Some("ada"),
            "case {:?}: the stored username, got {}",
            case.case,
            options["user"]["name"]
        );
        assert_eq!(
            options["user"]["displayName"].as_str(),
            Some(case.expected_display_name),
            "case {:?}: the stored display name, got {}",
            case.case,
            options["user"]["displayName"]
        );
        let sub = stored_sub(&engine, &user_pool_id, "ada");
        assert_eq!(
            options["user"]["id"].as_str(),
            Some(state::base64_url(sub.as_bytes()).as_str()),
            "case {:?}: the user handle is the stored sub, got {}",
            case.case,
            options["user"]["id"]
        );
        assert_eq!(
            options["excludeCredentials"],
            json!([]),
            "case {:?}: a user with no passkey excludes none, got {}",
            case.case,
            options["excludeCredentials"]
        );

        // The challenge the caller was handed is the one the store holds, bound
        // to the pool, the user, the app client of the access token and the
        // relying party in force.
        let challenge = options["challenge"].as_str().expect("a challenge");
        let registrations = stored_registrations(&engine);
        let stored = registrations.get(challenge).unwrap_or_else(|| {
            panic!(
                "case {:?}: the challenge must be stored, got {:?}",
                case.case,
                registrations.keys().collect::<Vec<_>>()
            )
        });
        assert_eq!(
            (
                stored.user_pool_id.as_str(),
                stored.username.as_str(),
                stored.client_id.as_str(),
                stored.relying_party_id.as_str(),
                stored.user_verification.as_str(),
            ),
            (
                user_pool_id.as_str(),
                "ada",
                client_id.as_str(),
                case.expected_relying_party_id,
                case.expected_user_verification,
            ),
            "case {:?}: the stored registration, got {stored:?}",
            case.case
        );
        assert_eq!(
            stored.expires_at,
            stored.created_at + Duration::from_millis(TIMEOUT_MILLIS),
            "case {:?}: the registration is held for the advertised timeout, got {stored:?}",
            case.case
        );
    }
}

#[tokio::test]
async fn every_registration_mints_its_own_challenge() {
    let Emulator { engine, router } = emulator();
    let user_pool_id = pool(&router, "repeat-pool", &PASSKEY_FACTORS).await;
    set_web_authn_configuration(&router, &user_pool_id, Some("repeat.example.com"), None).await;
    let client_id = client(&router, &user_pool_id).await;
    confirmed_user(&router, &user_pool_id, "ada", &[]).await;
    confirmed_user(&router, &user_pool_id, "grace", &[]).await;
    let ada = access_token(&router, &client_id, "ada").await;
    let grace = access_token(&router, &client_id, "grace").await;

    // Three registrations: two for one user and one for another.
    let mut challenges = Vec::new();
    for (case, token) in [
        ("ada's first registration", &ada),
        ("ada's second registration", &ada),
        ("grace's registration", &grace),
    ] {
        let options = start_ok(&router, token, case).await;
        challenges.push((
            case,
            options["challenge"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a challenge, got {options}"))
                .to_string(),
        ));
    }

    let registrations = stored_registrations(&engine);
    for (case, challenge) in &challenges {
        assert_eq!(
            challenges
                .iter()
                .filter(|(_, other)| other == challenge)
                .count(),
            1,
            "case {case:?}: the challenge is fresh, got {challenge:?} among {challenges:?}"
        );
        // A later registration leaves an earlier one answerable: each carries
        // its own challenge.
        assert!(
            registrations.contains_key(challenge),
            "case {case:?}: the challenge is still stored, got {:?}",
            registrations.keys().collect::<Vec<_>>()
        );
    }
    assert_eq!(
        registrations.len(),
        challenges.len(),
        "three registrations are three stored challenges, got {}",
        registrations.len()
    );
}

#[tokio::test]
async fn already_registered_passkeys_are_excluded() {
    // The rows differ along the COUNT of passkeys the user already holds and the
    // transports each one reports.
    const INTERNAL: PlantedCredential = PlantedCredential {
        credential_id: "Y3JlZGVudGlhbC1vbmU",
        transports: &["internal", "hybrid"],
    };
    const USB: PlantedCredential = PlantedCredential {
        credential_id: "Y3JlZGVudGlhbC10d28",
        transports: &["usb"],
    };
    const NO_TRANSPORT: PlantedCredential = PlantedCredential {
        credential_id: "Y3JlZGVudGlhbC10aHJlZQ",
        transports: &[],
    };
    let cases: [(&str, &[PlantedCredential]); 4] = [
        ("no passkey registered yet", &[]),
        ("one registered passkey", &[INTERNAL]),
        ("two registered passkeys", &[INTERNAL, USB]),
        (
            "three registered passkeys, one reporting no transport",
            &[USB, NO_TRANSPORT, INTERNAL],
        ),
    ];

    for (case, planted) in cases {
        let Emulator { engine, router } = emulator();
        let user_pool_id = pool(&router, "exclude-pool", &PASSKEY_FACTORS).await;
        set_web_authn_configuration(&router, &user_pool_id, Some("exclude.example.com"), None).await;
        let client_id = client(&router, &user_pool_id).await;
        confirmed_user(&router, &user_pool_id, "ada", &[]).await;
        plant_credentials(&engine, &user_pool_id, "ada", planted);
        let token = access_token(&router, &client_id, "ada").await;

        let options = start_ok(&router, &token, case).await;
        assert_invariant_options(case, &options);
        let expected = Value::Array(
            planted
                .iter()
                .map(|credential| {
                    json!({
                        "type": "public-key",
                        "id": credential.credential_id,
                        "transports": credential.transports,
                    })
                })
                .collect(),
        );
        assert_eq!(
            options["excludeCredentials"], expected,
            "case {case:?}: the passkeys already registered, in registration order, got {}",
            options["excludeCredentials"]
        );
    }
}

#[tokio::test]
async fn a_user_holding_every_passkey_it_may_hold_is_refused() {
    // One row below the bound, one at it: the bound itself is
    // `state::MAX_WEB_AUTHN_CREDENTIALS_PER_USER`.
    let bound = state::MAX_WEB_AUTHN_CREDENTIALS_PER_USER;
    for (case, held) in [
        ("one passkey short of the bound", bound - 1),
        ("every passkey the user may hold", bound),
        ("more passkeys than the bound", bound + 1),
    ] {
        let Emulator { engine, router } = emulator();
        let user_pool_id = pool(&router, "limit-pool", &PASSKEY_FACTORS).await;
        set_web_authn_configuration(&router, &user_pool_id, Some("limit.example.com"), None).await;
        let client_id = client(&router, &user_pool_id).await;
        confirmed_user(&router, &user_pool_id, "ada", &[]).await;
        {
            // Planted one at a time, so the count is the only thing that varies.
            let service_state = state::service_state(&*engine).expect("this service's state");
            let mut guard = service_state.lock();
            let pool = guard.pool_mut(&user_pool_id).expect("the created pool");
            let user = pool.user_mut("ada").expect("the created user");
            for ordinal in 0..held {
                user.web_authn_credentials.push(state::WebAuthnCredential {
                    credential_id: format!("held-credential-{ordinal}"),
                    friendly_credential_name: format!("planted passkey {ordinal}"),
                    relying_party_id: "limit.example.com".to_string(),
                    authenticator_attachment: None,
                    authenticator_transports: vec!["internal".to_string()],
                    created_at: SystemTime::now(),
                });
            }
        }
        let token = access_token(&router, &client_id, "ada").await;

        let (status, payload) = start(&router, &token).await;
        if held < bound {
            assert_eq!(
                status, 200,
                "case {case:?}: a user below the bound registers, got {status} with {payload}"
            );
            assert_eq!(
                payload["CredentialCreationOptions"]["excludeCredentials"]
                    .as_array()
                    .map(Vec::len),
                Some(held),
                "case {case:?}: every held passkey is excluded, got {}",
                payload["CredentialCreationOptions"]["excludeCredentials"]
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&payload)),
            (400, "LimitExceededException"),
            "case {case:?}: a user at the bound is refused, got {status} with {payload}"
        );
        assert!(
            error_message(&payload).contains(&bound.to_string())
                && error_message(&payload).contains("ada"),
            "case {case:?}: the refusal names the user and the bound, got {:?}",
            error_message(&payload)
        );
        assert!(
            stored_registrations(&engine).is_empty(),
            "case {case:?}: a refused registration stores nothing, got {:?}",
            stored_registrations(&engine).keys().collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn a_pool_that_does_not_offer_passkeys_is_refused() {
    // The rows span a pool that configures no sign-in policy at all and one that
    // configures first factors without the passkey one.
    for (case, factors) in [
        ("a pool with no sign-in policy", &[] as &[&str]),
        ("a pool whose first factors omit the passkey one", &[
            "PASSWORD",
            "EMAIL_OTP",
        ]),
    ] {
        let Emulator { engine, router } = emulator();
        let user_pool_id = pool(&router, "no-passkey-pool", factors).await;
        // Configured relying party and domain both in place, so the only thing
        // missing is the factor itself.
        set_web_authn_configuration(&router, &user_pool_id, Some("absent.example.com"), None).await;
        domain(&router, &user_pool_id, "absent-prefix", false).await;
        let client_id = client(&router, &user_pool_id).await;
        confirmed_user(&router, &user_pool_id, "ada", &[]).await;
        let token = access_token(&router, &client_id, "ada").await;

        let (status, payload) = start(&router, &token).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "WebAuthnNotEnabledException"),
            "case {case:?}: the passkey feature is not enabled, got {status} with {payload}"
        );
        assert!(
            stored_registrations(&engine).is_empty(),
            "case {case:?}: a refused registration stores nothing, got {:?}",
            stored_registrations(&engine).keys().collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn a_pool_with_no_relying_party_is_refused() {
    // The rows span a pool with no passkey configuration at all and one whose
    // configuration names no relying party id; neither has a domain to serve as
    // one.
    for (case, configure) in [
        ("no passkey configuration and no domain", false),
        ("a passkey configuration naming no relying party id", true),
    ] {
        let Emulator { engine, router } = emulator();
        let user_pool_id = pool(&router, "no-relying-party-pool", &PASSKEY_FACTORS).await;
        if configure {
            set_web_authn_configuration(&router, &user_pool_id, None, Some("required")).await;
        }
        let client_id = client(&router, &user_pool_id).await;
        confirmed_user(&router, &user_pool_id, "ada", &[]).await;
        let token = access_token(&router, &client_id, "ada").await;

        let (status, payload) = start(&router, &token).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "WebAuthnConfigurationMissingException"),
            "case {case:?}: no relying party is in force, got {status} with {payload}"
        );
        assert!(
            stored_registrations(&engine).is_empty(),
            "case {case:?}: a refused registration stores nothing, got {:?}",
            stored_registrations(&engine).keys().collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn a_token_that_names_no_signed_in_user_is_refused() {
    let Emulator { engine, router } = emulator();
    let user_pool_id = pool(&router, "token-pool", &PASSKEY_FACTORS).await;
    set_web_authn_configuration(&router, &user_pool_id, Some("token.example.com"), None).await;
    let client_id = client(&router, &user_pool_id).await;
    confirmed_user(&router, &user_pool_id, "ada", &[]).await;
    confirmed_user(&router, &user_pool_id, "grace", &[]).await;

    let result = sign_in(&router, &client_id, "ada").await;
    let id_token = result["IdToken"].as_str().expect("an id token").to_string();
    let refresh_token = result["RefreshToken"]
        .as_str()
        .expect("a refresh token")
        .to_string();
    // A second sign-in, whose access token this test revokes, so revoking it
    // does not disturb the token the rest of the table uses.
    let revoked = access_token(&router, &client_id, "grace").await;

    // Three token records planted directly: an access token the clock has
    // already expired, one naming a pool this emulator does not hold, and one
    // naming a user its pool does not hold. None can be reached by signing in,
    // and each is a rejection the token lookup tells apart.
    const EXPIRED: &str = "expired.access.token";
    const POOL_GONE: &str = "pool-gone.access.token";
    const USER_GONE: &str = "user-gone.access.token";
    {
        let service_state = state::service_state(&*engine).expect("this service's state");
        let mut guard = service_state.lock();
        guard
            .issued_tokens
            .get_mut(&revoked)
            .expect("the recorded access token")
            .revoked = true;
        let now = SystemTime::now();
        // The id of the stored user a planted record names, so a record naming a
        // user this pool HOLDS names that user's id too; one naming a user the
        // pool does not hold names an id it does not hold either.
        let ada_sub = guard
            .pool(&user_pool_id)
            .and_then(|pool| pool.user("ada"))
            .expect("the stored user")
            .sub
            .clone();
        let record = |pool: &str, username: &str, sub: &str, expires_at: SystemTime| {
            state::TokenRecord {
                user_pool_id: pool.to_string(),
                client_id: client_id.clone(),
                username: username.to_string(),
                sub: sub.to_string(),
                kind: state::TokenKind::Access,
                scopes: vec![state::USER_ADMIN_SCOPE.to_string()],
                issued_at: now - Duration::from_secs(60),
                expires_at,
                revoked: false,
                rotated_out_at: None,
            }
        };
        let live = now + Duration::from_secs(3_600);
        for (token, planted) in [
            (
                EXPIRED,
                record(
                    &user_pool_id,
                    "ada",
                    &ada_sub,
                    now - Duration::from_secs(1),
                ),
            ),
            (
                POOL_GONE,
                record("us-east-1_absent", "ada", &ada_sub, live),
            ),
            (
                USER_GONE,
                record(&user_pool_id, "nobody", "nobody-has-this-id", live),
            ),
        ] {
            guard.issued_tokens.insert(token.to_string(), planted);
        }
    }

    for (case, token) in [
        ("a token this emulator never issued", "not.a.token"),
        ("the id token of the same sign-in", id_token.as_str()),
        ("the refresh token of the same sign-in", refresh_token.as_str()),
        ("a revoked access token", revoked.as_str()),
        ("an access token the clock has expired", EXPIRED),
        ("an access token whose pool is gone", POOL_GONE),
        ("an access token whose user is gone", USER_GONE),
    ] {
        let (status, payload) = start(&router, token).await;
        assert_eq!(
            (status, error_code(&payload)),
            (403, "NotAuthorizedException"),
            "case {case:?}: the token names no signed-in user, got {status} with {payload}"
        );
    }

    assert!(
        stored_registrations(&engine).is_empty(),
        "a refused registration stores nothing, got {:?}",
        stored_registrations(&engine).keys().collect::<Vec<_>>()
    );
}
