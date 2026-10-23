// SPDX-License-Identifier: CC0-1.0

//! The `awsQuery` error envelope this endpoint answers with, over every layer that can
//! answer one.
//!
//! Elastic Load Balancing speaks `awsQuery` (`smithy_query(query="service_info")` on either
//! model), and AWS answers an `awsQuery` error as `text/xml`:
//!
//! ```xml
//! <ErrorResponse>
//!   <Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error>
//!   <RequestId>…</RequestId>
//! </ErrorResponse>
//! ```
//!
//! The issue this suite belongs to reported the opposite: every request to this endpoint
//! was refused by the router with an awsJson `{"__type":"AmbiguousService"}` body, which a
//! query-protocol client cannot parse at all (`botocore.parsers.ResponseParserError:
//! invalid XML received`) — so it never read a code, let alone a request id.
//!
//! The rule pinned here, over a TABLE of refusals rather than the one recorded request: an
//! error answered for this endpoint is the WHOLE envelope above — an error status,
//! `content-type: text/xml`, `<Type>`, a `<Code>`, a `<Message>` and a per-response
//! `<RequestId>` — whichever layer produced it, and never an awsJson document. The rows
//! span the layers, because each renders its own response:
//!
//! * a modelled error a handler returned (both models), whose `<Code>` is the model's own
//!   `awsQueryError` code;
//! * a framework refusal raised inside a matched operation (a value outside a declared
//!   bound), which the framework words itself;
//! * a routing failure — an `Action` the serving model does not declare — rendered by the
//!   framework's own renderer through this service's wrapped definition;
//! * the action/version refusals this shared endpoint renders itself
//!   (`src/service/api_version.rs` in the Classic crate).
//!
//! `<RequestId>` is the element the three layers disagreed about: no model declares it, so
//! the framework's error renderers do not write it, and an SDK reports it as the failed
//! call's request id. It is minted once per response, which is why the suite also pins
//! that every answer carries exactly one and that no two answers share one.
//!
//! Both services are mounted on one router over one engine, exactly as the production
//! router assembles them, so the rows reach the two models through the ONE shared
//! `elasticloadbalancing` front door as an AWS SDK client does — and through this model's
//! own routing key, where the front door is not involved.

use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService as ClassicService;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService as LoadBalancingV2Service;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

// The wire helpers this crate's suites share, from a sibling module of this one test
// binary: one form encoder, one field constructor and one element reader, so no two suites
// drift on encoding or on how an element is read.
use super::provisioning_loop::{encode, first_text, p};

/// The signing name, endpoint prefix and ARN namespace both models declare, and so the
/// routing key an AWS SDK client of either API sends.
const SHARED_KEY: &str = "elasticloadbalancing";

/// The routing key that names the ELBv2 model directly, without the shared front door.
const V2_KEY: &str = "elasticloadbalancingv2";

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// This model's API version, read from the constant the front door that owns the shared key
/// routes on, so the suite cannot drift from the rule it tests.
const V2_VERSION: &str =
    service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::service::api_version::V2_API_VERSION;

/// The Classic Load Balancer model's API version, read from the same place for the same
/// reason.
const CLASSIC_VERSION: &str =
    service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::service::api_version::CLASSIC_API_VERSION;

/// One answer from the emulator: what a client reads before it parses anything.
struct Answer {
    status: u16,
    content_type: String,
    /// The `x-amzn-requestid` header, which AWS reports the envelope's request id in too.
    request_id_header: String,
    body: String,
}

/// The emulator under test: ONE shared engine with BOTH Elastic Load Balancing services
/// mounted on the multi-service router, as the production router assembles them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(ClassicService::new(Arc::clone(&engine)))
        .register_service(LoadBalancingV2Service::new(engine));
    router
}

