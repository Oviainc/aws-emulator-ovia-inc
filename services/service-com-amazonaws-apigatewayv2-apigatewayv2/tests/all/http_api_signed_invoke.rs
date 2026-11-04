// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SIGNED half of the `execute-api` HTTP data plane: what a
//! deployed HTTP api stage answers a request that carries a SigV4 credential with, on an
//! emulator that verifies request signatures.
//!
//! The RULE under test, over a SET of requests rather than one recorded call: every request
//! addressed to a deployed stage is answered by THE STAGE, from the api's own records —
//! the integration's answer, or the status and sentence the route's own records earn it —
//! whatever credential the request carries and whichever signing name that credential's
//! scope names. An invoke is signed under the data plane's own signing name (the
//! `execute-api` service AWS publishes the invoke endpoint and the `execute-api:Invoke`
//! action under), which is not the signing name of this service's control plane, so a
//! credential gate keyed to the control plane's name would refuse every correctly signed
//! invoke before the stage was reached.
//!
//! The rows span what the rule generalises over: WHICH credential form the request carries
//! (an `Authorization` header, a session credential with its security token, a presigned
//! URL's query credential, a scope naming another Region, no credential at all), WHICH
//! `AuthorizationType` the route declares (`AWS_IAM` and `NONE`), WHICH path is named (a
//! route of the api and one it holds none for) and HOW the request addresses the api (its
//! own endpoint host and the rewritten endpoint a client that resolves that host nowhere
//! uses).
//!
//! Authentication is genuinely ON for the rows: the control asserts a modelled
//! control-plane operation is refused by the same emulator, which is what makes the served
//! rows evidence of anything.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator};
use crate::websocket_handshake_data_plane::{message, send, Answer};

/// The Region the control-plane requests are signed for, and so the Region the api's own
/// endpoint host names. It is [`crate::api_lifecycle::call`]'s own.
const REGION: &str = "eu-west-1";

/// The signing name an invoke of a deployed stage is signed under: the `execute-api`
/// service, which AWS publishes the invoke endpoint and the `execute-api:Invoke` action
/// under. NOT this service's own signing name, which is its control plane's (`apigateway`).
const INVOKE_SIGNING_NAME: &str = "execute-api";

/// The emulator's own endpoint, which a client that resolves the api's endpoint host
/// nowhere sends as the `Host` instead.
const OWN_ENDPOINT: &str = "127.0.0.1:9297";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// The status and sentence an ADMITTED request earns: its route's integration names a
/// backend this emulator cannot call, so the gateway answers as it does for an integration
/// that produced no response. Asserting it is how a case observes that the request reached
/// the integration at all.
const INTEGRATION_REACHED: u16 = 500;
const INTEGRATION_REACHED_MESSAGE: &str = "Internal Server Error";

/// The status and sentence an `AWS_IAM` route reached with no AWS credential earns.
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The status and sentence a request matching no route of the api earns.
const NOT_FOUND: u16 = 404;
const NOT_FOUND_MESSAGE: &str = "Not Found";

/// The code the emulator's own credential gate refuses an unverifiable request with — the
/// answer that must never stand in front of the data plane. It is the gate's, not AWS's:
/// the control below asserts it on a MODELLED operation, which is where it belongs.
const CREDENTIAL_GATE_CODE: &str = "AccessDenied";

/// A value in the shape of a SigV4 signature: 64 lowercase hex characters, as
/// `AWS4-HMAC-SHA256` produces.
const SIGNATURE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A value in the shape of a session token, as temporary credentials carry one.
const SESSION_TOKEN: &str = "FwoGZXIvYXdzEBYaDEVNVUxBVE9SU0VTU0lPTg";

/// The `x-amz-date` a header-signed request carries, and the date part of its scope.
const AMZ_DATE: &str = "20261021T000000Z";
const SCOPE_DATE: &str = "20261021";

/// The `Authorization` header a request signed for `region` under the data plane's own
/// signing name carries.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/{SCOPE_DATE}/{region}/{INVOKE_SIGNING_NAME}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature={SIGNATURE}"
    )
}

