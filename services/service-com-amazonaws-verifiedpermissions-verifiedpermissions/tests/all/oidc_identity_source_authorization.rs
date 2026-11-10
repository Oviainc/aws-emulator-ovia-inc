// SPDX-License-Identifier: CC0-1.0

//! Authorizing with a token an OpenID Connect identity source trusts.
//!
//! The rule under test: a token-based authorization call answers a DECISION computed from
//! the policy store's persisted policies and the presented token's claims, read under the
//! configuration of the identity source that names the token's issuer — for a provider
//! outside this emulator exactly as for one inside it. A signature is checked against the
//! key set the token's ISSUER publishes, and the only key sets readable in this process are
//! the ones issuers the emulator itself hosts publish in it; a provider an
//! `openIdConnectConfiguration` names publishes its key set at a URL outside the emulator,
//! which makes no outbound connection to fetch one, so no key exists here to check such a
//! token's signature against and what decides the call is every rule the token's own value
//! settles: its form (nothing unsecured), the issuer it claims, the member it arrived in,
//! the audience it was issued to and its validity window. None of that is a service fault,
//! so no case here may be answered with a 500.
//!
//! Each row's token is a compact JWS built here, because the provider that would sign one
//! is not in this process; the Amazon Cognito suite
//! ([`crate::is_authorized_with_token`]) is where a really-signed token's signature is
//! really checked.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, error_code, fault_message};
// The `IsAuthorizedWithToken` suite owns this test crate's policy-writing helper and the
// sentence a token whose issuer the store has no identity source for is refused with.
use crate::is_authorized_with_token::{no_identity_source_message, policy};

/// The entity type the identity sources in this suite generate principals as.
pub(crate) const PRINCIPAL_TYPE: &str = "MyCorp::User";

/// The entity type the OIDC group configuration below maps group claims to.
const GROUP_TYPE: &str = "MyCorp::UserGroup";

/// The resource every request in this suite asks about.
pub(crate) const DOCUMENT: &str = "doc-1";

/// The action every request in this suite asks about.
pub(crate) const ACTION: (&str, &str) = ("MyCorp::Action", "read");

/// The refusal a presented token whose audience the identity source accepts none of is
/// answered with: ONE fixed sentence with no variable part, the same one the Amazon Cognito
/// arm of the audience rule earns.
pub(crate) const AUDIENCE_NOT_ACCEPTED: &str = "Token audience claim contains no valid audience";

/// The claim the group-mapping source below reads group membership from, which is the
/// `groupConfiguration.groupClaim` it was created with rather than any fixed claim name.
const GROUP_CLAIM: &str = "teams";

/// The claim the identity-token source below names its principal with — a
/// `principalIdClaim` that is NOT `sub`, so a principal id that followed `sub` regardless
/// of the configuration would be visible.
const USERNAME_CLAIM: &str = "preferred_username";

/// A compact JWS (RFC 7515 §3.1) carrying `claims` under `header`.
///
/// The segments are base64url-encoded through the one codec this tree has for them
/// (`service_common::jwt::encode_base64url`), and the third segment is a signature the
/// emulator holds no key to check: the provider that signed it lives outside this process.
fn token_with_header(header: &Value, claims: &Value) -> String {
    format!(
        "{}.{}.{}",
        service_common::jwt::encode_base64url(header.to_string().as_bytes()),
        service_common::jwt::encode_base64url(claims.to_string().as_bytes()),
        service_common::jwt::encode_base64url(b"an external provider's signature"),
    )
}

/// The token an OIDC provider mints: a JOSE header naming a real signing algorithm and the
/// key it signed with, plus `claims`.
pub(crate) fn token(claims: &Value) -> String {
    token_with_header(
        &json!({ "alg": "RS256", "kid": "oidc-key-1", "typ": "JWT" }),
        claims,
    )
}

/// Whole seconds since the epoch, `offset` from now — the `NumericDate` of RFC 7519 §2 the
/// `exp` and `nbf` claims are expressed in.
fn epoch_seconds(offset: Duration, ahead: bool) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch");
    let seconds = i64::try_from(now.as_secs()).expect("the epoch seconds fit in an i64");
    let offset = i64::try_from(offset.as_secs()).expect("an offset of minutes fits in an i64");
    if ahead {
        seconds + offset
    } else {
        seconds - offset
    }
}

/// An `exp` claim an hour ahead: a token the clock has not run out on.
pub(crate) fn live() -> i64 {
    epoch_seconds(Duration::from_secs(3600), true)
}

