// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SIGNED half of the `execute-api` data plane: what a deployed
//! REST stage answers a request that carries a SigV4 credential with, on an emulator that
//! verifies request signatures.
//!
//! The RULE under test, over a SET of requests rather than one recorded call: every request
//! addressed to a deployed stage is answered by THE STAGE, from the api's own records —
//! the integration's answer, or the gateway response for the refusal the records earn it —
//! whatever credential the request carries and whichever signing name that credential's
//! scope names. An invoke of a deployed stage is signed under the data plane's own signing
//! name (the `execute-api` service AWS publishes the invoke endpoint and the
//! `execute-api:Invoke` action under), which is not the signing name of this service's
//! control plane, so a credential gate keyed to the control plane's name would refuse every
//! correctly signed invoke before the stage was reached — and a client could not tell a
//! refused backend from an endpoint that answers nothing.
//!
//! The rows span what the rule generalises over: WHICH credential form the request carries
//! (an `Authorization` header, a session credential with its security token, a presigned
//! URL's query credential, a scope naming another Region, no credential at all), WHICH
//! `authorizationType` the deployed method declares (`AWS_IAM` and `NONE`), WHICH
//! integration is behind it (a `MOCK` that is served in full, and the `AWS_PROXY` Lambda
//! proxy whose backend this emulator cannot call), WHICH verb and path the request names
//! (a deployed one, a verb the method does not declare, a path the deployment holds no
//! resource for), and HOW the request addresses the api (the api's own invoke host, and the
//! reserved data-plane path prefix on the emulator's own endpoint).
//!
//! Authentication is genuinely ON for the rows: the control at the top of each test drives
//! a modelled control-plane operation through the same router and asserts it is refused,
//! which is what makes the served rows evidence of anything.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service, with the control plane driven through the same emulator
//! ([`Gateway`]) — so what is asserted is what a client of the published invoke URL sees.

use serde_json::{json, Value};

use crate::execute_api_data_plane::{
    deploy, integration_response, invoke_on_host, json_template, method_response, put_integration,
    put_method, Invoked,
};
use crate::rest_api_lifecycle::{create_api, resource_under, Gateway};
use crate::test_invoke_method::lambda_uri;

/// The stage every api below is deployed to.
const STAGE: &str = "beta";

/// The Region the api's own invoke host is addressed in.
const REGION: &str = "us-east-1";

/// The signing name an invoke of a deployed stage is signed under: the `execute-api`
/// service, which AWS publishes the invoke endpoint and the `execute-api:Invoke` action
/// under. NOT this service's own signing name, which is its control plane's (`apigateway`).
const INVOKE_SIGNING_NAME: &str = "execute-api";

/// The reserved path prefix that names this data plane and the api on the one endpoint this
/// emulator serves its whole surface at, from `crate::service::invoke`'s module docs:
/// `/_aws/execute-api/{api-id}/{stage}/{resource-path}`.
const DATA_PLANE_PREFIX: &str = "/_aws/execute-api";

/// The emulator's own endpoint, which a client that resolves the api's invoke host nowhere
/// sends as the `Host` instead.
const EMULATOR_ENDPOINT: &str = "localhost:9297";

/// The sentence the uncustomized `API_CONFIGURATION_ERROR` gateway response carries, which
/// is what an integration whose backend this emulator cannot call is answered with.
const CONFIGURATION_ERROR: &str = "Internal server error";

/// The sentence a request the deployment places no method for, and an `AWS_IAM` method
/// reached with no credential at all, are answered with.
const MISSING_TOKEN: &str = "Missing Authentication Token";

/// The body the `MOCK` route open to everyone answers.
const OPEN_BODY: &str = r#"{"served":"open"}"#;

/// The body the `MOCK` route whose `authorizationType` is `AWS_IAM` answers.
const IAM_BODY: &str = r#"{"served":"iam"}"#;

/// The header this data plane names the request it answered in, which `service_common`
/// owns: every response the stage's own serving funnel produces carries one.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The code the emulator's own credential gate refuses an unverifiable request with — the
/// answer that must never stand in front of the data plane. It is the gate's, not AWS's:
/// the control below asserts it on a MODELLED operation, which is where it belongs.
const CREDENTIAL_GATE_CODE: &str = "AccessDenied";

