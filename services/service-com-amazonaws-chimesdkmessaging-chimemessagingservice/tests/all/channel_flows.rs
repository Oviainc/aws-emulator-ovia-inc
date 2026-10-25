// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Chime SDK Messaging channel-FLOW slice.
//!
//! Every case drives real HTTP through the production router — restJson1 request on the
//! wire in, restJson1 response out — so nothing here can pass by reading a handler's Rust
//! types directly. Two rules are under test:
//!
//! * each operation computes its answer from the SAME persisted channel-flow store: what
//!   `CreateChannelFlow` wrote is what `DescribeChannelFlow` and `ListChannelFlows`
//!   report, and what `DeleteChannelFlow` removed is gone for both; and
//! * a `ChannelFlowArn` that names no channel flow this account holds is answered with
//!   the modelled `ForbiddenException` — a TERMINAL HTTP 403 carrying `Code` `Forbidden`
//!   in the restJson1 error shape, `Content-Type: application/json` and an
//!   `x-amzn-errortype` naming that modelled shape — because `DescribeChannelFlow` models
//!   no `NotFoundException` (`smithy_query(query="operation",
//!   operation="DescribeChannelFlow")`: errors `BadRequestException` plus the service's
//!   common `ForbiddenException`, `ServiceFailureException`,
//!   `ServiceUnavailableException`, `ThrottledClientException`,
//!   `UnauthorizedClientException`).
//!
//! The tables span what those rules generalise over rather than re-running one recorded
//! request: processor counts at both ends of the modelled `ProcessorList len 1..3`, every
//! `FallbackAction` wire value, every `ChannelFlowExecutionOrder` inside its modelled
//! `range 1..3`, tags present and absent, page sizes at and past the modelled `MaxResults`
//! bounds, and six shapes of ARN that name no stored flow.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

// The restJson1 label/query encoders this binary's other suite already states: one copy
// for the whole test target, so two suites cannot encode an ARN differently.
use super::channel_messaging::{encode, query};

/// The `AppInstanceArn` most cases create their channel flows under.
const APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1";
/// A second app instance, so the `ListChannelFlows` filter has something to exclude.
const OTHER_APP_INSTANCE: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai2";
/// The `x-amz-chime-bearer` identity every request is made as.
const BEARER: &str = "arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/alice";
/// A processor Lambda ARN inside the modelled `LambdaFunctionArn` pattern
/// `^arn:aws:lambda:[a-z]{2}-[a-z]+-\d{1}:\d{12}:function:[a-zA-Z0-9\-_\.]+(:(\$LATEST|[a-zA-Z0-9\-_]+))?$`.
const PROCESSOR_LAMBDA: &str = "arn:aws:lambda:us-east-1:111122223333:function:redact-profanity";

/// The `FallbackAction` wire values, from
/// `smithy_query(query="shape", shape="FallbackAction")`.
const FALLBACK_ACTIONS: [&str; 2] = ["ABORT", "CONTINUE"];
/// The `InvocationType` wire values, from the same model
/// (`smithy_query(query="shape", shape="LambdaConfiguration")` -> `InvocationType`).
const INVOCATION_TYPES: [&str; 1] = ["ASYNC"];
/// `MaxResults`'s modelled bound on `ListChannelFlows` (`range 1..50`).
const MAX_RESULTS_MAX: i64 = 50;

/// Every error shape `DescribeChannelFlow` declares, from
/// `smithy_query(query="operation", operation="DescribeChannelFlow")` — `errors`:
/// `BadRequestException` (400) plus this service's common `ForbiddenException` (403),
/// `ServiceFailureException` (500), `ServiceUnavailableException` (503),
/// `ThrottledClientException` (429) and `UnauthorizedClientException` (401). A refusal of
/// this operation names one of these in `x-amzn-errortype`, never a code no model carries.
const DESCRIBE_CHANNEL_FLOW_ERRORS: [&str; 6] = [
    "BadRequestException",
    "ForbiddenException",
    "ServiceFailureException",
    "ServiceUnavailableException",
    "ThrottledClientException",
    "UnauthorizedClientException",
];

/// One response, reduced to the four things the wire contract pins: the HTTP status, the
/// `Content-Type`, the `x-amzn-errortype` a restJson1 client resolves the error shape
/// from, and the JSON body.
#[derive(Debug)]
struct Reply {
    status: u16,
    content_type: String,
    error_type: String,
    body: Value,
}

impl Reply {
    /// The modelled error code this service puts in the error shape's `Code` member.
    fn code(&self) -> &str {
        self.body
            .get("Code")
            .and_then(Value::as_str)
            .unwrap_or("<no Code>")
    }

