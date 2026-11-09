// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE precondition an `API_GATEWAY` identity provider is
//! verified against, over every addressing a deployed API Gateway stage is reached at.
//!
//! RULE under test: the invocation-role verification resolves the API the provider's
//! `Url` names into the `execute-api` resource an IAM policy scopes an invoke to —
//! `arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/*` — from EITHER addressing
//! a deployed stage is published at:
//!
//! * `https://<api-id>.execute-api.<region>.amazonaws.com/<stage>/…`, which carries the
//!   api id and the Region in the HOST; and
//! * `https://<endpoint>/_aws/execute-api/<api-id>/<stage>/…`, which carries them in the
//!   PATH behind the reserved namespace segment, because a caller that reaches one
//!   endpoint for every service and every plane cannot carry them in the host. That is
//!   the invoke URL this emulator's own API Gateway emulation publishes.
//!
//! So a role scoped to exactly the API the `Url` names verifies, whichever way the URL
//! names it, and one scoped to another API — or to another Region — does not, with the
//! modelled `InvalidRequestException` (400) and no server persisted.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{member_of, Emulator, ACCOUNT, REGION};

/// The api id of the API each row's provider names: API Gateway's own ten-character
/// lowercase alphanumeric id shape.
const API_ID: &str = "abc1230000";

/// Another API's id, for the rows whose role is scoped away from the one the URL names.
const OTHER_API_ID: &str = "zzz9990000";

/// A Region that is NOT the one the request is signed for ([`REGION`]), so a row can
/// show which Region each addressing reads the API as being in.
const OTHER_REGION: &str = "eu-west-1";

/// A policy allowing `execute-api:Invoke` on exactly `resource`.
fn allow_invoke(resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[
            {{"Effect":"Allow","Action":"execute-api:Invoke","Resource":"{resource}"}}]}}"#
    )
}

/// `arn:aws:execute-api:<region>:<account>:<resource>` — how an IAM policy names an API
/// Gateway invoke resource.
fn execute_api(region: &str, resource: &str) -> String {
    format!("arn:aws:execute-api:{region}:{ACCOUNT}:{resource}")
}

/// One row: the case, the provider `Url`, the `execute-api` resource the seeded role is
/// allowed to invoke, and whether the server is created.
struct Row {
    case: &'static str,
    url: String,
    allowed: String,
    accepted: bool,
}

/// The rows: every addressing crossed with how the role's statement is scoped.
fn rows() -> Vec<Row> {
    // The emulator's own endpoint, as a client on this machine reaches it: a loopback
    // address with the data port. The host is a per-run value in a recording, so each row
    // spells its own rather than deriving one.
    let host_form = format!("https://{API_ID}.execute-api.{REGION}.amazonaws.com/prod");
    vec![
        Row {
            case: "the host form, with the role scoped to that API's stage",
            url: host_form.clone(),
            allowed: execute_api(REGION, &format!("{API_ID}/prod/*")),
            accepted: true,
        },
        Row {
            case: "the host form, with the role scoped to every stage of that API",
            url: host_form.clone(),
            allowed: execute_api(REGION, &format!("{API_ID}/*")),
            accepted: true,
        },
        Row {
            case: "the host form, with the role scoped to another API",
            url: host_form,
            allowed: execute_api(REGION, &format!("{OTHER_API_ID}/*")),
            accepted: false,
        },
        Row {
            case: "the host form in another Region, with the role scoped to that Region",
            url: format!("https://{API_ID}.execute-api.{OTHER_REGION}.amazonaws.com/prod"),
            allowed: execute_api(OTHER_REGION, &format!("{API_ID}/*")),
            accepted: true,
        },
        Row {
            case: "the host form in another Region, with the role scoped to the request's",
            url: format!("https://{API_ID}.execute-api.{OTHER_REGION}.amazonaws.com/prod"),
            allowed: execute_api(REGION, &format!("{API_ID}/*")),
            accepted: false,
        },
        Row {
            case: "the path form on a loopback endpoint, scoped to every stage of that API",
            url: format!("https://127.0.0.1:39477/_aws/execute-api/{API_ID}/sim2real"),
            allowed: execute_api(REGION, &format!("{API_ID}/*")),
            accepted: true,
        },
        Row {
            case: "the path form, scoped to exactly the stage it names",
            url: format!("https://127.0.0.1:46407/_aws/execute-api/{API_ID}/sim2real"),
            allowed: execute_api(REGION, &format!("{API_ID}/sim2real/*")),
            accepted: true,
        },
        Row {
            case: "the path form, scoped to a DIFFERENT stage of the same API",
            url: format!("https://127.0.0.1:34285/_aws/execute-api/{API_ID}/sim2real"),
            allowed: execute_api(REGION, &format!("{API_ID}/test/*")),
            accepted: false,
        },
        Row {
            case: "the path form naming a resource path after the stage",
            url: format!("https://localhost:9297/_aws/execute-api/{API_ID}/test/auth"),
            allowed: execute_api(REGION, &format!("{API_ID}/test/*")),
            accepted: true,
        },
        Row {
            case: "the path form over plain HTTP",
            url: format!("http://127.0.0.1:9297/_aws/execute-api/{API_ID}/prod"),
            allowed: execute_api(REGION, &format!("{API_ID}/*")),
            accepted: true,
        },
        Row {
            case: "the path form, scoped to another API",
            url: format!("https://127.0.0.1:9297/_aws/execute-api/{API_ID}/prod"),
            allowed: execute_api(REGION, &format!("{OTHER_API_ID}/*")),
            accepted: false,
        },
        Row {
            case: "the path form, scoped to the same API in another Region",
            url: format!("https://127.0.0.1:9297/_aws/execute-api/{API_ID}/prod"),
            allowed: execute_api(OTHER_REGION, &format!("{API_ID}/*")),
            accepted: false,
        },
        Row {
            case: "a URL that is no invoke endpoint, with a narrowly scoped role",
            url: "https://idp.example.invalid/auth".to_string(),
            allowed: execute_api(REGION, &format!("{API_ID}/*")),
            accepted: false,
        },
    ]
}