/// Send one `awsQuery` request: the routing key it is signed for, the `Action`, the
/// `Version` its envelope names (or none at all), and the operation's own form fields.
async fn call(
    router: &MultiServiceRouter,
    key: &str,
    action: &str,
    version: Option<&str>,
    params: &[(String, String)],
) -> Answer {
    let mut fields = vec![p("Action", action)];
    if let Some(version) = version {
        fields.push(p("Version", version));
    }
    fields.extend(params.iter().cloned());
    let body = fields
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    let request = http::Request::builder()
        .method("POST")
        .uri("https://elasticloadbalancing.us-west-2.amazonaws.com/")
        .header("host", "elasticloadbalancing.us-west-2.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{key}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    Answer {
        status: response.status().as_u16(),
        content_type: header("content-type"),
        request_id_header: header("x-amzn-requestid"),
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// One refusal row: how the envelope is addressed, and what the endpoint must answer.
struct Row {
    case: &'static str,
    /// The routing key the request is signed for.
    key: &'static str,
    action: &'static str,
    /// The `Version` the envelope names, or `None` for an envelope that names none.
    version: Option<&'static str>,
    params: Vec<(String, String)>,
    status: u16,
    /// The `awsQuery` `<Code>`, for a row whose code a model's `awsQueryError` trait or
    /// this endpoint's own action/version rule names. `None` where the code is the
    /// framework's own wording for a refusal no model describes — the row then pins only
    /// that a non-empty code is reported, which is what a client branches on.
    code: Option<&'static str>,
}

/// How many times `needle` occurs in `text`.
fn occurrences(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form of an AWS request id.
fn is_request_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()))
}

