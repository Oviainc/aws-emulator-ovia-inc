// SPDX-License-Identifier: CC0-1.0

//! An API Gateway stage whose method integrates an AWS SERVICE ACTION on a QUEUE: the whole
//! path a client takes from the stage invoke URL to the message in Amazon SQS, across three
//! services on one engine.
//!
//! RULE: a deployed method whose integration is of type `AWS` is served by MAKING the call
//! its `uri` and its integration request name against the service that `uri`'s subdomain
//! names — `Integration$uri` gives the form
//! `arn:aws:apigateway:{region}:{subdomain}:path|action/{service_api}` — under the role
//! `Integration$credentials` names, and the method response is the answer that service gave,
//! mapped through the integration response whose `selectionPattern` matches the status it
//! answered with ("For all other HTTP and AWS back ends, the HTTP status code is matched",
//! `IntegrationResponse$selectionPattern`). Amazon SQS's api is a QUERY api, so the action
//! and its parameters travel in the integration request as that wire form carries them — the
//! `Action` and the other fields of an `application/x-www-form-urlencoded` body, which is
//! what an integration request template renders, the query string the integration maps, and
//! the `action/{Action}` form of the `uri` itself — and the answer is the document that api
//! answers the action with. The role decides whether the call is made at all: a role whose
//! trust policy does not let `apigateway.amazonaws.com` assume it is an integration that
//! could not be performed (`API_CONFIGURATION_ERROR`, HTTP 500 and
//! `{"message":"Internal server error"}`), and a call the policies in force on the role do
//! not allow is refused by the service with its own `AccessDenied` (HTTP 403), which the
//! integration responses then map like any other backend answer. Every request to the
//! deployed stage is therefore answered, and what it is answered with is computed from the
//! queue the call reached.
//!
//! Only a cross-service suite can pin this: the queue and the messages are Amazon SQS's, the
//! role and its policies are IAM's, and the api, the stage and the integration are API
//! Gateway's, so the rule is observable only where all three are mounted on one engine.
//!
//! The rows vary what the rule generalises over: the ACTION (send, receive, delete, read the
//! attributes), the FORM the `uri` names the call in (the `path` form, which names the queue,
//! and the `action` form, which names the action), WHERE the action and its parameters come
//! from (the rendered request template, the mapped query string, the `uri`'s own pairs), the
//! message BODY (plain text, text carrying every character that is significant in a form
//! body, text outside ASCII, a long body), and WHICH role the integration names (one that may
//! send, one whose grant allows no send, one this service cannot assume, and no role at all).
//!
//! What a client reads is the document that api answers a query request with: the
//! `{Action}Response` envelope, whose result member carries the action's output, and the
//! error form of the same envelope, whose `Error` member carries the fault side, the code
//! and the sentence of a refusal.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and the Region the integration ARNs name.
const REGION: &str = "us-east-1";

/// The long-term key every emulator publishes, which is what the setup calls are signed
/// with. The signature itself is never checked: this router runs with request
/// authentication off, as every suite in this crate runs it.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The queue every row below addresses.
const QUEUE: &str = "stage-integration-orders";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// A trust policy that lets API Gateway assume the role it is attached to. It carries none
/// of the characters that are significant in an `application/x-www-form-urlencoded` body.
const APIGATEWAY_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"apigateway.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The same policy for ANOTHER service's principal: a role this api cannot assume, which is
/// the only thing that differs between it and the row above.
const LAMBDA_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The media type the method requests below declare, which is the key their integration
/// request template is stored under.
const JSON: &str = "application/json";

/// The integration responses every method here declares: the default (no pattern) for a
/// success, and one per class of refusal, so what a client sees is the status the service
/// answered rather than one status for every outcome.
const MAPPED_STATUSES: &[(&str, Option<&str>)] = &[
    ("200", None),
    ("400", Some("400")),
    ("403", Some("403")),
    ("500", Some(r"5\d{2}")),
];

/// What one call answered.
struct Answer {
    status: u16,
    body: Bytes,
}