/// Create an identity source in `store` for `configuration`, prove the store really
/// persisted it by reading it back through `GetIdentitySource`, and return its id.
pub(crate) async fn oidc_source(
    router: &MultiServiceRouter,
    store: &str,
    configuration: &Value,
) -> String {
    let created = harness::ok(
        router,
        "CreateIdentitySource",
        &json!({
            "policyStoreId": store,
            "principalEntityType": PRINCIPAL_TYPE,
            "configuration": { "openIdConnectConfiguration": configuration },
        }),
    )
    .await;
    let identity_source_id = created["identitySourceId"]
        .as_str()
        .expect("a minted identitySourceId")
        .to_string();
    let read_back = harness::ok(
        router,
        "GetIdentitySource",
        &json!({ "policyStoreId": store, "identitySourceId": identity_source_id }),
    )
    .await;
    assert_eq!(
        read_back["configuration"]["openIdConnectConfiguration"], *configuration,
        "case {configuration}: the source a decision is read under must be the persisted \
         one, got {read_back}"
    );
    identity_source_id
}

/// One `IsAuthorizedWithToken` request against `store`, carrying whatever token members
/// `tokens` names.
pub(crate) fn request(store: &str, tokens: Value) -> Value {
    let mut body = json!({
        "policyStoreId": store,
        "action": { "actionType": ACTION.0, "actionId": ACTION.1 },
        "resource": { "entityType": "MyCorp::Doc", "entityId": DOCUMENT },
    });
    for (member, value) in tokens.as_object().expect("the token members are an object") {
        body[member.clone()] = value.clone();
    }
    body
}

