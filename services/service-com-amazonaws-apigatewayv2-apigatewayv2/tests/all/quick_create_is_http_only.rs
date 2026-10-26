// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the SCOPE of quick create: which `ProtocolType` the
//! quick-create members actually configure something for, and which requests carrying them
//! are accepted.
//!
//! The rule these cases assert, one level above the recorded case
//! (`CreateApi(ProtocolType=WEBSOCKET, Target=<url>)` answered 201):
//!
//! > A `CreateApi` carrying quick create's members — `Target`, `RouteKey` and the
//! > quick-create `CredentialsArn` — is ACCEPTED (201) whatever `ProtocolType` it names.
//! > Quick create itself is what is HTTP-only ("Supported only for HTTP APIs",
//! > `CreateApi$Target`): on an HTTP api those members produce the set the member documents
//! > (an integration, a catch-all route and the `$default` stage), and on a WEBSOCKET api
//! > they configure nothing at all — the api comes back with zero integrations, zero routes
//! > and zero stages, and no member of the stored api echoes the `Target` back. The one
//! > precondition that is a property of the REQUEST rather than of the protocol still
//! > holds for either: a `RouteKey` with no `Target` names a route of no integration and is
//! > refused 400 `BadRequestException`.
//!
//! The rows span what the rule generalises over: the form of the `Target` (a fully
//! qualified `https` URL, a plain `http` URL, a Lambda function ARN, a qualified Lambda
//! ARN), whether the request also names a `RouteKey` and its form, and whether it carries
//! the quick-create `CredentialsArn`. Each row is sent on BOTH protocol types, so each
//! assertion is about the protocol and not about the row.
//!
//! What this suite adds over its neighbours: `quick_create_servable_set` pins WHAT quick
//! create produces for an HTTP api, and `protocol_type_gating` pins the members that really
//! are refused on the other protocol; neither says which requests quick create's own
//! members are accepted in.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    HTTP_PROTOCOL_TYPE, WEBSOCKET_PROTOCOL_TYPE,
};

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// The `StageName` quick create mints its stage under.
const DEFAULT_STAGE_NAME: &str = "$default";

/// One row: the quick-create members a `CreateApi` carries, beside the api's name and
/// protocol.
struct Row {
    case: &'static str,
    /// The members under test, merged into the request for each protocol.
    members: Value,
    /// How many routes/integrations/stages quick create produces from them for an HTTP
    /// api: one of each for every row here, since each names a `Target`.
    quick_created: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: a fully qualified https URL and nothing else",
            members: json!({ "target": "https://example.com/backend" }),
            quick_created: 1,
        },
        Row {
            case: "a plain http URL",
            members: json!({ "target": "http://example.com/legacy" }),
            quick_created: 1,
        },
        Row {
            case: "a Lambda function ARN",
            members: json!({
                "target": "arn:aws:lambda:eu-west-1:000000000000:function:handler"
            }),
            quick_created: 1,
        },
        Row {
            case: "a Target with an explicit RouteKey",
            members: json!({
                "target": "https://example.com/backend",
                "routeKey": "GET /items"
            }),
            quick_created: 1,
        },
        Row {
            case: "a qualified Lambda ARN with a RouteKey and the quick-create \
                   CredentialsArn",
            members: json!({
                "target": "arn:aws:lambda:eu-west-1:000000000000:function:handler:3",
                "routeKey": "ANY /{proxy+}",
                "credentialsArn": "arn:aws:iam::000000000000:role/apigw-invoke"
            }),
            quick_created: 1,
        },
    ]
}

/// The row's members in a `CreateApi` request for an api of `protocol_type`.
fn request(row: &Row, protocol_type: &str, name: &str) -> Value {
    let mut body = json!({ "name": name, "protocolType": protocol_type });
    if protocol_type == WEBSOCKET_PROTOCOL_TYPE {
        // "This property is required for WebSocket APIs"
        // (`CreateApi$RouteSelectionExpression`).
        body["routeSelectionExpression"] = json!("$request.body.action");
    }
    for (member, value) in row.members.as_object().expect("the row's members") {
        body[member.as_str()] = value.clone();
    }
    body
}

