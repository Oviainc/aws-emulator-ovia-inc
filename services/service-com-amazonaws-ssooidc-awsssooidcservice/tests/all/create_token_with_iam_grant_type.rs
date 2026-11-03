// SPDX-License-Identifier: CC0-1.0

//! `CreateTokenWithIAM`'s `grantType`, end to end over the wire.
//!
//! RULE: `CreateTokenWithIAM` serves exactly the four grants its model enumerates,
//! and the grants an application ARN can have configured are only those four — so a
//! `grantType` carrying any other value names a grant flow of no client of this
//! endpoint, and the request authenticates no client: HTTP 401,
//! `InvalidClientException`, OAuth `error` `invalid_client` and the fixed text of
//! that class, whatever client the request names, whatever grant members it carries
//! and whatever the rejected value looks like. Each of the four served grants keeps
//! flowing to grant resolution and gets its own modeled refusal (or a token).
//!
//! Every case drives the service through the router the production emulator serves,
//! using the harness the device-flow suite beside this file builds.

use serde_json::{json, Value};

use service_com_amazonaws_ssooidc_awsssooidcservice as ssooidc;
use ssooidc::state::{
    DESCRIPTION_ASSERTION_REQUIRED, DESCRIPTION_AUTHORIZATION_CODE_REQUIRED,
    DESCRIPTION_INVALID_ASSERTION, DESCRIPTION_INVALID_AUTHORIZATION_CODE,
    DESCRIPTION_INVALID_CLIENT, DESCRIPTION_INVALID_REFRESH_TOKEN,
    DESCRIPTION_INVALID_SUBJECT_TOKEN, DESCRIPTION_REFRESH_TOKEN_REQUIRED,
    DESCRIPTION_SUBJECT_TOKEN_REQUIRED, GRANT_TYPE_AUTHORIZATION_CODE, GRANT_TYPE_DEVICE_CODE,
    GRANT_TYPE_JWT_BEARER, GRANT_TYPE_REFRESH_TOKEN, GRANT_TYPE_TOKEN_EXCHANGE,
    IAM_SUPPORTED_GRANT_TYPES,
};

use crate::device_authorization_flow::{
    assert_failure, device_flow_token, error_shape, text, Emulator, REGION,
};

/// The route `CreateTokenWithIAM` is bound to: `CreateToken`'s path with the
/// `aws_iam=t` query literal the model's `@http` URI carries.
const IAM_TOKEN_PATH: &str = "/token?aws_iam=t";

/// A well-formed but never-existing IAM Identity Center application ARN: this
/// operation's `clientId` "is an application ARN that has OAuth grants configured",
/// and no row may depend on the ARN naming anything this service holds.
const UNKNOWN_APPLICATION_ARN: &str =
    "arn:aws:sso::222233334444:application/ssoins-3333333333333333/apl-4444444444444444";

/// A `grantType` outside the four served ones, and how its value differs from them.
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

/// The values the table spans: the device-code grant (the sibling operation's, whose
/// `deviceCode` member this request shape does not even model), other OAuth grant
/// names, an empty value, a served name in another case or padded with whitespace, a
/// served name with something appended, a grant URN no operation of this API serves,
/// and a value far longer than any name.
fn unserved_grant_types() -> Vec<Unserved> {
    vec![
        unserved("the device-code grant", GRANT_TYPE_DEVICE_CODE),
        unserved(
            "the device-code grant in upper case",
            "URN:IETF:PARAMS:OAUTH:GRANT-TYPE:DEVICE_CODE",
        ),
        unserved("the client-credentials grant", "client_credentials"),
        unserved("the resource-owner password grant", "password"),
        unserved("the implicit grant", "implicit"),
        unserved("an empty grantType", ""),
        unserved("a grantType of blanks", "   "),
        unserved("a served name in upper case", "REFRESH_TOKEN"),
        unserved("a served name padded with blanks", " refresh_token "),
        unserved("a served name with a suffix", "refresh_token_2"),
        unserved(
            "a served URN with a suffix",
            "urn:ietf:params:oauth:grant-type:jwt-bearer-2",
        ),
        unserved(
            "a grant URN no operation of this API serves",
            "urn:ietf:params:oauth:grant-type:saml2-bearer",
        ),
        unserved("a grantType longer than any served name", "g".repeat(256)),
    ]
}