#[tokio::test]
async fn every_layer_answers_the_whole_aws_query_error_envelope() {
    let router = emulator();
    let absent_target_group = format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:targetgroup/no-such-group/0123456789abcdef",
        emulator_server::emulator_account_id()
    );
    let target_group = |name: &str, port: &str| {
        vec![
            p("Name", name),
            p("Protocol", "HTTP"),
            p("Port", port),
            p("VpcId", "vpc-12345678"),
        ]
    };

    // The live target group a later row collides with, created through the shared front
    // door the way an AWS SDK ELBv2 client reaches this model. A SUCCESS, so it also pins
    // that the success envelope is untouched by the error one: its request id lives in
    // `ResponseMetadata`, it carries no `<ErrorResponse>`, and it carries exactly one id.
    let created = call(
        &router,
        SHARED_KEY,
        "CreateTargetGroup",
        Some(V2_VERSION),
        &target_group("envelope-tg", "80"),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"the live target group a collision needs\": expected HTTP 200, got {} with {}",
        created.status, created.body
    );
    assert_eq!(
        created.content_type, "text/xml",
        "case \"the live target group a collision needs\": expected content-type text/xml, got \
         {:?}",
        created.content_type
    );
    assert!(
        !created.body.contains("<ErrorResponse"),
        "case \"the live target group a collision needs\": a success is no error envelope, got {}",
        created.body
    );
    assert_eq!(
        occurrences(&created.body, "<RequestId>"),
        1,
        "case \"the live target group a collision needs\": expected exactly one <RequestId>, got {}",
        created.body
    );
    assert!(
        created.body.contains("<ResponseMetadata>"),
        "case \"the live target group a collision needs\": a success carries its request id in \
         ResponseMetadata, got {}",
        created.body
    );

    let rows: Vec<Row> = vec![
        Row {
            // The issue's own measured oracle: a well-formed ARN nothing holds, asked for
            // through the shared front door.
            case: "a modelled not-found through the shared front door",
            key: SHARED_KEY,
            action: "DescribeTargetGroups",
            version: Some(V2_VERSION),
            params: vec![p("TargetGroupArns.member.1", &absent_target_group)],
            status: 400,
            code: Some("TargetGroupNotFound"),
        },
        Row {
            // The same modelled error on this model's own key, by name rather than by ARN:
            // the envelope cannot depend on how the resource was addressed.
            case: "a modelled not-found on this model's own key",
            key: V2_KEY,
            action: "DescribeTargetGroups",
            version: Some(V2_VERSION),
            params: vec![p("Names.member.1", "no-such-group")],
            status: 400,
            code: Some("TargetGroupNotFound"),
        },
        Row {
            // The issue's third measured oracle: the live name above, reused with a
            // different `Port`, which is NOT the idempotent replay.
            case: "a live target-group name reused with a different port",
            key: SHARED_KEY,
            action: "CreateTargetGroup",
            version: Some(V2_VERSION),
            params: target_group("envelope-tg", "8080"),
            status: 400,
            code: Some("DuplicateTargetGroupName"),
        },
        Row {
            // A second modelled error of the same operation, so the row set does not pin
            // one error shape's serialization only.
            case: "health checks disabled on an instance target group",
            key: V2_KEY,
            action: "CreateTargetGroup",
            version: Some(V2_VERSION),
            params: {
                let mut fields = target_group("envelope-tg-2", "80");
                fields.push(p("HealthCheckEnabled", "false"));
                fields
            },
            status: 400,
            code: Some("InvalidConfigurationRequest"),
        },
        Row {
            // A framework refusal raised INSIDE a matched operation: `PageSize` is modelled
            // `range 1..400` (`smithy_query(query="operation",
            // operation="DescribeTargetGroups")`), so 0 is refused before the handler runs.
            case: "a page size below the modelled minimum",
            key: V2_KEY,
            action: "DescribeTargetGroups",
            version: Some(V2_VERSION),
            params: vec![p("PageSize", "0")],
            status: 400,
            code: None,
        },
        Row {
            // The same class on a different member and through the shared door:
            // `HealthCheckIntervalSeconds` is modelled `range 5..300`.
            case: "a health-check interval below the modelled minimum",
            key: SHARED_KEY,
            action: "CreateTargetGroup",
            version: Some(V2_VERSION),
            params: {
                let mut fields = target_group("envelope-tg-3", "80");
                fields.push(p("HealthCheckIntervalSeconds", "1"));
                fields
            },
            status: 400,
            code: None,
        },
        Row {
            // A routing failure of THIS model on its own key: no front door is involved,
            // so the envelope is the one its wrapped definition renders.
            case: "an action this model does not declare, on its own key",
            key: V2_KEY,
            action: "DescribeInstanceHealth",
            version: Some(V2_VERSION),
            params: vec![p("LoadBalancerName", "no-such-lb")],
            status: 400,
            code: Some("InvalidAction"),
        },
        Row {
            // An action NEITHER model declares, through the shared door.
            case: "an action neither model declares",
            key: SHARED_KEY,
            action: "NoSuchAction",
            version: Some(V2_VERSION),
            params: vec![],
            status: 400,
            code: Some("InvalidAction"),
        },
        Row {
            // The refusal this shared endpoint renders ITSELF: a version no Elastic Load
            // Balancing model declares.
            case: "a version neither model declares",
            key: SHARED_KEY,
            action: "CreateTargetGroup",
            version: Some("2099-01-01"),
            params: target_group("envelope-tg-4", "80"),
            status: 400,
            code: Some("InvalidAction"),
        },
        Row {
            // …and the same renderer reached from the other side: an envelope naming no
            // version is served as the front door's own API, which has no such action.
            case: "an action the serving model lacks, with no version named",
            key: SHARED_KEY,
            action: "CreateTargetGroup",
            version: None,
            params: target_group("envelope-tg-5", "80"),
            status: 400,
            code: Some("InvalidAction"),
        },
        Row {
            // A `Version` carrying XML metacharacters, including the envelope's own tags:
            // the refusal message is built from the request, so the renderer must escape
            // what it reflects and the envelope must stay ONE well-formed document with
            // ONE request id of its own — a reflected `<RequestId>` is not the
            // envelope's.
            case: "a version carrying the envelope's own markup",
            key: SHARED_KEY,
            action: "DescribeTargetGroups",
            version: Some("</Message></Error></ErrorResponse><RequestId>spoofed</RequestId>"),
            params: vec![],
            status: 400,
            code: Some("InvalidAction"),
        },
        Row {
            // The CLASSIC model's own modelled error, whose `awsQueryError` code
            // (`smithy_query(query="operation", operation="DescribeLoadBalancers")` on that
            // model) differs from its shape name: the rule is the endpoint's, not one
            // model's.
            case: "the Classic model's modelled not-found",
            key: SHARED_KEY,
            action: "DescribeLoadBalancers",
            version: Some(CLASSIC_VERSION),
            params: vec![p("LoadBalancerNames.member.1", "no-such-lb")],
            status: 400,
            code: Some("LoadBalancerNotFound"),
        },
        Row {
            // A framework refusal on the Classic model, whose `PageSize` is modelled
            // `range 1..400` too.
            case: "a page size below the modelled minimum on the Classic model",
            key: SHARED_KEY,
            action: "DescribeLoadBalancers",
            version: Some(CLASSIC_VERSION),
            params: vec![p("PageSize", "0")],
            status: 400,
            code: None,
        },
    ];

    // Every request id this endpoint reported, the success included: one per response, so
    // no two of them may be equal.
    let mut request_ids: Vec<(&str, String)> = vec![(
        "the live target group a collision needs",
        first_text(&created.body, "RequestId").unwrap_or_default(),
    )];

    for row in &rows {
        let answer = call(&router, row.key, row.action, row.version, &row.params).await;
        let case = row.case;
        let status = answer.status;
        let body = &answer.body;

        // What the issue reported: a JSON router refusal no awsQuery client can parse.
        assert!(
            body.trim_start().starts_with('<') && !body.contains("__type"),
            "case {case:?}: an awsQuery client must get XML, got {status} with {body}"
        );
        assert!(
            !body.contains("AmbiguousService") && !body.contains("UnknownService"),
            "case {case:?}: the request must reach exactly one service, got {status} with {body}"
        );
        assert_eq!(
            answer.content_type, "text/xml",
            "case {case:?}: expected content-type text/xml, got {:?} with {body}",
            answer.content_type
        );
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with {body}",
            row.status
        );
        assert!(
            body.contains("<ErrorResponse"),
            "case {case:?}: expected an <ErrorResponse> envelope, got {status} with {body}"
        );
        assert_eq!(
            first_text(body, "Type").as_deref(),
            Some("Sender"),
            "case {case:?}: a 4xx is the sender's fault, got {status} with {body}"
        );
        let code = first_text(body, "Code");
        match row.code {
            Some(expected) => assert_eq!(
                code.as_deref(),
                Some(expected),
                "case {case:?}: expected the error code {expected}, got {status} with {body}"
            ),
            None => assert!(
                code.is_some(),
                "case {case:?}: expected a non-empty <Code> to branch on, got {status} with {body}"
            ),
        }
        assert!(
            first_text(body, "Message").is_some(),
            "case {case:?}: expected a non-empty <Message>, got {status} with {body}"
        );

        // The envelope is ONE document: a value reflected into the message cannot open a
        // second one, and cannot contribute the envelope's request id.
        assert_eq!(
            occurrences(body, "<ErrorResponse"),
            1,
            "case {case:?}: expected exactly one <ErrorResponse> envelope, got {status} with \
             {body}"
        );
        assert!(
            !body.contains("spoofed") || body.contains("&lt;RequestId&gt;spoofed"),
            "case {case:?}: a reflected value must be escaped, not markup, got {status} with \
             {body}"
        );

        // The element the layers disagreed about: the envelope's own request id, exactly
        // one of it, in the form an AWS request id has.
        assert_eq!(
            occurrences(body, "<RequestId>"),
            1,
            "case {case:?}: expected exactly one <RequestId> in the envelope, got {status} with \
             {body}"
        );
        let request_id = first_text(body, "RequestId").unwrap_or_default();
        assert!(
            is_request_id(&request_id),
            "case {case:?}: expected a request id of the form 8-4-4-4-12 lowercase hex, got \
             {request_id:?} in {body}"
        );
        // AWS reports the one id in the header as well as in the document, so a client that
        // correlates on either reads the same value.
        assert_eq!(
            answer.request_id_header, request_id,
            "case {case:?}: expected the x-amzn-requestid header to carry the envelope's id \
             {request_id:?}, got {:?}",
            answer.request_id_header
        );
        request_ids.push((case, request_id));
    }

    for (case, id) in &request_ids {
        let shared: Vec<&str> = request_ids
            .iter()
            .filter(|(other_case, other)| other_case != case && other == id)
            .map(|(other_case, _)| *other_case)
            .collect();
        assert!(
            shared.is_empty(),
            "case {case:?}: a request id names ONE response, but {id} is also reported by \
             {shared:?}"
        );
    }
}