    /// The error SHAPE `x-amzn-errortype` names, stripped of the optional `namespace#`
    /// prefix and `:sdk-url` suffix the restJson1 protocol allows around it.
    fn error_shape(&self) -> &str {
        let shape = self
            .error_type
            .split('#')
            .next_back()
            .unwrap_or(&self.error_type);
        shape.split(':').next().unwrap_or(shape)
    }
}

/// The emulator under test: this one service mounted in the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        // ONE shared engine, exactly as `router/src/lib.rs` builds it, so every request
        // in a case observes the same store.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is exercised
        // elsewhere; these cases are about the service's behaviour.
        router
            .set_request_authentication(false)
            .register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<&Value>) -> Reply {
        let bytes = match body {
            Some(value) => Bytes::from(serde_json::to_vec(value).expect("serialize the request")),
            None => Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "messaging-chime.us-east-1.amazonaws.com")
            // The SigV4 credential scope carries the signing name five sibling services
            // share, so this request only reaches this service because `can_handle` keys
            // on the endpoint and the path instead.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/chime/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("content-type", "application/json")
            .header("x-amz-chime-bearer", BEARER)
            .body(bytes)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        let content_type = header("content-type");
        let error_type = header("x-amzn-errortype");
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply {
            status,
            content_type,
            error_type,
            body,
        }
    }

    async fn create_channel_flow(&self, request: &Value) -> Reply {
        self.call("POST", "/channel-flows", Some(request)).await
    }

    async fn describe_channel_flow(&self, channel_flow_arn: &str) -> Reply {
        self.call(
            "GET",
            &format!("/channel-flows/{}", encode(channel_flow_arn)),
            None,
        )
        .await
    }

    async fn delete_channel_flow(&self, channel_flow_arn: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/channel-flows/{}", encode(channel_flow_arn)),
            None,
        )
        .await
    }

    async fn list_channel_flows(&self, params: &[(&str, String)]) -> Reply {
        self.call("GET", &format!("/channel-flows{}", query(params)), None)
            .await
    }

    /// Create a channel flow with one processor and return its ARN.
    async fn channel_flow(&self, app_instance_arn: &str, name: &str, token: &str) -> String {
        let reply = self
            .create_channel_flow(&json!({
                "AppInstanceArn": app_instance_arn,
                "Name": name,
                "ClientRequestToken": token,
                "Processors": [processor(name, 1, "ABORT", "ASYNC", PROCESSOR_LAMBDA)],
            }))
            .await;
        assert_eq!(
            reply.status, 201,
            "case {name:?}: CreateChannelFlow must answer 201, got {} {:?}",
            reply.status, reply.body
        );
        reply.body["ChannelFlowArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {name:?}: CreateChannelFlow must return a ChannelFlowArn")
            })
            .to_string()
    }
}

/// One modelled `Processor` as the wire carries it.
fn processor(
    name: &str,
    execution_order: i64,
    fallback_action: &str,
    invocation_type: &str,
    resource_arn: &str,
) -> Value {
    json!({
        "Name": name,
        "ExecutionOrder": execution_order,
        "FallbackAction": fallback_action,
        "Configuration": {
            "Lambda": {
                "ResourceArn": resource_arn,
                "InvocationType": invocation_type,
            }
        }
    })
}