/// What a row expects of the call.
enum Expected {
    /// A 200 carrying this decision for this principal entity id, determined by these
    /// policies.
    Decision {
        decision: &'static str,
        entity_id: String,
        determining: Vec<String>,
    },
    /// A 400 `ValidationException` whose message carries this clause and is worded as the
    /// sentence a token that did not verify earns.
    Refused(&'static str),
    /// A 400 `ValidationException` whose message is EXACTLY this sentence.
    RefusedExactly(&'static str),
    /// A 404 `ResourceNotFoundException`: the call referenced an identity source for this
    /// issuer, and the store holds none.
    NoSourceForIssuer(String),
}

/// Assert `answered` is what `expected` says, for the case named `case`.
fn assert_answer(case: &str, store: &str, status: u16, answered: &Value, expected: &Expected) {
    match expected {
        Expected::Decision {
            decision,
            entity_id,
            determining,
        } => {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a decision, got {status} with {answered}"
            );
            assert_eq!(
                answered["decision"].as_str(),
                Some(*decision),
                "case {case:?}: expected decision {decision}, got {answered}"
            );
            assert_eq!(
                answered["principal"],
                json!({ "entityType": PRINCIPAL_TYPE, "entityId": entity_id }),
                "case {case:?}: expected the principal the token's claims name under the \
                 source's configuration, got {answered}"
            );
            let reported: Vec<String> = answered["determiningPolicies"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item["policyId"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                &reported, determining,
                "case {case:?}: expected the determining policies {determining:?}, got \
                 {answered}"
            );
            assert_eq!(
                answered["errors"],
                json!([]),
                "case {case:?}: a clean evaluation reports no errors, got {answered}"
            );
        }
        Expected::Refused(clause) => {
            assert_eq!(
                (status, error_code(answered).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected a 400 ValidationException, got {status} with \
                 {answered}"
            );
            let message = fault_message(answered);
            assert!(
                message.contains(clause),
                "case {case:?}: expected a message carrying {clause:?}, got {message:?}"
            );
            assert!(
                message.starts_with("Failed to verify "),
                "case {case:?}: a token that does not verify is refused as such, got \
                 {message:?}"
            );
        }
        Expected::RefusedExactly(sentence) => {
            assert_eq!(
                (status, error_code(answered).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected a 400 ValidationException, got {status} with \
                 {answered}"
            );
            assert_eq!(
                fault_message(answered),
                *sentence,
                "case {case:?}: expected exactly {sentence:?}, got {answered}"
            );
        }
        Expected::NoSourceForIssuer(issuer) => {
            assert_eq!(
                (status, error_code(answered).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected a 404 ResourceNotFoundException, got {status} with \
                 {answered}"
            );
            assert_eq!(
                (
                    answered["resourceType"].as_str(),
                    answered["resourceId"].as_str(),
                ),
                (Some("IDENTITY_SOURCE"), Some(issuer.as_str())),
                "case {case:?}: expected the missing identity source to be reported for the \
                 issuer the token claims, got {answered}"
            );
            assert_eq!(
                fault_message(answered),
                no_identity_source_message(issuer, store),
                "case {case:?}: expected the sentence naming the issuer and the store, got \
                 {answered}"
            );
        }
    }
}

/// The issuer of the access-token source: an OIDC provider outside this emulator.
const ACCESS_ISSUER: &str = "https://auth.example.com";

/// One audience that source accepts.
const ACCEPTED_AUDIENCE: &str = "https://myapp.example.com";

/// The issuer of the identity-token source, a provider URL carrying a path.
const IDENTITY_ISSUER: &str = "https://oidc.example.org/tenant";

/// The one client whose identity tokens that source accepts.
const ACCEPTED_CLIENT: &str = "clientone";

/// The prefix that source puts in front of every entity id it generates, as the model
/// documents it ("if you set an `entityIdPrefix` of `MyOIDCProvider`, you can reference
/// principals in your policies in the format `MyCorp::User::MyOIDCProvider|Carlos`").
const ENTITY_ID_PREFIX: &str = "MyOIDCProvider";

/// The group whose members the identity-token store's policy permits.
const PERMITTED_TEAM: &str = "engineering";

/// A decision follows a token's claims read under the OIDC source's configuration, and a
/// token the service can judge wrong is refused rather than answered with a server fault.
///
/// The rows differ along what the rule generalises over: which token member the source
/// processes, which claim names the principal, whether entity ids carry a prefix, whether
/// groups are mapped at all, how the audience is claimed (one `aud`, several, none plus a
/// `client_id`, an unlisted one, a source listing none), where the token sits in its
/// validity window, and which part of the token is missing or unsecured.
#[tokio::test]
async fn a_decision_follows_the_claims_of_a_token_an_external_oidc_issuer_minted() {
    let router = harness::router();

    // Store A: an access-token-only source with no prefix and no group mapping, keyed on
    // `sub`, accepting one audience.
    let store_a = harness::store_with_validation_off(&router).await;
    oidc_source(
        &router,
        &store_a,
        &json!({
            "issuer": ACCESS_ISSUER,
            "tokenSelection": {
                "accessTokenOnly": {
                    "principalIdClaim": "sub",
                    "audiences": [ACCEPTED_AUDIENCE],
                }
            },
        }),
    )
    .await;
    let permit_a = policy(
        &router,
        &store_a,
        format!(
            "permit(principal == {PRINCIPAL_TYPE}::\"carlos\", \
             action == {}::\"{}\", resource == MyCorp::Doc::\"{DOCUMENT}\");",
            ACTION.0, ACTION.1
        ),
    )
    .await;

    // Store B: an identity-token-only source that prefixes its entity ids, maps a group
    // claim of its own choosing and names its principal with a claim that is not `sub`.
    let store_b = harness::store_with_validation_off(&router).await;
    oidc_source(
        &router,
        &store_b,
        &json!({
            "issuer": IDENTITY_ISSUER,
            "entityIdPrefix": ENTITY_ID_PREFIX,
            "groupConfiguration": {
                "groupClaim": GROUP_CLAIM,
                "groupEntityType": GROUP_TYPE,
            },
            "tokenSelection": {
                "identityTokenOnly": {
                    "principalIdClaim": USERNAME_CLAIM,
                    "clientIds": [ACCEPTED_CLIENT],
                }
            },
        }),
    )
    .await;
    let permit_b = policy(
        &router,
        &store_b,
        format!(
            "permit(principal in {GROUP_TYPE}::\"{ENTITY_ID_PREFIX}|{PERMITTED_TEAM}\", \
             action == {}::\"{}\", resource == MyCorp::Doc::\"{DOCUMENT}\") \
             when {{ principal.department == \"platform\" }};",
            ACTION.0, ACTION.1
        ),
    )
    .await;

    // Store C: an access-token-only source listing NO audiences, which accepts any client
    // of its issuer, and permitting every principal it generates.
    let store_c = harness::store_with_validation_off(&router).await;
    oidc_source(
        &router,
        &store_c,
        &json!({
            "issuer": ACCESS_ISSUER,
            "tokenSelection": { "accessTokenOnly": { "principalIdClaim": "sub" } },
        }),
    )
    .await;
    let permit_c = policy(
        &router,
        &store_c,
        format!(
            "permit(principal, action == {}::\"{}\", \
             resource == MyCorp::Doc::\"{DOCUMENT}\");",
            ACTION.0, ACTION.1
        ),
    )
    .await;

    // The claims each row's token carries, built from the source it is presented to.
    let access_claims = |subject: &str, audience: Value, expiry: i64| {
        let mut claims = json!({
            "iss": ACCESS_ISSUER,
            "sub": subject,
            "exp": expiry,
        });
        if !audience.is_null() {
            claims["aud"] = audience;
        }
        claims
    };
    let identity_claims = |username: &str, teams: Value, department: &str| {
        json!({
            "iss": IDENTITY_ISSUER,
            "sub": "ignored-by-this-source",
            USERNAME_CLAIM: username,
            "aud": ACCEPTED_CLIENT,
            GROUP_CLAIM: teams,
            "department": department,
            "exp": live(),
        })
    };
    let allow = |entity_id: String, determining: &str| Expected::Decision {
        decision: "ALLOW",
        entity_id,
        determining: vec![determining.to_string()],
    };
    let deny = |entity_id: String| Expected::Decision {
        decision: "DENY",
        entity_id,
        determining: Vec::new(),
    };
    let prefixed = |id: &str| format!("{ENTITY_ID_PREFIX}|{id}");

    let rows: Vec<(&str, &str, Value, Expected)> = vec![
        (
            "an access token the policy names the principal of",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&access_claims(
                        "carlos",
                        json!(ACCEPTED_AUDIENCE),
                        live(),
                    )),
                }),
            ),
            allow("carlos".to_string(), &permit_a),
        ),
        (
            "the same token with another signature, which no key here can check either",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": format!(
                        "{}.{}.{}",
                        service_common::jwt::encode_base64url(
                            br#"{"alg":"ES256","kid":"oidc-key-2","typ":"JWT"}"#,
                        ),
                        service_common::jwt::encode_base64url(
                            access_claims("carlos", json!(ACCEPTED_AUDIENCE), live())
                                .to_string()
                                .as_bytes(),
                        ),
                        service_common::jwt::encode_base64url(b"another signature"),
                    ),
                }),
            ),
            allow("carlos".to_string(), &permit_a),
        ),
        (
            "an access token of another identity of the same issuer",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&access_claims(
                        "dana",
                        json!(ACCEPTED_AUDIENCE),
                        live(),
                    )),
                }),
            ),
            deny("dana".to_string()),
        ),
        (
            "an access token claiming several audiences, one of them accepted",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&access_claims(
                        "carlos",
                        json!(["https://other.example.com", ACCEPTED_AUDIENCE]),
                        live(),
                    )),
                }),
            ),
            allow("carlos".to_string(), &permit_a),
        ),
        (
            "an access token naming its client in client_id and no aud at all",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&json!({
                        "iss": ACCESS_ISSUER,
                        "sub": "carlos",
                        "client_id": ACCEPTED_AUDIENCE,
                        "exp": live(),
                    })),
                }),
            ),
            allow("carlos".to_string(), &permit_a),
        ),
        (
            "an access token issued to an audience the source does not accept",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&access_claims(
                        "carlos",
                        json!("https://unlisted.example.com"),
                        live(),
                    )),
                }),
            ),
            Expected::RefusedExactly(AUDIENCE_NOT_ACCEPTED),
        ),
        (
            "an access token whose aud is the accepted audience with spaces around it",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&access_claims(
                        "carlos",
                        json!(format!(" {ACCEPTED_AUDIENCE} ")),
                        live(),
                    )),
                }),
            ),
            Expected::RefusedExactly(AUDIENCE_NOT_ACCEPTED),
        ),
        (
            "an access token naming no audience at all",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&access_claims("carlos", Value::Null, live())),
                }),
            ),
            Expected::RefusedExactly(AUDIENCE_NOT_ACCEPTED),
        ),
        (
            "an access token the clock has run out on",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&access_claims(
                        "carlos",
                        json!(ACCEPTED_AUDIENCE),
                        epoch_seconds(Duration::from_secs(60), false),
                    )),
                }),
            ),
            Expected::Refused("the token is expired"),
        ),
        (
            "an access token that names no expiry",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&json!({
                        "iss": ACCESS_ISSUER,
                        "sub": "carlos",
                        "aud": ACCEPTED_AUDIENCE,
                    })),
                }),
            ),
            Expected::Refused("names no exp claim"),
        ),
        (
            "an access token that is not yet valid",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&json!({
                        "iss": ACCESS_ISSUER,
                        "sub": "carlos",
                        "aud": ACCEPTED_AUDIENCE,
                        "nbf": epoch_seconds(Duration::from_secs(600), true),
                        "exp": live(),
                    })),
                }),
            ),
            Expected::Refused("the token is not yet valid"),
        ),
        (
            "an access token carrying no claim the source names its principal with",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&json!({
                        "iss": ACCESS_ISSUER,
                        "aud": ACCEPTED_AUDIENCE,
                        "exp": live(),
                    })),
                }),
            ),
            Expected::Refused("names no sub claim"),
        ),
        (
            "an UNSECURED token carrying the permitted principal's claims",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token_with_header(
                        &json!({ "alg": "none", "typ": "JWT" }),
                        &access_claims("carlos", json!(ACCEPTED_AUDIENCE), live()),
                    ),
                }),
            ),
            Expected::Refused("unsecured"),
        ),
        (
            "an access token presented in the member the source does not process",
            &store_a,
            request(
                &store_a,
                json!({
                    "identityToken": token(&access_claims(
                        "carlos",
                        json!(ACCEPTED_AUDIENCE),
                        live(),
                    )),
                }),
            ),
            Expected::NoSourceForIssuer(ACCESS_ISSUER.to_string()),
        ),
        (
            "a token of an issuer this store has no identity source for",
            &store_a,
            request(
                &store_a,
                json!({
                    "accessToken": token(&json!({
                        "iss": "https://elsewhere.example.net",
                        "sub": "carlos",
                        "aud": ACCEPTED_AUDIENCE,
                        "exp": live(),
                    })),
                }),
            ),
            Expected::NoSourceForIssuer("https://elsewhere.example.net".to_string()),
        ),
        (
            "an identity token whose group claim and attribute the policy requires",
            &store_b,
            request(
                &store_b,
                json!({
                    "identityToken": token(&identity_claims(
                        "carlos",
                        json!([PERMITTED_TEAM, "volunteers"]),
                        "platform",
                    )),
                }),
            ),
            allow(prefixed("carlos"), &permit_b),
        ),
        (
            "an identity token claiming its one group as a bare string",
            &store_b,
            request(
                &store_b,
                json!({
                    "identityToken": token(&identity_claims(
                        "carlos",
                        json!(PERMITTED_TEAM),
                        "platform",
                    )),
                }),
            ),
            allow(prefixed("carlos"), &permit_b),
        ),
        (
            "an identity token in none of the groups the policy names",
            &store_b,
            request(
                &store_b,
                json!({
                    "identityToken": token(&identity_claims(
                        "carlos",
                        json!(["volunteers"]),
                        "platform",
                    )),
                }),
            ),
            deny(prefixed("carlos")),
        ),
        (
            "an identity token in the group whose attribute the policy does not match",
            &store_b,
            request(
                &store_b,
                json!({
                    "identityToken": token(&identity_claims(
                        "carlos",
                        json!([PERMITTED_TEAM]),
                        "sales",
                    )),
                }),
            ),
            deny(prefixed("carlos")),
        ),
        (
            "an identity token of another identity of the same provider",
            &store_b,
            request(
                &store_b,
                json!({
                    "identityToken": token(&identity_claims(
                        "dana",
                        json!([PERMITTED_TEAM]),
                        "platform",
                    )),
                }),
            ),
            allow(prefixed("dana"), &permit_b),
        ),
        (
            "an identity token issued to a client the source does not accept",
            &store_b,
            request(
                &store_b,
                json!({
                    "identityToken": token(&json!({
                        "iss": IDENTITY_ISSUER,
                        USERNAME_CLAIM: "carlos",
                        "aud": "clienttwo",
                        GROUP_CLAIM: [PERMITTED_TEAM],
                        "department": "platform",
                        "exp": live(),
                    })),
                }),
            ),
            Expected::RefusedExactly(AUDIENCE_NOT_ACCEPTED),
        ),
        (
            "an identity token carrying only the sub this source does not key on",
            &store_b,
            request(
                &store_b,
                json!({
                    "identityToken": token(&json!({
                        "iss": IDENTITY_ISSUER,
                        "sub": "carlos",
                        "aud": ACCEPTED_CLIENT,
                        "department": "platform",
                        "exp": live(),
                    })),
                }),
            ),
            Expected::Refused("names no preferred_username claim"),
        ),
        (
            "an identity token presented in the member the source does not process",
            &store_b,
            request(
                &store_b,
                json!({
                    "accessToken": token(&identity_claims(
                        "carlos",
                        json!([PERMITTED_TEAM]),
                        "platform",
                    )),
                }),
            ),
            Expected::NoSourceForIssuer(IDENTITY_ISSUER.to_string()),
        ),
        (
            "an access token of any audience, against a source that lists none",
            &store_c,
            request(
                &store_c,
                json!({
                    "accessToken": token(&access_claims(
                        "carlos",
                        json!("https://whatever.example.com"),
                        live(),
                    )),
                }),
            ),
            allow("carlos".to_string(), &permit_c),
        ),
        (
            "an access token of another identity, against a source that lists none",
            &store_c,
            request(
                &store_c,
                json!({
                    "accessToken": token(&access_claims("dana", Value::Null, live())),
                }),
            ),
            allow("dana".to_string(), &permit_c),
        ),
    ];

    for (case, store, body, expected) in rows {
        let (status, answered) = harness::call(&router, "IsAuthorizedWithToken", &body).await;
        assert_ne!(
            status, 500,
            "case {case:?}: an identity source the store holds is never a server fault, got \
             {answered}"
        );
        assert_answer(case, store, status, &answered, &expected);
    }
}

