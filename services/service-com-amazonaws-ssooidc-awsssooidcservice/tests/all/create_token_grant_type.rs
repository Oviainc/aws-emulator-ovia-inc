// SPDX-License-Identifier: CC0-1.0

//! `CreateToken`'s `grantType`, end to end over the wire.
//!
//! RULE: `CreateToken` serves exactly the three grants its model enumerates, and a
//! `grantType` carrying any other value names no grant flow of the endpoint at all,
//! so the request is refused as a MALFORMED one — HTTP 400, `InvalidRequestException`,
//! OAuth `error` `invalid_request`, and the fixed text of that class — whatever grant
//! types the presenting client registered and whatever the rejected value looks like,
//! while each of the three served grants keeps flowing to grant resolution and gets
//! its own modeled refusal (or a token).
//!
//! Every case drives the service through the router the production emulator serves,
//! using the harness the device-flow suite beside this file builds.

use serde_json::{json, Value};

use ssooidc::state::{
    DESCRIPTION_AUTHORIZATION_CODE_REQUIRED, DESCRIPTION_DEVICE_CODE_REQUIRED,
    DESCRIPTION_INVALID_AUTHORIZATION_CODE, DESCRIPTION_INVALID_DEVICE_CODE,
    DESCRIPTION_INVALID_REFRESH_TOKEN, DESCRIPTION_INVALID_REQUEST,
    GRANT_TYPE_AUTHORIZATION_CODE, GRANT_TYPE_DEVICE_CODE, GRANT_TYPE_REFRESH_TOKEN,
};
use service_com_amazonaws_ssooidc_awsssooidcservice as ssooidc;

use crate::device_authorization_flow::{
    assert_failure, device_flow_token, error_shape, text, Emulator, REGION,
};

/// The route `CreateToken` is bound to.
const TOKEN_PATH: &str = "/token";

/// Every grant type `CreateToken` serves, from `CreateTokenRequest$grantType`:
/// "Specify one of the following values, depending on the grant type that you want:
/// Authorization Code - authorization_code; Device Code -
/// urn:ietf:params:oauth:grant-type:device_code; Refresh Token - refresh_token", read
/// with
/// `smithy_query(query="doc",shape="com.amazonaws.ssooidc#CreateTokenRequest",member="grantType")`.
const SERVED_GRANT_TYPES: [&str; 3] = [
    GRANT_TYPE_AUTHORIZATION_CODE,
    GRANT_TYPE_DEVICE_CODE,
    GRANT_TYPE_REFRESH_TOKEN,
];

/// A `grantType` outside the three served ones, and how its value differs from them.
struct Unserved {
    case: &'static str,
    grant_type: String,
}

fn unserved(case: &'static str, grant_type: impl Into<String>) -> Unserved {
    Unserved {
        case,
        grant_type: grant_type.into(),
    }
}

/// The values the table spans: another OAuth grant name, an empty value, a served
/// name in another case or padded with whitespace, a served name with something
/// appended, the grant URNs the sibling IAM operation serves, a URN of a grant
/// neither operation serves, and a value far longer than any name.
fn unserved_grant_types() -> Vec<Unserved> {
    vec![
        unserved("the client-credentials grant", "client_credentials"),
        unserved("the resource-owner password grant", "password"),
        unserved("the implicit grant", "implicit"),
        unserved("an empty grantType", ""),
        unserved("a grantType of blanks", "   "),
        unserved("a served name in upper case", "AUTHORIZATION_CODE"),
        unserved("a served name padded with blanks", " refresh_token "),
        unserved("a served name with a suffix", "refresh_token_2"),
        unserved(
            "a served URN in upper case",
            "URN:IETF:PARAMS:OAUTH:GRANT-TYPE:DEVICE_CODE",
        ),
        // The JWT-bearer and token-exchange grants are `CreateTokenWithIAM`'s, so
        // on this operation they are as unserved as any other value.
        unserved(
            "the JWT-bearer grant",
            "urn:ietf:params:oauth:grant-type:jwt-bearer",
        ),
        unserved(
            "the token-exchange grant",
            "urn:ietf:params:oauth:grant-type:token-exchange",
        ),
        unserved(
            "a grant URN no operation of this API serves",
            "urn:ietf:params:oauth:grant-type:saml2-bearer",
        ),
        unserved("a grantType longer than any served name", "g".repeat(256)),
    ]
}

/// Assert the whole refusal of one out-of-enum `grantType`: the status, the modeled
/// shape named by both the body and the error-type header, the OAuth `error` code,
/// the fixed text, and that the rejected value is not echoed back.
async fn assert_malformed_request(emulator: &Emulator, case: &str, body: &Value) {
    let (status, error_type, response) = emulator
        .post_with_error_type(TOKEN_PATH, REGION, body)
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {response}"
    );
    assert!(
        error_shape(&response).contains("InvalidRequestException"),
        "case {case:?}: expected the InvalidRequestException shape, got {response}"
    );
    assert!(
        error_type
            .as_deref()
            .is_some_and(|named| named.contains("InvalidRequestException")),
        "case {case:?}: expected the InvalidRequestException error-type header, got {error_type:?}"
    );
    assert_eq!(
        (
            response["error"].as_str(),
            response["error_description"].as_str(),
        ),
        (Some("invalid_request"), Some(DESCRIPTION_INVALID_REQUEST)),
        "case {case:?}: expected the fixed invalid-request refusal, got {response}"
    );
    let rejected = body["grantType"].as_str().unwrap_or_default();
    assert!(
        rejected.len() < 3
            || !response["error_description"]
                .as_str()
                .unwrap_or_default()
                .contains(rejected),
        "case {case:?}: the text must not echo the rejected {rejected:?}, got {response}"
    );
}