/// Assert the whole refusal of one unserved `grantType`: the status, the modeled
/// shape named by both the body and the error-type header, the OAuth `error` code,
/// the fixed text of the class, and that the rejected value is not echoed back.
async fn assert_unauthenticated_client(emulator: &Emulator, case: &str, body: &Value) {
    let (status, error_type, response) = emulator
        .post_with_error_type(IAM_TOKEN_PATH, REGION, body)
        .await;
    assert_eq!(
        status, 401,
        "case {case:?}: expected HTTP 401, got {status} with {response}"
    );
    assert!(
        error_shape(&response).contains("InvalidClientException"),
        "case {case:?}: expected the InvalidClientException shape, got {response}"
    );
    assert!(
        error_type
            .as_deref()
            .is_some_and(|named| named.contains("InvalidClientException")),
        "case {case:?}: expected the InvalidClientException error-type header, got {error_type:?}"
    );
    assert_eq!(
        (
            response["error"].as_str(),
            response["error_description"].as_str(),
        ),
        (Some("invalid_client"), Some(DESCRIPTION_INVALID_CLIENT)),
        "case {case:?}: expected the fixed invalid-client refusal, got {response}"
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

/// RULE (above): a `grantType` outside the four `CreateTokenWithIAM` serves refuses
/// the request as an unauthenticated client, whatever client it names and whatever
/// that client registered. The table spans the unserved values; the clients span an
/// ARN this service holds nothing for, a registration that named every grant it can
/// and one restricted to the grant the sibling operation serves.
#[tokio::test]
async fn an_unserved_grant_type_refuses_the_request_as_an_unauthenticated_client() {
    let emulator = Emulator::new();

    let (every_grant_id, _) = emulator
        .registered_client(json!({
            "clientName": "every-grant",
            "grantTypes": [GRANT_TYPE_AUTHORIZATION_CODE, GRANT_TYPE_REFRESH_TOKEN],
        }))
        .await;
    let (device_only_id, _) = emulator
        .registered_client(json!({
            "clientName": "device-only",
            "grantTypes": [GRANT_TYPE_DEVICE_CODE],
        }))
        .await;

    let clients: Vec<(&str, String)> = vec![
        (
            "an application ARN this service holds nothing for",
            UNKNOWN_APPLICATION_ARN.to_string(),
        ),
        (
            "a client registered for grants this operation serves",
            every_grant_id,
        ),
        (
            "a client registered only for the device-code grant",
            device_only_id,
        ),
    ];

    for (client_case, client_id) in clients {
        for Unserved { case, grant_type } in unserved_grant_types() {
            assert!(
                !IAM_SUPPORTED_GRANT_TYPES.contains(&grant_type.as_str()),
                "case {case:?}: the table may only carry values outside the served grants, \
                 got {grant_type:?}"
            );
            let case = format!("{case} presented by {client_case}");
            assert_unauthenticated_client(
                &emulator,
                &case,
                &json!({"clientId": client_id, "grantType": grant_type}),
            )
            .await;
        }
    }
}

/// RULE (above): the refusal is settled on the grant type alone, so the members a
/// served grant would need — including live grants of the client itself — do not
/// change it, and none of them is spent.
#[tokio::test]
async fn an_unserved_grant_type_is_refused_whatever_else_the_request_carries() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;
    let token = device_flow_token(&emulator, &client_id, &client_secret).await;
    let access_token = text(&token, "accessToken");
    let refresh_token = text(&token, "refreshToken");

    let rows: Vec<(&str, Value)> = vec![
        (
            "the device-code grant carrying a deviceCode member",
            json!({
                "clientId": client_id,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "fabricated-device-code-nothing-issued",
            }),
        ),
        (
            // The refresh token is a LIVE grant of this client, and the unserved
            // grant type is still what the request is refused for.
            "an unserved grantType carrying a refreshToken this service issued",
            json!({
                "clientId": client_id,
                "grantType": "password",
                "refreshToken": refresh_token,
            }),
        ),
        (
            "an unserved grantType carrying an access token this service issued",
            json!({
                "clientId": client_id,
                "grantType": "client_credentials",
                "subjectToken": access_token,
            }),
        ),
        (
            "an unserved grantType carrying a code and a redirectUri",
            json!({
                "clientId": client_id,
                "grantType": "implicit",
                "code": "5baa61e4c9b93f3f0682250b6cf8331b",
                "redirectUri": "http://127.0.0.1:52000/oauth/callback",
            }),
        ),
        (
            "an unserved grantType with a requested scope",
            json!({
                "clientId": client_id,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "scope": ["sso:account:access"],
            }),
        ),
    ];

    for (case, body) in rows {
        assert_unauthenticated_client(&emulator, case, &body).await;
    }

    // Nothing was spent and nothing was minted by the refused requests: the served
    // refresh grant still rotates the same refresh token through this operation.
    let (status, rotated) = emulator
        .post(
            IAM_TOKEN_PATH,
            REGION,
            &json!({
                "clientId": client_id,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": refresh_token,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the served refresh grant after the refused requests\": expected HTTP 200, \
         got {status} with {rotated}"
    );
    assert!(
        !text(&rotated, "accessToken").is_empty(),
        "case \"the served refresh grant after the refused requests\": expected an \
         accessToken, got {rotated}"
    );
}

/// RULE (above): each of the four served grants keeps flowing to grant resolution —
/// it is answered from the state the presented grant resolves to, not from the
/// grant-type check — so a served grant presented without its member is the
/// malformed request naming that member, and one presented with a value this service
/// never issued is an invalid grant, even for the same application ARN that holds
/// nothing. The rows span every entry of the served catalog.
#[tokio::test]
async fn every_served_grant_type_still_reaches_grant_resolution() {
    let emulator = Emulator::new();

    // Each served grant, with its member absent and then fabricated.
    let rows: Vec<(&str, &str, Value, u16, &str, &str, &str)> = vec![
        (
            "the refresh-token grant with no refreshToken",
            GRANT_TYPE_REFRESH_TOKEN,
            json!({}),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_REFRESH_TOKEN_REQUIRED,
        ),
        (
            "the refresh-token grant with a refreshToken this service never issued",
            GRANT_TYPE_REFRESH_TOKEN,
            json!({"refreshToken": "fabricated-refresh-token-nothing-issued"}),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_REFRESH_TOKEN,
        ),
        (
            "the token-exchange grant with no subjectToken",
            GRANT_TYPE_TOKEN_EXCHANGE,
            json!({}),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_SUBJECT_TOKEN_REQUIRED,
        ),
        (
            "the token-exchange grant with a subjectToken this service never issued",
            GRANT_TYPE_TOKEN_EXCHANGE,
            json!({"subjectToken": "fabricated-subject-token-nothing-issued"}),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_SUBJECT_TOKEN,
        ),
        (
            "the authorization-code grant with no code",
            GRANT_TYPE_AUTHORIZATION_CODE,
            json!({}),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_AUTHORIZATION_CODE_REQUIRED,
        ),
        (
            "the authorization-code grant with a fabricated code",
            GRANT_TYPE_AUTHORIZATION_CODE,
            json!({"code": "ZmFicmljYXRlZC1hdXRob3JpemF0aW9uLWNvZGU"}),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_AUTHORIZATION_CODE,
        ),
        (
            "the JWT-bearer grant with no assertion",
            GRANT_TYPE_JWT_BEARER,
            json!({}),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_ASSERTION_REQUIRED,
        ),
        (
            "the JWT-bearer grant with an assertion from an untrusted issuer",
            GRANT_TYPE_JWT_BEARER,
            json!({"assertion": "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.c2ln"}),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_ASSERTION,
        ),
    ];

    // Every grant the operation serves must be exercised by a row above, so the
    // served catalog and the rule are checked against each other and not only
    // against the grant the issue cited.
    for served in IAM_SUPPORTED_GRANT_TYPES {
        assert!(
            rows.iter().any(|(_, grant_type, ..)| grant_type == served),
            "case \"the served catalog\": no row presents the served grant {served:?}"
        );
    }

    for (case, grant_type, grant, expected_status, shape, error, description) in rows {
        let mut body = json!({"clientId": UNKNOWN_APPLICATION_ARN, "grantType": grant_type});
        for (member, value) in grant.as_object().cloned().unwrap_or_default() {
            body[member] = value;
        }
        let (status, response) = emulator.post(IAM_TOKEN_PATH, REGION, &body).await;
        assert_failure(case, status, &response, expected_status, shape, error);
        assert_eq!(
            response["error_description"].as_str(),
            Some(description),
            "case {case:?}: expected the fixed text of its class, got {response}"
        );
    }
}