/// `BatchIsAuthorizedWithToken` reads a token of an OIDC provider by the same rule, and
/// answers one decision per request for the principal its claims name.
#[tokio::test]
async fn a_batch_decides_each_request_for_the_principal_an_oidc_token_names() {
    let router = harness::router();
    let store = harness::store_with_validation_off(&router).await;
    oidc_source(
        &router,
        &store,
        &json!({
            "issuer": ACCESS_ISSUER,
            "entityIdPrefix": ENTITY_ID_PREFIX,
            "tokenSelection": {
                "accessTokenOnly": {
                    "principalIdClaim": "sub",
                    "audiences": [ACCEPTED_AUDIENCE],
                }
            },
        }),
    )
    .await;
    let permit = policy(
        &router,
        &store,
        format!(
            "permit(principal == {PRINCIPAL_TYPE}::\"{ENTITY_ID_PREFIX}|carlos\", \
             action == {}::\"{}\", resource == MyCorp::Doc::\"{DOCUMENT}\");",
            ACTION.0, ACTION.1
        ),
    )
    .await;

    // The requests differ in the resource they ask about: the one the policy names, and
    // another document of the same type.
    let requests = json!([
        {
            "action": { "actionType": ACTION.0, "actionId": ACTION.1 },
            "resource": { "entityType": "MyCorp::Doc", "entityId": DOCUMENT },
        },
        {
            "action": { "actionType": ACTION.0, "actionId": ACTION.1 },
            "resource": { "entityType": "MyCorp::Doc", "entityId": "doc-2" },
        },
    ]);
    let answered = harness::ok(
        &router,
        "BatchIsAuthorizedWithToken",
        &json!({
            "policyStoreId": store,
            "accessToken": token(&json!({
                "iss": ACCESS_ISSUER,
                "sub": "carlos",
                "aud": ACCEPTED_AUDIENCE,
                "exp": live(),
            })),
            "requests": requests,
        }),
    )
    .await;

    assert_eq!(
        answered["principal"],
        json!({
            "entityType": PRINCIPAL_TYPE,
            "entityId": format!("{ENTITY_ID_PREFIX}|carlos"),
        }),
        "case a batch on an OIDC access token: expected the prefixed principal the token's \
         claims name, got {answered}"
    );
    // One row per request: the case, and the decision and determining policies it earns.
    let expected: Vec<(&str, &str, Vec<String>)> = vec![
        ("the document the policy names", "ALLOW", vec![permit]),
        ("another document", "DENY", Vec::new()),
    ];
    for (index, (case, decision, determining)) in expected.into_iter().enumerate() {
        let result = &answered["results"][index];
        assert_eq!(
            result["decision"].as_str(),
            Some(decision),
            "case {case:?}: expected decision {decision}, got {answered}"
        );
        let reported: Vec<String> = result["determiningPolicies"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["policyId"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported, determining,
            "case {case:?}: expected the determining policies {determining:?}, got {answered}"
        );
    }
}