/// A value in the shape of a SigV4 signature: 64 lowercase hex characters, as
/// `AWS4-HMAC-SHA256` produces. Nothing about these tests depends on the value — what they
/// assert is that the STAGE decides the answer — so one shape is enough.
const SIGNATURE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A value in the shape of a session token, as temporary credentials carry one.
const SESSION_TOKEN: &str = "FwoGZXIvYXdzEBYaDEVNVUxBVE9SU0VTU0lPTg";

/// The `x-amz-date` a header-signed request carries, in the basic ISO 8601 form SigV4 uses.
const AMZ_DATE: &str = "20260101T000000Z";

/// The date part of a credential scope, as SigV4 writes it.
const SCOPE_DATE: &str = "20260101";

/// How a request carries its AWS credential, which is what each row varies.
#[derive(Clone, Copy, Debug)]
enum Credential {
    /// No credential of any kind: the request a client of an open route sends.
    Absent,
    /// The `Authorization` header a signed SDK request carries, scoped to `region`.
    Header {
        region: &'static str,
        /// Whether the credential is a temporary one, which travels with its session
        /// token in `x-amz-security-token`.
        session: bool,
    },
    /// The query credential a presigned URL carries, scoped to `region`.
    PresignedQuery { region: &'static str },
}

/// How the request addresses the api, both of which `crate::service::invoke` resolves.
#[derive(Clone, Copy, Debug)]
enum Addressing {
    /// The api's own invoke host, as `RestApi$disableExecuteApiEndpoint` documents it:
    /// `{api-id}.execute-api.{region}.amazonaws.com`.
    OwnHost,
    /// The emulator's own endpoint, with the api id behind the reserved data-plane path
    /// prefix — what a client that resolves the invoke host nowhere uses.
    EmulatorPath,
}

/// What the api's records earn one request.
#[derive(Clone, Copy, Debug)]
enum Answer {
    /// The integration answered: this status and this body, verbatim.
    Served(u16, &'static str),
    /// The api answered through a gateway response: this status, and this sentence under
    /// the `message` member every gateway response body carries.
    Gateway(u16, &'static str),
}

/// One invoke of the deployed stage: how it is addressed, what it carries, and the answer
/// the api's own records earn it.
#[derive(Clone, Copy, Debug)]
struct Row {
    case: &'static str,
    verb: &'static str,
    /// The path WITHIN the api, as a client of the published invoke URL writes it:
    /// `/{stage}/{resource-path}`.
    path: &'static str,
    credential: Credential,
    addressing: Addressing,
    answer: Answer,
}

/// Issue one row's request through `gateway`'s router and read the whole answer.
///
/// The request is built and read by the crate's own data-plane helper
/// ([`invoke_on_host`]), so this suite and `execute_api_data_plane` cannot drift about what
/// "the answer" is; what a row varies — the host, the path, the presigned query and the
/// credential headers — is all that is computed here.
async fn invoke(gateway: &Gateway, api: &str, row: &Row) -> Invoked {
    let (host, path) = match row.addressing {
        Addressing::OwnHost => (
            format!("{api}.execute-api.{REGION}.amazonaws.com"),
            row.path.to_string(),
        ),
        Addressing::EmulatorPath => (
            EMULATOR_ENDPOINT.to_string(),
            format!("{DATA_PLANE_PREFIX}/{api}{}", row.path),
        ),
    };
    let query = match row.credential {
        Credential::PresignedQuery { region } => format!(
            "?X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential=AKIDEXAMPLE%2F{SCOPE_DATE}%2F{region}%2F{INVOKE_SIGNING_NAME}\
             %2Faws4_request\
             &X-Amz-Date={AMZ_DATE}&X-Amz-Expires=300&X-Amz-SignedHeaders=host\
             &X-Amz-Signature={SIGNATURE}"
        ),
        _ => String::new(),
    };
    let authorization = match row.credential {
        Credential::Header { region, .. } => Some(format!(
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/{SCOPE_DATE}/{region}/{INVOKE_SIGNING_NAME}/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature={SIGNATURE}"
        )),
        _ => None,
    };
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(authorization) = &authorization {
        headers.push(("authorization", authorization.as_str()));
        headers.push(("x-amz-date", AMZ_DATE));
    }
    if matches!(row.credential, Credential::Header { session: true, .. }) {
        headers.push(("x-amz-security-token", SESSION_TOKEN));
    }
    invoke_on_host(
        gateway,
        &host,
        row.verb,
        &format!("{path}{query}"),
        &headers,
    )
    .await
}

/// A `MOCK` route under `root` answering `body` for `verb`, with `authorization` as its
/// method's `authorizationType`.
async fn mock_route(
    gateway: &Gateway,
    api: &str,
    root: &str,
    path_part: &str,
    verb: &str,
    authorization: &str,
    body: &str,
) {
    let resource = resource_under(gateway, api, root, path_part).await;
    put_method(
        gateway,
        api,
        &resource,
        verb,
        json!({ "authorizationType": authorization }),
    )
    .await;
    put_integration(gateway, api, &resource, verb, json!({ "type": "MOCK" })).await;
    method_response(gateway, api, &resource, verb, "200", json!({})).await;
    integration_response(
        gateway,
        api,
        &resource,
        verb,
        "200",
        json!({ "responseTemplates": json_template(body) }),
    )
    .await;
}

/// A route under `root` whose integration is the Lambda PROXY — the backend this emulator
/// cannot call — with `authorization` as its method's `authorizationType`.
async fn proxy_route(
    gateway: &Gateway,
    api: &str,
    root: &str,
    path_part: &str,
    verb: &str,
    authorization: &str,
) {
    let resource = resource_under(gateway, api, root, path_part).await;
    put_method(
        gateway,
        api,
        &resource,
        verb,
        json!({ "authorizationType": authorization }),
    )
    .await;
    put_integration(
        gateway,
        api,
        &resource,
        verb,
        json!({ "type": "AWS_PROXY", "httpMethod": "POST", "uri": lambda_uri() }),
    )
    .await;
}

/// Assert that the router this `gateway` now drives really does verify credentials: a
/// modelled control-plane operation carrying the harness's unsigned credential scope is
/// refused by it, with the status and the code the emulator's credential gate answers —
/// not merely with some non-success a routing failure would also produce.
async fn assert_authenticating(gateway: &Gateway) {
    let refused = gateway.call("GET", "/restapis", None).await;
    let code = refused
        .error
        .clone()
        .or_else(|| {
            refused
                .body
                .get("__type")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    assert_eq!(
        (refused.status, code.as_str()),
        (403, CREDENTIAL_GATE_CODE),
        "case \"the control: request authentication is on\": expected the credential gate's \
         403 {CREDENTIAL_GATE_CODE}, got {} with {:?}",
        refused.status,
        refused.body
    );
}

#[tokio::test]
async fn every_signed_invoke_of_a_deployed_stage_is_answered_by_the_stage() {
    let mut gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "signed-invoke").await;

    // Four deployed methods: each `authorizationType` the data plane admits a signed
    // request under, in front of each of the two integrations whose answer is observable —
    // the `MOCK` that is served in full, and the Lambda proxy whose backend this emulator
    // cannot call, which is answered as an integration that could not be performed.
    mock_route(&gateway, &api, &root, "open", "GET", "NONE", OPEN_BODY).await;
    mock_route(&gateway, &api, &root, "signed", "GET", "AWS_IAM", IAM_BODY).await;
    proxy_route(&gateway, &api, &root, "backend", "GET", "AWS_IAM").await;
    proxy_route(&gateway, &api, &root, "open-backend", "GET", "NONE").await;
    deploy(&gateway, &api, STAGE).await;

    // From here on the SAME router verifies request signatures, as the emulator does when
    // it is started with request authentication on.
    gateway.router.set_request_authentication(true);
    assert_authenticating(&gateway).await;

    let rows = [
        Row {
            case: "an IAM-authorized Lambda proxy method, signed in the Authorization header",
            verb: "GET",
            path: "/beta/backend",
            credential: Credential::Header {
                region: REGION,
                session: false,
            },
            addressing: Addressing::OwnHost,
            answer: Answer::Gateway(500, CONFIGURATION_ERROR),
        },
        Row {
            case: "the same, signed with a temporary credential and its session token",
            verb: "GET",
            path: "/beta/backend",
            credential: Credential::Header {
                region: REGION,
                session: true,
            },
            addressing: Addressing::OwnHost,
            answer: Answer::Gateway(500, CONFIGURATION_ERROR),
        },
        Row {
            case: "the same, with a credential scope naming another Region",
            verb: "GET",
            path: "/beta/backend",
            credential: Credential::Header {
                region: "ap-southeast-2",
                session: false,
            },
            addressing: Addressing::OwnHost,
            answer: Answer::Gateway(500, CONFIGURATION_ERROR),
        },
        Row {
            case: "the same, reached as a presigned URL carrying its credential in the query",
            verb: "GET",
            path: "/beta/backend",
            credential: Credential::PresignedQuery { region: REGION },
            addressing: Addressing::OwnHost,
            answer: Answer::Gateway(500, CONFIGURATION_ERROR),
        },
        Row {
            case: "the same, addressed through the data-plane path prefix",
            verb: "GET",
            path: "/beta/backend",
            credential: Credential::Header {
                region: REGION,
                session: false,
            },
            addressing: Addressing::EmulatorPath,
            answer: Answer::Gateway(500, CONFIGURATION_ERROR),
        },
        Row {
            case: "a Lambda proxy method open to everyone, signed anyway",
            verb: "GET",
            path: "/beta/open-backend",
            credential: Credential::Header {
                region: REGION,
                session: false,
            },
            addressing: Addressing::OwnHost,
            answer: Answer::Gateway(500, CONFIGURATION_ERROR),
        },
        Row {
            case: "an IAM-authorized MOCK method, signed in the Authorization header",
            verb: "GET",
            path: "/beta/signed",
            credential: Credential::Header {
                region: REGION,
                session: false,
            },
            addressing: Addressing::OwnHost,
            answer: Answer::Served(200, IAM_BODY),
        },
        Row {
            case: "an IAM-authorized MOCK method, reached as a presigned URL",
            verb: "GET",
            path: "/beta/signed",
            credential: Credential::PresignedQuery { region: REGION },
            addressing: Addressing::OwnHost,
            answer: Answer::Served(200, IAM_BODY),
        },
        Row {
            case: "an IAM-authorized MOCK method, addressed through the data-plane path prefix",
            verb: "GET",
            path: "/beta/signed",
            credential: Credential::Header {
                region: REGION,
                session: true,
            },
            addressing: Addressing::EmulatorPath,
            answer: Answer::Served(200, IAM_BODY),
        },
        Row {
            case: "a method open to everyone, signed anyway",
            verb: "GET",
            path: "/beta/open",
            credential: Credential::Header {
                region: REGION,
                session: false,
            },
            addressing: Addressing::OwnHost,
            answer: Answer::Served(200, OPEN_BODY),
        },
        Row {
            case: "a method open to everyone, carrying no credential at all",
            verb: "GET",
            path: "/beta/open",
            credential: Credential::Absent,
            addressing: Addressing::OwnHost,
            answer: Answer::Served(200, OPEN_BODY),
        },
        Row {
            case: "an IAM-authorized method reached with no credential at all",
            verb: "GET",
            path: "/beta/signed",
            credential: Credential::Absent,
            addressing: Addressing::OwnHost,
            answer: Answer::Gateway(403, MISSING_TOKEN),
        },
        Row {
            case: "a signed request to a path the deployment holds no resource for",
            verb: "GET",
            path: "/beta/absent",
            credential: Credential::Header {
                region: REGION,
                session: false,
            },
            addressing: Addressing::OwnHost,
            answer: Answer::Gateway(403, MISSING_TOKEN),
        },
        Row {
            case: "a signed request naming a verb the deployed method does not declare",
            verb: "POST",
            path: "/beta/signed",
            credential: Credential::Header {
                region: REGION,
                session: false,
            },
            addressing: Addressing::OwnHost,
            answer: Answer::Gateway(403, MISSING_TOKEN),
        },
        Row {
            case: "a signed request naming a stage this api holds no deployment of",
            verb: "GET",
            path: "/prod/signed",
            credential: Credential::Header {
                region: REGION,
                session: false,
            },
            addressing: Addressing::OwnHost,
            answer: Answer::Gateway(403, "Forbidden"),
        },
    ];

    for row in &rows {
        let answered = invoke(&gateway, &api, row).await;
        match row.answer {
            Answer::Served(status, body) => {
                assert_eq!(
                    answered.status, status,
                    "case {:?}: expected the integration's own {status}, got {} with {:?}",
                    row.case, answered.status, answered.body
                );
                assert_eq!(
                    answered.body, body,
                    "case {:?}: expected the integration's own body {body:?}, got {:?}",
                    row.case, answered.body
                );
            }
            Answer::Gateway(status, message) => {
                assert_eq!(
                    answered.status, status,
                    "case {:?}: expected the api's own gateway response {status}, got {} with \
                     {:?}",
                    row.case, answered.status, answered.body
                );
                assert_eq!(
                    answered.message().as_deref(),
                    Some(message),
                    "case {:?}: expected the gateway response sentence {message:?}, got {:?}",
                    row.case,
                    answered.body
                );
            }
        }
        // Whatever the outcome, it came out of the stage's own serving funnel, which names
        // the request it answered. A refusal taken in front of the data plane carries no
        // such id.
        assert!(
            answered.header(REQUEST_ID_HEADER).is_some(),
            "case {:?}: expected the stage's answer to name the request it answered, got {:?}",
            row.case,
            answered.body
        );
    }
}

#[tokio::test]
async fn a_signed_invoke_reads_the_api_record_the_control_plane_wrote() {
    // The round trip: what a signed invoke is answered with follows the records, so a
    // change the control plane makes to the deployed method is what the next signed invoke
    // reads — the same credential, two answers.
    let mut gateway = Gateway::new();
    let (api, root) = create_api(&gateway, "signed-invoke-records").await;
    let resource = resource_under(&gateway, &api, &root, "items").await;
    put_method(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "authorizationType": "AWS_IAM" }),
    )
    .await;
    put_integration(
        &gateway,
        &api,
        &resource,
        "GET",
        json!({ "type": "AWS_PROXY", "httpMethod": "POST", "uri": lambda_uri() }),
    )
    .await;
    deploy(&gateway, &api, STAGE).await;

    gateway.router.set_request_authentication(true);
    assert_authenticating(&gateway).await;

    let signed = Row {
        case: "the proxy integration the deployment holds",
        verb: "GET",
        path: "/beta/items",
        credential: Credential::Header {
            region: REGION,
            session: false,
        },
        addressing: Addressing::OwnHost,
        answer: Answer::Gateway(500, CONFIGURATION_ERROR),
    };
    let answered = invoke(&gateway, &api, &signed).await;
    assert_eq!(
        (answered.status, answered.message().as_deref()),
        (500, Some(CONFIGURATION_ERROR)),
        "case {:?}: expected the integration-failure gateway response, got {} with {:?}",
        signed.case,
        answered.status,
        answered.body
    );

    // The developer customizes that gateway response, and the next signed invoke — byte
    // for byte the same request — is answered the customization. A memorised answer could
    // not follow the record. The control plane is driven with authentication off, as every
    // other suite drives it; the invokes are the requests the authenticating router serves.
    gateway.router.set_request_authentication(false);
    let customized = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/gatewayresponses/API_CONFIGURATION_ERROR"),
            Some(json!({
                "statusCode": "502",
                "responseTemplates": json_template(
                    r#"{"message":"the backend did not answer","customized":true}"#,
                ),
            })),
        )
        .await;
    assert_eq!(
        customized.status, 201,
        "case PutGatewayResponse(API_CONFIGURATION_ERROR): expected 201, got {} with {:?}",
        customized.status, customized.body
    );
    gateway.router.set_request_authentication(true);
    assert_authenticating(&gateway).await;

    let answered = invoke(&gateway, &api, &signed).await;
    assert_eq!(
        answered.status, 502,
        "case {:?}: expected the customized status the api now holds, got {} with {:?}",
        signed.case, answered.status, answered.body
    );
    assert_eq!(
        answered.message().as_deref(),
        Some("the backend did not answer"),
        "case {:?}: expected the customized sentence the api now holds, got {:?}",
        signed.case,
        answered.body
    );
}