#[tokio::test]
async fn a_channel_flow_reads_back_as_it_was_created() {
    let emulator = Emulator::new();

    // (case, the processors the request declares)
    //
    // The rows span the modelled `ProcessorList len 1..3` at both ends, every
    // `FallbackAction` wire value, every `ChannelFlowExecutionOrder` inside its modelled
    // `range 1..3`, and every `InvocationType` value.
    let mut rows: Vec<(String, Vec<Value>)> = vec![
        (
            "one processor, the shortest list the model allows".to_string(),
            vec![processor("only", 1, "ABORT", "ASYNC", PROCESSOR_LAMBDA)],
        ),
        (
            "two processors in execution order".to_string(),
            vec![
                processor("first", 1, "CONTINUE", "ASYNC", PROCESSOR_LAMBDA),
                processor("second", 2, "ABORT", "ASYNC", PROCESSOR_LAMBDA),
            ],
        ),
        (
            "three processors, the longest list the model allows".to_string(),
            vec![
                processor("first", 1, "ABORT", "ASYNC", PROCESSOR_LAMBDA),
                processor("second", 2, "CONTINUE", "ASYNC", PROCESSOR_LAMBDA),
                processor(
                    "third",
                    3,
                    "ABORT",
                    "ASYNC",
                    "arn:aws:lambda:eu-west-1:210987654321:function:another-processor:$LATEST",
                ),
            ],
        ),
    ];
    for action in FALLBACK_ACTIONS {
        for invocation in INVOCATION_TYPES {
            rows.push((
                format!("a single processor with FallbackAction {action} / {invocation}"),
                vec![processor(
                    "lone",
                    1,
                    action,
                    invocation,
                    PROCESSOR_LAMBDA,
                )],
            ));
        }
    }

    for (index, (case, processors)) in rows.into_iter().enumerate() {
        let name = format!("flow-{index}");
        let mut request = json!({
            "AppInstanceArn": APP_INSTANCE,
            "Name": name,
            "ClientRequestToken": format!("token-{index}"),
            "Processors": processors,
        });
        // The optional `Tags` member is present on the even rows and ABSENT (not null:
        // the modelled `TagList` is `len 1..50`) on the odd ones, so it is exercised both
        // ways.
        if index % 2 == 0 {
            request["Tags"] = json!([{"Key": "team", "Value": "chat"}]);
        }
        let created = emulator.create_channel_flow(&request).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 from CreateChannelFlow, got {} ({:?})",
            created.status, created.body
        );
        let arn = created.body["ChannelFlowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            arn.starts_with(&format!("{APP_INSTANCE}/channel-flow/")),
            "case {case:?}: expected a ChannelFlowArn under the app instance, got {arn:?}"
        );

        let described = emulator.describe_channel_flow(&arn).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected HTTP 200 from DescribeChannelFlow, got {} ({:?})",
            described.status, described.body
        );
        let flow = &described.body["ChannelFlow"];
        assert_eq!(
            flow["ChannelFlowArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the created ARN back, got {:?}",
            flow["ChannelFlowArn"]
        );
        assert_eq!(
            flow["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the Name the request carried, got {:?}",
            flow["Name"]
        );
        assert_eq!(
            flow["Processors"], request["Processors"],
            "case {case:?}: expected the processors the request declared, in order, got {:?}",
            flow["Processors"]
        );
        for member in ["CreatedTimestamp", "LastUpdatedTimestamp"] {
            assert!(
                flow[member].is_number(),
                "case {case:?}: expected {member} as an epoch-seconds timestamp, got {:?}",
                flow[member]
            );
        }
    }
}

#[tokio::test]
async fn an_arn_that_names_no_stored_channel_flow_is_forbidden() {
    let emulator = Emulator::new();
    let live = emulator.channel_flow(APP_INSTANCE, "live-flow", "token-live").await;
    let deleted = emulator
        .channel_flow(APP_INSTANCE, "doomed-flow", "token-doomed")
        .await;
    let gone = emulator.delete_channel_flow(&deleted).await;
    assert_eq!(
        gone.status, 204,
        "case \"DeleteChannelFlow\": expected HTTP 204, got {} ({:?})",
        gone.status, gone.body
    );

    // (case, the ChannelFlowArn to describe)
    //
    // Six shapes of ARN that name no channel flow this store holds: a flow id nothing
    // created, a flow under another app instance, a flow that was deleted, a channel's
    // ARN rather than a flow's, and the same flow id under another account and another
    // region. Each must be the SAME terminal refusal; the recorded AWS response this rule
    // was filed from is one row of this table (a channel flow whose create was rejected).
    let rows: Vec<(&str, String)> = vec![
        (
            "a channel flow id nothing created",
            format!("{APP_INSTANCE}/channel-flow/never-created"),
        ),
        (
            "a channel flow under another app instance",
            format!("{OTHER_APP_INSTANCE}/channel-flow/never-created"),
        ),
        ("a channel flow that was deleted", deleted.clone()),
        (
            "a channel's ARN rather than a channel flow's",
            format!("{APP_INSTANCE}/channel/abc123"),
        ),
        (
            "the live flow's id under another account",
            live.replace("111122223333", "444455556666"),
        ),
        (
            "the live flow's id in another region",
            live.replace("us-east-1", "eu-west-1"),
        ),
    ];

    for (case, arn) in rows {
        let reply = emulator.describe_channel_flow(&arn).await;
        assert_eq!(
            (reply.status, reply.code()),
            (403, "Forbidden"),
            "case {case:?} ({arn}): expected the modelled ForbiddenException — HTTP 403 with \
             Code \"Forbidden\" — got {} {:?} ({:?})",
            reply.status,
            reply.code(),
            reply.body
        );
        assert_eq!(
            reply.content_type, "application/json",
            "case {case:?}: a restJson1 error is framed as application/json, got {:?}",
            reply.content_type
        );
        assert!(
            reply.body.get("Message").and_then(Value::as_str).is_some(),
            "case {case:?}: the modelled ForbiddenException carries a Message, got {:?}",
            reply.body
        );
        // restJson1 resolves the error shape from this header, so it must name a shape
        // this operation models — never a code no model declares (the router's own
        // `AmbiguousService`, say).
        assert!(
            DESCRIBE_CHANNEL_FLOW_ERRORS.contains(&reply.error_shape()),
            "case {case:?}: expected x-amzn-errortype to name one of this operation's \
             modelled errors {DESCRIBE_CHANNEL_FLOW_ERRORS:?}, got {:?}",
            reply.error_type
        );
        assert_eq!(
            reply.error_shape(),
            "ForbiddenException",
            "case {case:?}: expected x-amzn-errortype to name ForbiddenException, got {:?}",
            reply.error_type
        );
    }

    // The flow that is still there is still readable: the refusal above is about the ARN,
    // not about the operation.
    let described = emulator.describe_channel_flow(&live).await;
    assert_eq!(
        described.status, 200,
        "case \"the live channel flow\": expected HTTP 200, got {} ({:?})",
        described.status, described.body
    );
}