/// The provider members a row sends.
fn provider(url: &str, role_arn: &str) -> Value {
    json!({
        "IdentityProviderType": "API_GATEWAY",
        "IdentityProviderDetails": {"Url": url, "InvocationRole": role_arn},
    })
}

#[tokio::test]
async fn an_api_gateway_provider_verifies_against_the_api_either_invoke_url_form_names() {
    for row in rows() {
        let emulator = Emulator::new();
        let role_arn = "arn:aws:iam::000000000000:role/transfer-idp";
        emulator.seed_role(role_arn, &allow_invoke(&row.allowed));

        let answer = emulator
            .call("CreateServer", provider(&row.url, role_arn))
            .await;

        if !row.accepted {
            assert_eq!(
                answer.status, 400,
                "case {:?}: expected HTTP 400, got {} with body {}",
                row.case, answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidRequestException",
                "case {:?}: expected InvalidRequestException, got body {}",
                row.case,
                answer.body
            );
            let message = member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                message,
                format!("Unable to verify access to API {}", row.url),
                "case {:?}: the refusal must name the API it could not verify",
                row.case
            );
            let listed = emulator.call("ListServers", json!({})).await;
            let servers = listed
                .get("Servers")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                servers.is_empty(),
                "case {:?}: a refused create must persist no server, got {}",
                row.case,
                listed.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 200,
            "case {:?}: expected the create to be served, got {} with body {}",
            row.case, answer.status, answer.body
        );
        let server_id = answer
            .get("ServerId")
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected a ServerId", row.case))
            .to_string();

        // The round trip: the server really was persisted as an API_GATEWAY-backed one,
        // with the provider it was verified for.
        let described = emulator
            .call("DescribeServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected DescribeServer 200, got {} with body {}",
            row.case, described.status, described.body
        );
        let server = described.get("Server").clone();
        assert_eq!(
            member_of(&server, "IdentityProviderType"),
            &json!("API_GATEWAY"),
            "case {:?}: expected the stored provider type, got {server}",
            row.case
        );
        let details = member_of(&server, "IdentityProviderDetails").clone();
        assert_eq!(
            (
                member_of(&details, "Url").as_str(),
                member_of(&details, "InvocationRole").as_str(),
            ),
            (Some(row.url.as_str()), Some(role_arn)),
            "case {:?}: expected the provider to be reported as configured, got {details}",
            row.case
        );
    }
}

/// RULE (the same rule, at the other operation that verifies it): `UpdateServer`
/// verifies a provider it is handed on exactly the same reading of the `Url`, so a
/// server can be moved onto an API named by either addressing.
///
/// The rows differ along the addressing and along whether the role covers the API.
#[tokio::test]
async fn an_update_verifies_a_provider_on_the_same_reading_of_the_invoke_url() {
    // (case, the Url to update onto, the resource the role may invoke, accepted)
    let cases: Vec<(&str, String, String, bool)> = vec![
        (
            "the host form, covered",
            format!("https://{API_ID}.execute-api.{REGION}.amazonaws.com/prod"),
            execute_api(REGION, &format!("{API_ID}/*")),
            true,
        ),
        (
            "the path form, covered",
            format!("https://127.0.0.1:39477/_aws/execute-api/{API_ID}/sim2real"),
            execute_api(REGION, &format!("{API_ID}/*")),
            true,
        ),
        (
            "the path form, not covered",
            format!("https://127.0.0.1:39477/_aws/execute-api/{API_ID}/sim2real"),
            execute_api(REGION, &format!("{OTHER_API_ID}/*")),
            false,
        ),
    ];

    for (case, url, allowed, accepted) in cases {
        let emulator = Emulator::new();
        let role_arn = "arn:aws:iam::000000000000:role/transfer-idp";
        emulator.seed_role(role_arn, &allow_invoke(&allowed));
        let server_id = emulator.create_server(json!({})).await;

        let mut request = provider(&url, role_arn);
        request["ServerId"] = json!(&server_id);
        let answer = emulator.call("UpdateServer", request).await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the update to be served, got {} with body {}",
                answer.status, answer.body
            );
            let described = emulator
                .call("DescribeServer", json!({"ServerId": &server_id}))
                .await;
            let details =
                member_of(described.get("Server"), "IdentityProviderDetails").clone();
            assert_eq!(
                member_of(&details, "Url").as_str(),
                Some(url.as_str()),
                "case {case:?}: expected the updated provider to be stored, got {details}"
            );
        } else {
            assert_eq!(
                answer.error_code(),
                "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException, got {} with body {}",
                answer.status,
                answer.body
            );
            let described = emulator
                .call("DescribeServer", json!({"ServerId": &server_id}))
                .await;
            assert_eq!(
                member_of(described.get("Server"), "IdentityProviderType"),
                &json!("SERVICE_MANAGED"),
                "case {case:?}: a refused update must leave the stored provider alone, got {}",
                described.body
            );
        }
    }
}