/// How many items `GET <uri>` lists.
async fn listed(emulator: &MultiServiceRouter, case: &str, uri: &str) -> usize {
    let (status, body) = call(emulator, "GET", uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GET {uri}, got {status} {body}"
    );
    body["items"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_else(|| panic!("case {case:?}: expected GET {uri} to report items, got {body}"))
}

#[tokio::test]
async fn quick_create_members_are_accepted_on_either_protocol_and_configure_only_an_http_api() {
    for (index, row) in rows().into_iter().enumerate() {
        for protocol_type in [HTTP_PROTOCOL_TYPE, WEBSOCKET_PROTOCOL_TYPE] {
            let case = format!("{} on a {protocol_type} api", row.case);
            // One emulator per api, so what the api holds is attributable to this one
            // call.
            let emulator = emulator();
            let (status, created) = call(
                &emulator,
                "POST",
                "/v2/apis",
                Some(request(&row, protocol_type, &format!("quick-{index}"))),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from CreateApi, got {status} {created}"
            );
            // `Target` is no member of the modelled api, so nothing echoes it back on
            // either protocol.
            assert!(
                created.get("target").is_none(),
                "case {case:?}: no member of the created api echoes the Target, got {created}"
            );
            let api_id = as_str(&created, "apiId");

            // Quick create ran for an HTTP api and for nothing else.
            let expected = if protocol_type == HTTP_PROTOCOL_TYPE {
                row.quick_created
            } else {
                0
            };
            for collection in ["routes", "integrations"] {
                let held = listed(
                    &emulator,
                    &case,
                    &format!("/v2/apis/{api_id}/{collection}"),
                )
                .await;
                assert_eq!(
                    held, expected,
                    "case {case:?}: expected {expected} {collection} for a {protocol_type} \
                     api, got {held}"
                );
            }
            // The `$default` stage, read back one by one: `GetStages` lists a stage of
            // either protocol, so the count above is asserted here through the stage quick
            // create would have minted.
            let (status, stage) = call(
                &emulator,
                "GET",
                &format!("/v2/apis/{api_id}/stages/{DEFAULT_STAGE_NAME}"),
                None,
            )
            .await;
            if protocol_type == HTTP_PROTOCOL_TYPE {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected quick create's $default stage, got {status} {stage}"
                );
            } else {
                assert_eq!(
                    status, 404,
                    "case {case:?}: a WEBSOCKET api gets no quick-create stage, got \
                     {status} {stage}"
                );
                assert_eq!(
                    error_code(&stage),
                    "NotFoundException",
                    "case {case:?}: expected NotFoundException, got {stage}"
                );
            }

            // And the api itself reads back, on either protocol: the request was accepted,
            // not half-applied.
            let (status, fetched) = call(&emulator, "GET", &format!("/v2/apis/{api_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetApi, got {status} {fetched}"
            );
            assert_eq!(
                as_str(&fetched, "protocolType"),
                protocol_type,
                "case {case:?}: expected the stored api to carry its own protocol type, \
                 got {fetched}"
            );
            assert!(
                fetched.get("target").is_none(),
                "case {case:?}: GetApi carries no Target member, got {fetched}"
            );
        }
    }
}

/// The one precondition that is a property of the REQUEST: a `RouteKey` names the route
/// quick create mints, so without a `Target` there is no integration for it to front. It
/// holds on an api of either protocol, since neither has a quick create to run.
#[tokio::test]
async fn a_route_key_with_no_target_is_refused_on_either_protocol() {
    let emulator = emulator();

    for protocol_type in [HTTP_PROTOCOL_TYPE, WEBSOCKET_PROTOCOL_TYPE] {
        let case = format!("a RouteKey with no Target on a {protocol_type} api");
        let row = Row {
            case: "route key only",
            members: json!({ "routeKey": "GET /items" }),
            quick_created: 0,
        };
        let (status, body) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(request(&row, protocol_type, "route-key-only")),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateApi, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        // The sentence API Gateway answers this request with names the member the request
        // is missing.
        assert_eq!(
            as_str(&body, "message"),
            "Target is a required property in this context",
            "case {case:?}: expected the refusal to name the missing Target, got {body}"
        );
    }

    // Nothing was persisted by either refusal.
    assert_eq!(
        listed(&emulator, "the refused creates", "/v2/apis").await,
        0,
        "a refused CreateApi must persist nothing"
    );
}

/// The other quick-create precondition: a `Target` quick create BUILDS an integration from
/// names one of the two backends it can proxy to — "For HTTP integrations, specify a fully
/// qualified URL. For Lambda integrations, specify a function ARN. The type of the
/// integration will be HTTP_PROXY or AWS_PROXY, respectively" (`CreateApi$Target`) — and a
/// value of any other form is refused 400 with the sentence API Gateway states the rule in.
///
/// The rows span the forms: both URL schemes, a bare and a qualified Lambda function ARN,
/// an ARN of another AWS service, an ARN of another Lambda resource, a URL of a scheme no
/// integration speaks, a host with no scheme at all, and a scheme naming no host.
#[tokio::test]
async fn a_target_that_selects_neither_proxy_is_refused() {
    // (case, the Target, whether quick create can build from it)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a fully qualified https URL", "https://example.com/backend", true),
        ("a plain http URL", "http://example.com/legacy", true),
        (
            "a Lambda function ARN",
            "arn:aws:lambda:eu-west-1:000000000000:function:handler",
            true,
        ),
        (
            "a Lambda function ARN with a version qualifier",
            "arn:aws:lambda:eu-west-1:000000000000:function:handler:3",
            true,
        ),
        (
            "a Lambda function ARN in another partition",
            "arn:aws-cn:lambda:cn-north-1:000000000000:function:handler",
            true,
        ),
        // Not a proxy quick create can build: another service's resource, another Lambda
        // resource type, and values that are no backend address at all.
        (
            "an SQS queue ARN",
            "arn:aws:sqs:eu-west-1:000000000000:orders",
            false,
        ),
        (
            "an IAM role ARN",
            "arn:aws:iam::000000000000:role/apigw",
            false,
        ),
        (
            "a Lambda LAYER ARN, which is no function",
            "arn:aws:lambda:eu-west-1:000000000000:layer:shared:1",
            false,
        ),
        (
            "a Lambda ARN naming no function at all",
            "arn:aws:lambda:eu-west-1:000000000000:function:",
            false,
        ),
        ("a scheme no integration speaks", "ftp://example.com/drop", false),
        ("a host with no scheme", "example.com/backend", false),
        ("a scheme naming no host", "https://", false),
    ];

    for (index, (case, target, buildable)) in rows.into_iter().enumerate() {
        // One emulator per row, so `GetApis` lists exactly what this row created.
        let emulator = emulator();
        let (status, body) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(json!({
                "name": format!("target-form-{index}"),
                "protocolType": HTTP_PROTOCOL_TYPE,
                "target": target
            })),
        )
        .await;
        if buildable {
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 from CreateApi for {target:?}, got {status} {body}"
            );
            assert_eq!(
                listed(&emulator, case, "/v2/apis").await,
                1,
                "case {case:?}: expected the accepted CreateApi to have minted an api"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateApi for {target:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            "Target only supports HTTP Proxy or Lambda Proxy",
            "case {case:?}: expected the refusal to state the rule, got {body}"
        );
        assert_eq!(
            listed(&emulator, case, "/v2/apis").await,
            0,
            "case {case:?}: a refused CreateApi must persist nothing"
        );
    }
}