#[tokio::test]
async fn deleting_a_channel_flow_removes_it_from_every_reader() {
    let emulator = Emulator::new();
    let arn = emulator.channel_flow(APP_INSTANCE, "flow", "token-1").await;

    let listed = emulator
        .list_channel_flows(&[("app-instance-arn", APP_INSTANCE.to_string())])
        .await;
    assert_eq!(
        listed.body["ChannelFlows"]
            .as_array()
            .map(|flows| flows.len()),
        Some(1),
        "case \"before the delete\": expected the one created flow in ListChannelFlows, got {:?}",
        listed.body
    );

    let deleted = emulator.delete_channel_flow(&arn).await;
    assert_eq!(
        deleted.status, 204,
        "case \"DeleteChannelFlow\": expected HTTP 204, got {} ({:?})",
        deleted.status, deleted.body
    );

    let described = emulator.describe_channel_flow(&arn).await;
    assert_eq!(
        (described.status, described.code()),
        (403, "Forbidden"),
        "case \"DescribeChannelFlow after the delete\": expected HTTP 403 Forbidden, got {} {:?}",
        described.status,
        described.code()
    );
    let listed = emulator
        .list_channel_flows(&[("app-instance-arn", APP_INSTANCE.to_string())])
        .await;
    assert_eq!(
        listed.body["ChannelFlows"]
            .as_array()
            .map(|flows| flows.len()),
        Some(0),
        "case \"after the delete\": expected ListChannelFlows to report none, got {:?}",
        listed.body
    );
    let twice = emulator.delete_channel_flow(&arn).await;
    assert_eq!(
        (twice.status, twice.code()),
        (403, "Forbidden"),
        "case \"DeleteChannelFlow twice\": expected HTTP 403 Forbidden the second time, got {} \
         {:?}",
        twice.status,
        twice.code()
    );
}

#[tokio::test]
async fn list_channel_flows_pages_one_app_instances_flows_in_arn_order() {
    let emulator = Emulator::new();

    // Nine flows under the app instance under test and two under another one, so the
    // filter has something to exclude at every page size.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..9 {
        expected.push(
            emulator
                .channel_flow(APP_INSTANCE, &format!("flow-{index}"), &format!("token-{index}"))
                .await,
        );
    }
    for index in 0..2 {
        emulator
            .channel_flow(
                OTHER_APP_INSTANCE,
                &format!("other-{index}"),
                &format!("other-token-{index}"),
            )
            .await;
    }
    expected.sort();

    // Page sizes spanning the modelled bound: one at a time, an uneven split, exactly the
    // page count, more than there are, and the modelled maximum.
    for page_size in [1_i64, 2, 4, 9, 20, MAX_RESULTS_MAX] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![
                ("app-instance-arn", APP_INSTANCE.to_string()),
                ("max-results", page_size.to_string()),
            ];
            if let Some(token) = &token {
                params.push(("next-token", token.clone()));
            }
            let reply = emulator.list_channel_flows(&params).await;
            assert_eq!(
                reply.status, 200,
                "case \"page size {page_size}\": expected HTTP 200 from ListChannelFlows, got {} \
                 ({:?})",
                reply.status, reply.body
            );
            let page = reply.body["ChannelFlows"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
                "case \"page size {page_size}\": a page must hold at most MaxResults entries, got \
                 {}",
                page.len()
            );
            for summary in page {
                seen.push(
                    summary["ChannelFlowArn"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            match reply.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case \"page size {page_size}\": expected this app instance's nine flows in ARN \
             order, got {seen:?}"
        );
    }

    // A page size outside the modelled `range 1..50` is a client error, whichever side of
    // the bound it falls on.
    for page_size in ["0", "51"] {
        let reply = emulator
            .list_channel_flows(&[
                ("app-instance-arn", APP_INSTANCE.to_string()),
                ("max-results", page_size.to_string()),
            ])
            .await;
        assert_eq!(
            reply.status, 400,
            "case \"max-results {page_size}\": expected HTTP 400 for a page size outside the \
             modelled bound, got {} ({:?})",
            reply.status, reply.body
        );
    }

    // A token this service never issued is refused rather than silently restarting the
    // listing.
    let reply = emulator
        .list_channel_flows(&[
            ("app-instance-arn", APP_INSTANCE.to_string()),
            ("next-token", "not-a-token".to_string()),
        ])
        .await;
    assert_eq!(
        reply.status, 400,
        "case \"a foreign NextToken\": expected HTTP 400, got {} ({:?})",
        reply.status, reply.body
    );
}