/// The query a presigned URL signed for `region` carries its credential in.
fn presigned_query(region: &str) -> String {
    format!(
        "?X-Amz-Algorithm=AWS4-HMAC-SHA256\
         &X-Amz-Credential=AKIDEXAMPLE%2F{SCOPE_DATE}%2F{region}%2F{INVOKE_SIGNING_NAME}\
         %2Faws4_request\
         &X-Amz-Date={AMZ_DATE}&X-Amz-Expires=300&X-Amz-SignedHeaders=host\
         &X-Amz-Signature={SIGNATURE}"
    )
}

/// What a client needs to address the deployed api.
struct Deployed {
    api_id: String,
    endpoint_host: String,
}

/// Create an HTTP api with a Lambda-backed `AWS_PROXY` integration behind two routes — one
/// `AWS_IAM`, one `NONE` — and an auto-deploying stage.
async fn deploy(emulator: &MultiServiceRouter) -> Deployed {
    let (status, api) = call(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "signed-invoke-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case CreateApi: expected 201, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let endpoint = as_str(&api, "apiEndpoint");
    let endpoint_host = endpoint
        .split_once("://")
        .map_or(endpoint.as_str(), |(_scheme, host)| host)
        .to_string();

    let (status, integration) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": format!("arn:aws:lambda:{REGION}:000000000000:function:echo"),
            "payloadFormatVersion": "2.0"
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case CreateIntegration: expected 201, got {status} {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    for (route_key, authorization_type) in [("GET /signed", "AWS_IAM"), ("GET /open", "NONE")] {
        let (status, route) = call(
            emulator,
            "POST",
            &format!("/v2/apis/{api_id}/routes"),
            Some(json!({
                "routeKey": route_key,
                "target": format!("integrations/{integration_id}"),
                "authorizationType": authorization_type,
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case CreateRoute({route_key}): expected 201, got {status} {route}"
        );
    }

    let (status, stage) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": STAGE, "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case CreateStage: expected 201, got {status} {stage}"
    );
    assert!(
        stage.get("deploymentId").and_then(Value::as_str).is_some(),
        "case CreateStage: an auto-deploying stage must carry a deployment, got {stage}"
    );

    Deployed {
        api_id,
        endpoint_host,
    }
}

/// Assert the emulator really does verify credentials: a modelled control-plane operation
/// carrying the harness's unsigned credential scope is refused by it, with the status and
/// the code the emulator's credential gate answers — not merely with some non-success a
/// routing failure would also produce.
async fn assert_authenticating(emulator: &MultiServiceRouter) {
    let (status, body) = call(emulator, "GET", "/v2/apis", None).await;
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        (status, code),
        (403, CREDENTIAL_GATE_CODE),
        "case \"the control: request authentication is on\": expected the credential gate's \
         403 {CREDENTIAL_GATE_CODE}, got {status} {body}"
    );
}

/// One invoke of the deployed stage: how it is addressed, what it carries, and the answer
/// the api's own records earn it.
struct Row<'c> {
    case: &'c str,
    method: &'c str,
    /// The endpoint the request is SENT to: the api's own, or the emulator's own.
    host: String,
    /// The path as a client writes it, with any presigned query.
    path: String,
    headers: &'c [(&'c str, &'c str)],
    status: u16,
    /// The sentence under the `message` member the answer carries.
    sentence: &'c str,
}

/// Assert one invoke was answered with the status and `{"message": …}` sentence expected.
fn answered(case: &str, answer: &Answer, status: u16, sentence: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected {status}, got {}",
        answer.summary()
    );
    assert_eq!(
        message(answer),
        sentence,
        "case {case:?}: expected the sentence {sentence:?}, got {}",
        answer.summary()
    );
}

#[tokio::test]
async fn every_signed_invoke_of_a_deployed_http_stage_is_answered_by_the_stage() {
    let mut emulator = emulator();
    let deployed = deploy(&emulator).await;

    // From here on the SAME emulator verifies request signatures, as it does when it is
    // started with request authentication on.
    emulator.set_request_authentication(true);
    assert_authenticating(&emulator).await;

    let header = authorization(REGION);
    let other_region = authorization("ap-southeast-2");
    let session: &[(&str, &str)] = &[
        ("authorization", header.as_str()),
        ("x-amz-date", AMZ_DATE),
        ("x-amz-security-token", SESSION_TOKEN),
    ];
    let signed: &[(&str, &str)] = &[("authorization", header.as_str()), ("x-amz-date", AMZ_DATE)];
    let signed_elsewhere: &[(&str, &str)] = &[
        ("authorization", other_region.as_str()),
        ("x-amz-date", AMZ_DATE),
    ];
    let unsigned: &[(&str, &str)] = &[];

    let stage_path = format!("/{STAGE}");
    let rewritten = format!("/_aws/execute-api/{}/{STAGE}", deployed.api_id);
    let rows = vec![
        Row {
            case: "an AWS_IAM route, signed in the Authorization header",
            method: "GET",
            host: deployed.endpoint_host.clone(),
            path: format!("{stage_path}/signed"),
            headers: signed,
            status: INTEGRATION_REACHED,
            sentence: INTEGRATION_REACHED_MESSAGE,
        },
        Row {
            case: "an AWS_IAM route, signed with a temporary credential and its session token",
            method: "GET",
            host: deployed.endpoint_host.clone(),
            path: format!("{stage_path}/signed"),
            headers: session,
            status: INTEGRATION_REACHED,
            sentence: INTEGRATION_REACHED_MESSAGE,
        },
        Row {
            case: "an AWS_IAM route, with a credential scope naming another Region",
            method: "GET",
            host: deployed.endpoint_host.clone(),
            path: format!("{stage_path}/signed"),
            headers: signed_elsewhere,
            status: INTEGRATION_REACHED,
            sentence: INTEGRATION_REACHED_MESSAGE,
        },
        Row {
            case: "an AWS_IAM route, reached as a presigned URL carrying its credential in the \
                   query",
            method: "GET",
            host: deployed.endpoint_host.clone(),
            path: format!("{stage_path}/signed{}", presigned_query(REGION)),
            headers: unsigned,
            status: INTEGRATION_REACHED,
            sentence: INTEGRATION_REACHED_MESSAGE,
        },
        Row {
            case: "an AWS_IAM route, signed and addressed at the rewritten endpoint",
            method: "GET",
            host: OWN_ENDPOINT.to_string(),
            path: format!("{rewritten}/signed"),
            headers: signed,
            status: INTEGRATION_REACHED,
            sentence: INTEGRATION_REACHED_MESSAGE,
        },
        Row {
            case: "a route open to everyone, signed anyway",
            method: "GET",
            host: deployed.endpoint_host.clone(),
            path: format!("{stage_path}/open"),
            headers: signed,
            status: INTEGRATION_REACHED,
            sentence: INTEGRATION_REACHED_MESSAGE,
        },
        Row {
            case: "a route open to everyone, carrying no credential at all",
            method: "GET",
            host: deployed.endpoint_host.clone(),
            path: format!("{stage_path}/open"),
            headers: unsigned,
            status: INTEGRATION_REACHED,
            sentence: INTEGRATION_REACHED_MESSAGE,
        },
        Row {
            case: "an AWS_IAM route reached with no credential at all",
            method: "GET",
            host: deployed.endpoint_host.clone(),
            path: format!("{stage_path}/signed"),
            headers: unsigned,
            status: FORBIDDEN,
            sentence: FORBIDDEN_MESSAGE,
        },
        Row {
            case: "a signed request to a path the api holds no route for",
            method: "GET",
            host: deployed.endpoint_host.clone(),
            path: format!("{stage_path}/absent"),
            headers: signed,
            status: NOT_FOUND,
            sentence: NOT_FOUND_MESSAGE,
        },
        Row {
            case: "a signed request naming a verb no route of the api declares",
            method: "DELETE",
            host: deployed.endpoint_host.clone(),
            path: format!("{stage_path}/open"),
            headers: signed,
            status: NOT_FOUND,
            sentence: NOT_FOUND_MESSAGE,
        },
    ];

    for row in &rows {
        let answer = send(&emulator, &row.host, row.method, &row.path, row.headers).await;
        answered(row.case, &answer, row.status, row.sentence);
    }
}