impl Answer {
    /// The body as text.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is neither JSON nor present.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The three services this rule spans, over ONE engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AMBIENT_KEY}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One API Gateway control-plane call that has to succeed with `expected`.
async fn apigw_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answer = dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", JSON),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await;
    assert_eq!(
        answer.status, expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`), as its status and parsed
/// body.
async fn sqs(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let answer = dispatch(
        router,
        "POST",
        &format!("sqs.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("x-amz-target", &format!("AmazonSQS.{operation}")),
            ("authorization", &authorization("sqs")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    (answer.status, answer.json())
}

/// One Amazon SQS call that has to succeed.
async fn sqs_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = sqs(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One IAM awsQuery call that has to succeed, answering its body.
async fn iam_ok(router: &MultiServiceRouter, case: &str, body: String) -> String {
    let answer = dispatch(
        router,
        "POST",
        "iam.amazonaws.com",
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("iam")),
        ],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "setup {case:?}: expected HTTP 200 from IAM, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.text()
}

/// One request to the api's own invoke endpoint — the host
/// `{rest-api-id}.execute-api.{region}.amazonaws.com` — carrying no credential at all, as a
/// client of a public method sends it.
async fn invoke(
    router: &MultiServiceRouter,
    api: &str,
    verb: &str,
    path: &str,
    body: &str,
) -> Answer {
    dispatch(
        router,
        verb,
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}{path}"),
        &[("content-type", JSON)],
        Bytes::from(body.to_string()),
    )
    .await
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// The member at `path` of a JSON document, as text — `None` when the document carries no
/// value there.
fn at(body: &Value, path: &[&str]) -> Option<String> {
    let mut node = body;
    for step in path {
        node = node.get(step)?;
    }
    node.as_str().map(str::to_string)
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One IAM role: created with `trust` as its trust policy and `document` as its one inline
/// policy, answering its ARN.
async fn role(router: &MultiServiceRouter, name: &str, trust: &str, document: &str) -> String {
    let created = iam_ok(
        router,
        &format!("CreateRole({name})"),
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={name}\
             &AssumeRolePolicyDocument={trust}"
        ),
    )
    .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup CreateRole({name}): no <Arn> in {created}"));
    iam_ok(
        router,
        &format!("PutRolePolicy({name})"),
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={name}\
             &PolicyName=queue-integration&PolicyDocument={document}"
        ),
    )
    .await;
    arn
}

/// One identity-based policy granting `actions` on `resource`, written with none of the
/// characters a form body treats specially.
fn policy(actions: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":[{actions}],"Resource":"{resource}"}}]}}"#
    )
}

/// One resource of the api under `parent`, answering its id.
async fn resource(router: &MultiServiceRouter, api: &str, parent: &str, part: &str) -> String {
    let created = apigw_ok(
        router,
        &format!("CreateResource({part})"),
        "POST",
        &format!("/restapis/{api}/resources/{parent}"),
        Some(json!({ "pathPart": part })),
        201,
    )
    .await;
    member(&format!("CreateResource({part})"), &created, "id")
}

/// The `{account-id}/{queue-name}` tail of a queue url, which is what the `path` form of an
/// integration `uri` names a queue by — read off the url Amazon SQS itself handed back, so
/// no account id is written here.
fn queue_path(url: &str) -> String {
    let mut segments = url.trim_end_matches('/').rsplit('/');
    let name = segments.next().unwrap_or_default();
    let account = segments.next().unwrap_or_default();
    format!("{account}/{name}")
}

/// One method of `resource` whose integration is the `AWS` queue call `uri` names, under
/// `credentials` (none when it is `None`), with `template` as its integration request
/// template (none when it is `None`) and `parameters` as the values it maps onto the
/// integration request.
#[allow(clippy::too_many_arguments)]
async fn queue_method(
    router: &MultiServiceRouter,
    api: &str,
    resource_id: &str,
    verb: &str,
    uri: &str,
    credentials: Option<&str>,
    template: Option<&str>,
    parameters: &[(&str, &str)],
) {
    let case = format!("{verb} {uri}");
    apigw_ok(
        router,
        &case,
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    let mut integration = json!({
        "type": "AWS",
        // `integrationHttpMethod` travels under its `jsonName`, which is what
        // `smithy_query(query="operation",operation="PutIntegration")` reports for it. Every
        // call of a query api is a POST, whatever verb the METHOD carries.
        "httpMethod": "POST",
        "uri": uri,
    });
    if let Some(credentials) = credentials {
        integration["credentials"] = json!(credentials);
    }
    if let Some(template) = template {
        integration["requestTemplates"] = json!({ JSON: template });
    }
    if !parameters.is_empty() {
        integration["requestParameters"] = Value::Object(
            parameters
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    apigw_ok(
        router,
        &case,
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}/integration"),
        Some(integration),
        201,
    )
    .await;
    for (status, pattern) in MAPPED_STATUSES {
        apigw_ok(
            router,
            &case,
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}/responses/{status}"),
            Some(json!({})),
            201,
        )
        .await;
        let mut response = json!({ "statusCode": status });
        if let Some(pattern) = pattern {
            response["selectionPattern"] = json!(pattern);
        }
        apigw_ok(
            router,
            &case,
            "PUT",
            &format!(
                "/restapis/{api}/resources/{resource_id}/methods/{verb}/integration/responses/{status}"
            ),
            Some(response),
            201,
        )
        .await;
    }
}

/// One row of the send table: the route it is sent through and the message body it carries.
struct Sent {
    case: &'static str,
    route: &'static str,
    body: String,
}

#[tokio::test]
async fn a_deployed_method_integrating_a_queue_action_makes_that_call() {
    let router = emulator();
    let queue_url = member(
        "CreateQueue",
        &sqs_ok(&router, "CreateQueue", "CreateQueue", &json!({ "QueueName": QUEUE })).await,
        "QueueUrl",
    );
    let path = queue_path(&queue_url);
    let queue_arn = member(
        "GetQueueAttributes",
        sqs_ok(
            &router,
            "CreateQueue",
            "GetQueueAttributes",
            &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
        )
        .await
        .get("Attributes")
        .unwrap_or(&Value::Null),
        "QueueArn",
    );

    // One role this api can assume that may act on this queue, written by IAM's own
    // `CreateRole`/`PutRolePolicy`.
    let allowed = role(
        &router,
        "queue-integration-allowed",
        APIGATEWAY_TRUST,
        &policy(
            r#""sqs:SendMessage","sqs:ReceiveMessage","sqs:DeleteMessage","sqs:GetQueueAttributes""#,
            &queue_arn,
        ),
    )
    .await;

    let api_body = apigw_ok(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "queue-integration" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api_body, "id");
    let root = member("CreateRestApi", &api_body, "rootResourceId");

    let path_form = format!("arn:aws:apigateway:{REGION}:sqs:path/{path}");
    let action_form = format!("arn:aws:apigateway:{REGION}:sqs:action/SendMessage");

    // A send whose action and body both come from the RENDERED request template, which is
    // the form the integration request carries.
    let send = resource(&router, &api, &root, "send").await;
    queue_method(
        &router,
        &api,
        &send,
        "POST",
        &path_form,
        Some(&allowed),
        Some("Action=SendMessage&MessageBody=$util.urlEncode($input.body)"),
        &[(
            "integration.request.header.Content-Type",
            "'application/x-www-form-urlencoded'",
        )],
    )
    .await;

    // The same send through the ACTION form of the `uri`, with the queue named by a
    // parameter the integration maps onto the integration request's query string.
    let acted = resource(&router, &api, &root, "acted").await;
    queue_method(
        &router,
        &api,
        &acted,
        "POST",
        &action_form,
        Some(&allowed),
        Some("MessageBody=$util.urlEncode($input.body)"),
        &[(
            "integration.request.querystring.QueueUrl",
            &format!("'{queue_url}'"),
        )],
    )
    .await;

    // A send whose every parameter is MAPPED rather than rendered: no request template at
    // all, so the action and the body come from the query string the integration maps.
    let mapped = resource(&router, &api, &root, "mapped").await;
    queue_method(
        &router,
        &api,
        &mapped,
        "POST",
        &path_form,
        Some(&allowed),
        None,
        &[
            ("integration.request.querystring.Action", "'SendMessage'"),
            (
                "integration.request.querystring.MessageBody",
                "method.request.querystring.text",
            ),
        ],
    )
    .await;

    // The three other actions of the queue's message plane, each named where that action's
    // own request carries it.
    let receive = resource(&router, &api, &root, "receive").await;
    queue_method(
        &router,
        &api,
        &receive,
        "GET",
        &path_form,
        Some(&allowed),
        None,
        &[
            ("integration.request.querystring.Action", "'ReceiveMessage'"),
            (
                "integration.request.querystring.MaxNumberOfMessages",
                "'10'",
            ),
        ],
    )
    .await;
    let remove = resource(&router, &api, &root, "remove").await;
    queue_method(
        &router,
        &api,
        &remove,
        "POST",
        &path_form,
        Some(&allowed),
        Some("Action=DeleteMessage&ReceiptHandle=$util.urlEncode($input.body)"),
        &[],
    )
    .await;
    let attributes = resource(&router, &api, &root, "attributes").await;
    queue_method(
        &router,
        &api,
        &attributes,
        "GET",
        &path_form,
        Some(&allowed),
        None,
        &[
            (
                "integration.request.querystring.Action",
                "'GetQueueAttributes'",
            ),
            ("integration.request.querystring.AttributeName.1", "'All'"),
        ],
    )
    .await;

    apigw_ok(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;

    // The send rows: each is sent through the stage, then READ BACK through Amazon SQS's own
    // API, so what is asserted is the message the queue holds rather than what the stage
    // said about it.
    let rows = vec![
        Sent {
            case: "plain text through the path form",
            route: "/send",
            body: "the first order".to_string(),
        },
        Sent {
            case: "text carrying every character significant in a form body",
            route: "/send",
            body: "a&b=c+d%e".to_string(),
        },
        Sent {
            case: "text outside ASCII",
            route: "/send",
            body: "café — naïve".to_string(),
        },
        Sent {
            case: "a long body, which one form field carries whole",
            route: "/send",
            body: "x".repeat(4096),
        },
        Sent {
            case: "the action form of the same call",
            route: "/acted",
            body: "sent through the action form".to_string(),
        },
        Sent {
            case: "a send whose parameters are mapped onto the query string",
            route: "/mapped?text=mapped+from+the+query+string",
            body: String::new(),
        },
    ];
    for row in &rows {
        let answered = invoke(&router, &api, "POST", row.route, &row.body).await;
        let text = answered.text();
        assert_eq!(
            answered.status, 200,
            "case {:?}: expected the queue's own 200, got {} with {text:?}",
            row.case, answered.status
        );
        let result = ["SendMessageResponse", "SendMessageResult"];
        let digest = at(&answered.json(), &[result[0], result[1], "MD5OfMessageBody"])
            .unwrap_or_else(|| {
                panic!("case {:?}: expected an MD5OfMessageBody, got {text:?}", row.case)
            });
        assert!(
            at(&answered.json(), &[result[0], result[1], "MessageId"])
                .is_some_and(|id| !id.is_empty()),
            "case {:?}: expected a MessageId the queue minted, got {text:?}",
            row.case
        );
        // The message the queue holds, read through Amazon SQS itself: its body is what the
        // client sent and the digest it reports is the one the stage answered.
        let received = sqs_ok(
            &router,
            row.case,
            "ReceiveMessage",
            &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 1 }),
        )
        .await;
        let messages = received
            .get("Messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            messages.len(),
            1,
            "case {:?}: expected exactly the one message this row sent, got {received}",
            row.case
        );
        let expected_body = match row.body.is_empty() {
            true => "mapped from the query string".to_string(),
            false => row.body.clone(),
        };
        assert_eq!(
            member(row.case, &messages[0], "Body"),
            expected_body,
            "case {:?}: expected the body the client sent, got {}",
            row.case,
            messages[0]
        );
        assert_eq!(
            member(row.case, &messages[0], "MD5OfBody"),
            digest,
            "case {:?}: expected the digest the stage answered to be the queue's own, got {}",
            row.case,
            messages[0]
        );
        sqs_ok(
            &router,
            row.case,
            "DeleteMessage",
            &json!({
                "QueueUrl": queue_url,
                "ReceiptHandle": member(row.case, &messages[0], "ReceiptHandle"),
            }),
        )
        .await;
    }

    // The RECEIVE and the DELETE through the stage: a message put on the queue through
    // Amazon SQS's own API is handed to the client by the stage, and the receipt handle it
    // answered removes it.
    let polled = "polled through the stage";
    sqs_ok(
        &router,
        "a receive through the stage",
        "SendMessage",
        &json!({ "QueueUrl": queue_url, "MessageBody": polled }),
    )
    .await;
    let answered = invoke(&router, &api, "GET", "/receive", "").await;
    let text = answered.text();
    assert_eq!(
        answered.status, 200,
        "case {:?}: expected the queue's own 200, got {} with {text:?}",
        "a receive through the stage", answered.status
    );
    let received = answered.json();
    let message = received
        .get("ReceiveMessageResponse")
        .and_then(|answer| answer.get("ReceiveMessageResult"))
        .and_then(|result| result.get("Messages"))
        .and_then(Value::as_array)
        .and_then(|messages| messages.first())
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the one message the queue holds, got {text:?}",
                "a receive through the stage"
            )
        });
    assert_eq!(
        at(&message, &["Body"]).as_deref(),
        Some(polled),
        "case {:?}: expected the body the queue holds, got {text:?}",
        "a receive through the stage"
    );
    let handle = at(&message, &["ReceiptHandle"]).unwrap_or_else(|| {
        panic!(
            "case {:?}: expected a ReceiptHandle, got {text:?}",
            "a receive through the stage"
        )
    });
    let removed = invoke(&router, &api, "POST", "/remove", &handle).await;
    assert_eq!(
        removed.status,
        200,
        "case {:?}: expected the queue's own 200, got {} with {:?}",
        "a delete through the stage",
        removed.status,
        removed.text()
    );
    let (_, after) = sqs(
        &router,
        "ReceiveMessage",
        &json!({ "QueueUrl": queue_url, "VisibilityTimeout": 0 }),
    )
    .await;
    assert!(
        after.get("Messages").is_none(),
        "case {:?}: expected the deleted message to be gone, got {after}",
        "a delete through the stage"
    );

    // The ATTRIBUTE read through the stage, whose answer is the queue's own records.
    let read = invoke(&router, &api, "GET", "/attributes", "").await;
    let text = read.text();
    assert_eq!(
        read.status, 200,
        "case {:?}: expected the queue's own 200, got {} with {text:?}",
        "the attribute read", read.status
    );
    assert_eq!(
        at(
            &read.json(),
            &[
                "GetQueueAttributesResponse",
                "GetQueueAttributesResult",
                "Attributes",
                "QueueArn",
            ],
        )
        .as_deref(),
        Some(queue_arn.as_str()),
        "case {:?}: expected the queue's own ARN among its attributes, got {text:?}",
        "the attribute read"
    );
}

#[tokio::test]
async fn the_integration_role_decides_whether_the_queue_call_is_made() {
    let router = emulator();
    let queue_url = member(
        "CreateQueue",
        &sqs_ok(&router, "CreateQueue", "CreateQueue", &json!({ "QueueName": QUEUE })).await,
        "QueueUrl",
    );
    let path = queue_path(&queue_url);
    let queue_arn = member(
        "GetQueueAttributes",
        sqs_ok(
            &router,
            "CreateQueue",
            "GetQueueAttributes",
            &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
        )
        .await
        .get("Attributes")
        .unwrap_or(&Value::Null),
        "QueueArn",
    );

    // Three roles: one that may send, one this api can assume whose grant allows every
    // queue action EXCEPT the send, and one this api cannot assume at all.
    let allowed = role(
        &router,
        "role-allowed",
        APIGATEWAY_TRUST,
        &policy(r#""sqs:SendMessage""#, &queue_arn),
    )
    .await;
    let without_send = role(
        &router,
        "role-without-send",
        APIGATEWAY_TRUST,
        &policy(
            r#""sqs:ReceiveMessage","sqs:GetQueueAttributes""#,
            &queue_arn,
        ),
    )
    .await;
    let untrusted = role(
        &router,
        "role-untrusted",
        LAMBDA_TRUST,
        &policy(r#""sqs:SendMessage""#, &queue_arn),
    )
    .await;

    let api_body = apigw_ok(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "queue-integration-roles" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api_body, "id");
    let root = member("CreateRestApi", &api_body, "rootResourceId");
    let path_form = format!("arn:aws:apigateway:{REGION}:sqs:path/{path}");
    let template = "Action=SendMessage&MessageBody=$util.urlEncode($input.body)";

    for (part, credentials) in [
        ("allowed", Some(allowed.as_str())),
        ("withheld", Some(without_send.as_str())),
        ("unassumable", Some(untrusted.as_str())),
        ("open", None),
    ] {
        let resource_id = resource(&router, &api, &root, part).await;
        queue_method(
            &router,
            &api,
            &resource_id,
            "POST",
            &path_form,
            credentials,
            Some(template),
            &[],
        )
        .await;
    }
    apigw_ok(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;

    // Rows: the route, the status the stage must answer, and what the body must say — the
    // message is delivered only where the call was made.
    let rows: &[(&str, &str, u16, bool)] = &[
        ("a role that may send", "/allowed", 200, true),
        ("a role whose grant allows no send", "/withheld", 403, false),
        ("a role this api cannot assume", "/unassumable", 500, false),
        ("no role at all", "/open", 200, true),
    ];
    for (case, route, expected, delivered) in rows {
        let answered = invoke(&router, &api, "POST", route, case).await;
        let text = answered.text();
        assert_eq!(
            answered.status, *expected,
            "case {case:?}: expected HTTP {expected}, got {} with {text:?}",
            answered.status
        );
        match (*expected, *delivered) {
            (200, _) => assert!(
                at(
                    &answered.json(),
                    &["SendMessageResponse", "SendMessageResult", "MessageId"],
                )
                .is_some(),
                "case {case:?}: expected the queue's own MessageId, got {text:?}"
            ),
            (403, _) => {
                assert_eq!(
                    at(&answered.json(), &["Error", "Code"]).as_deref(),
                    Some("AccessDenied"),
                    "case {case:?}: expected the service's own AccessDenied, got {text:?}"
                );
                assert_eq!(
                    at(&answered.json(), &["Error", "Type"]).as_deref(),
                    Some("Sender"),
                    "case {case:?}: expected the fault to be the caller's, got {text:?}"
                );
                assert_eq!(
                    at(&answered.json(), &["Error", "Message"]).as_deref(),
                    Some(
                        format!("Access to the resource {queue_url} is denied.").as_str()
                    ),
                    "case {case:?}: expected the sentence naming the queue, got {text:?}"
                );
            }
            _ => assert_eq!(
                answered.json().get("message").and_then(Value::as_str),
                Some("Internal server error"),
                "case {case:?}: expected the api's own integration-failure message, got \
                 {text:?}"
            ),
        }
        let received = sqs_ok(
            &router,
            case,
            "ReceiveMessage",
            &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        let bodies: Vec<String> = received
            .get("Messages")
            .and_then(Value::as_array)
            .map(|messages| {
                messages
                    .iter()
                    .filter_map(|message| message.get("Body").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            bodies.contains(&(*case).to_string()),
            *delivered,
            "case {case:?}: expected delivered={delivered}, got {received}"
        );
        for message in received
            .get("Messages")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
        {
            sqs_ok(
                &router,
                case,
                "DeleteMessage",
                &json!({
                    "QueueUrl": queue_url,
                    "ReceiptHandle": member(case, message, "ReceiptHandle"),
                }),
            )
            .await;
        }
    }
}