#[tokio::test]
async fn a_replayed_client_request_token_answers_with_the_first_channel_flow() {
    let emulator = Emulator::new();
    let request = json!({
        "AppInstanceArn": APP_INSTANCE,
        "Name": "idempotent",
        "ClientRequestToken": "token-replayed",
        "Processors": [processor("only", 1, "ABORT", "ASYNC", PROCESSOR_LAMBDA)],
    });

    let first = emulator.create_channel_flow(&request).await;
    let replay = emulator.create_channel_flow(&request).await;
    assert_eq!(
        (first.status, replay.status),
        (201, 201),
        "case \"a replayed ClientRequestToken\": both answers are 201, got {} and {}",
        first.status,
        replay.status
    );
    assert_eq!(
        first.body["ChannelFlowArn"], replay.body["ChannelFlowArn"],
        "case \"a replayed ClientRequestToken\": expected the first flow's ARN again, got {:?} \
         after {:?}",
        replay.body["ChannelFlowArn"], first.body["ChannelFlowArn"]
    );

    // A DIFFERENT token under the same app instance mints a second flow, so the ledger
    // keys on the token rather than on the request shape.
    let mut other = request.clone();
    other["ClientRequestToken"] = json!("token-other");
    let second = emulator.create_channel_flow(&other).await;
    assert_ne!(
        first.body["ChannelFlowArn"], second.body["ChannelFlowArn"],
        "case \"a second ClientRequestToken\": expected a new flow, got the first one's ARN {:?}",
        second.body["ChannelFlowArn"]
    );

    let listed = emulator
        .list_channel_flows(&[("app-instance-arn", APP_INSTANCE.to_string())])
        .await;
    assert_eq!(
        listed.body["ChannelFlows"]
            .as_array()
            .map(|flows| flows.len()),
        Some(2),
        "case \"a replayed ClientRequestToken\": the store holds two flows, not three, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn a_processor_outside_the_modelled_value_sets_is_refused() {
    let emulator = Emulator::new();

    // (case, the processor the request declares) — one row per member whose value set the
    // model declares, each with a value outside it.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a FallbackAction the model does not declare",
            processor("p", 1, "MAYBE", "ASYNC", PROCESSOR_LAMBDA),
        ),
        (
            "an InvocationType the model does not declare",
            processor("p", 1, "ABORT", "SYNC", PROCESSOR_LAMBDA),
        ),
        (
            "an ExecutionOrder past the modelled range",
            processor("p", 4, "ABORT", "ASYNC", PROCESSOR_LAMBDA),
        ),
        (
            "a ResourceArn that is not a Lambda function ARN",
            processor(
                "p",
                1,
                "ABORT",
                "ASYNC",
                "arn:aws:sqs:us-east-1:111122223333:queue/not-a-function",
            ),
        ),
    ];

    for (index, (case, processor)) in rows.into_iter().enumerate() {
        let reply = emulator
            .create_channel_flow(&json!({
                "AppInstanceArn": APP_INSTANCE,
                "Name": "refused",
                "ClientRequestToken": format!("refused-{index}"),
                "Processors": [processor],
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 and no channel flow written, got {} ({:?})",
            reply.status, reply.body
        );
    }

    // Nothing above was stored: a refused create writes no flow.
    let listed = emulator
        .list_channel_flows(&[("app-instance-arn", APP_INSTANCE.to_string())])
        .await;
    assert_eq!(
        listed.body["ChannelFlows"]
            .as_array()
            .map(|flows| flows.len()),
        Some(0),
        "case \"after four refused creates\": expected an empty store, got {:?}",
        listed.body
    );
}