/// RULE (above): a `grantType` outside the three `CreateToken` serves is refused as a
/// malformed request, whatever grant types the client registered. The table spans the
/// unserved values; the registrations span what the client itself registered.
#[tokio::test]
async fn create_token_refuses_an_unserved_grant_type_as_a_malformed_request() {
    let emulator = Emulator::new();

    // What the presenting client registered must make no difference: one client
    // registered every served grant, one only the device-code grant, and one named
    // no grant types at all.
    let registrations: Vec<(&str, Value)> = vec![
        ("a client that registered no grant types", json!({})),
        (
            "a client that registered every served grant",
            json!({"grantTypes": SERVED_GRANT_TYPES}),
        ),
        (
            "a client that registered only the device-code grant",
            json!({"grantTypes": [GRANT_TYPE_DEVICE_CODE]}),
        ),
    ];

    for (registration, extra) in registrations {
        let (client_id, client_secret) = emulator.registered_client(extra).await;
        for Unserved { case, grant_type } in unserved_grant_types() {
            let case = format!("{case} presented by {registration}");
            assert_malformed_request(
                &emulator,
                &case,
                &json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    "grantType": grant_type,
                }),
            )
            .await;
        }
    }
}

/// RULE (above): the refusal is settled on the request alone, so the members a served
/// grant would need, and the credential presented beside them, do not change it.
#[tokio::test]
async fn an_unserved_grant_type_is_refused_whatever_else_the_request_carries() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;
    let token = device_flow_token(&emulator, &client_id, &client_secret).await;
    let refresh_token = text(&token, "refreshToken");

    let rows: Vec<(&str, Value)> = vec![
        (
            "an unserved grantType carrying a deviceCode member",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": "client_credentials",
                "deviceCode": "fabricated-device-code-nothing-issued",
            }),
        ),
        (
            // The refresh token is a LIVE grant of this client, and the unserved
            // grant type is still what the request is refused for.
            "an unserved grantType carrying a refreshToken this service issued",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": "password",
                "refreshToken": refresh_token,
            }),
        ),
        (
            "an unserved grantType carrying a code and a redirectUri",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": "implicit",
                "code": "5baa61e4c9b93f3f0682250b6cf8331b",
                "redirectUri": "http://127.0.0.1:52000/oauth/callback",
            }),
        ),
        (
            "an unserved grantType with a wrong clientSecret",
            json!({
                "clientId": client_id,
                "clientSecret": "not-the-secret",
                "grantType": "client_credentials",
            }),
        ),
        (
            "an unserved grantType with a requested scope",
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": "urn:ietf:params:oauth:grant-type:token-exchange",
                "scope": ["sso:account:access"],
            }),
        ),
    ];

    for (case, body) in rows {
        assert_malformed_request(&emulator, case, &body).await;
    }

    // The live refresh token was not spent by any of the refused requests: the
    // served grant still rotates it.
    let (status, rotated) = emulator
        .create_token(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "grantType": GRANT_TYPE_REFRESH_TOKEN,
            "refreshToken": refresh_token,
        }))
        .await;
    assert_eq!(
        status, 200,
        "case \"the refresh grant after the refused requests\": expected HTTP 200, got {status} with {rotated}"
    );
}

/// RULE (above): the three served grants keep flowing to grant resolution — each one
/// is answered from the state it resolves to, not from the grant-type check, so a
/// served grant presented with nothing stored is an invalid grant and one presented
/// without its member is the malformed request naming that member.
#[tokio::test]
async fn every_served_grant_type_still_reaches_grant_resolution() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;

    // Each served grant, with its member absent and then fabricated.
    let rows: Vec<(&str, Value, &str, &str, &str)> = vec![
        (
            "the device-code grant with no deviceCode",
            json!({"grantType": GRANT_TYPE_DEVICE_CODE}),
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_DEVICE_CODE_REQUIRED,
        ),
        (
            "the device-code grant with a fabricated deviceCode",
            json!({
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "fabricated-device-code-does-not-exist",
            }),
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_DEVICE_CODE,
        ),
        (
            "the refresh-token grant with a fabricated refreshToken",
            json!({
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": "fabricated-refresh-token-does-not-exist",
            }),
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_REFRESH_TOKEN,
        ),
        (
            "the authorization-code grant with no code",
            json!({"grantType": GRANT_TYPE_AUTHORIZATION_CODE}),
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_AUTHORIZATION_CODE_REQUIRED,
        ),
        (
            "the authorization-code grant with a fabricated code",
            json!({
                "grantType": GRANT_TYPE_AUTHORIZATION_CODE,
                "code": "aGVsbG8td29ybGQ",
            }),
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_AUTHORIZATION_CODE,
        ),
    ];

    for (case, grant, shape, error, description) in rows {
        let mut body = json!({"clientId": client_id, "clientSecret": client_secret});
        for (member, value) in grant.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }
        let (status, response) = emulator.create_token(&body).await;
        assert_failure(case, status, &response, 400, shape, error);
        assert_eq!(
            response["error_description"].as_str(),
            Some(description),
            "case {case:?}: expected the fixed text of its class, got {response}"
        );
    }

    // And the served device-code grant still issues a token end to end, so the
    // grant-type check refuses nothing it must serve.
    let issued = device_flow_token(&emulator, &client_id, &client_secret).await;
    assert!(
        !text(&issued, "accessToken").is_empty(),
        "case \"the served device-code grant\": expected an accessToken, got {issued}"
    );
}
