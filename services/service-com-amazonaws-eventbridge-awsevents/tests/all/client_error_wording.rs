// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the client errors it answers, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the
//! real `MultiServiceRouter`.
//!
//! The RULES asserted here, each over a table spanning its class rather than one
//! request:
//!
//! * a request that would take a resource COUNT past the quota bounding it is
//!   `LimitExceededException` (HTTP 400) carrying EventBridge's fixed sentence,
//!   which names neither the resource nor the number that bounds it — so no
//!   consumer can read a limit out of it;
//! * a `ScheduleExpression` this service will not take is refused by naming the
//!   PARAMETER and nothing else: no reason clause, and none of the submitted text;
//! * a create whose name is already taken states the resource by the name the
//!   request carried, quoted, and that one with that name exists already;
//! * a write that resolves another resource by ARN states the refusal with the ARN
//!   the request SENT, verbatim — never a name derived from it;
//! * `EventBusName` names the SCOPE a rule is sought in, so the bus is resolved
//!   FIRST: a request naming a bus this account does not hold reads that the BUS
//!   is missing, whatever rule name came with it, and only on a bus that exists is
//!   the rule the missing resource;
//! * a global endpoint that would replicate with no role to replicate as is
//!   refused with a sentence that is entirely fixed, carrying nothing of the
//!   request;
//! * a constraint clause states the value that was REJECTED, so a clause on a
//!   collection member states the collection the caller submitted — rendered the
//!   way the validation layer in front of EventBridge prints the entry it refused
//!   — rather than a count of it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The bus a request naming none is resolved against.
const DEFAULT_BUS: &str = "default";

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One `awsJson1_1` call to this service.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` a failure body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The ARN of a queue a rule's target points at.
fn target_arn(name: &str) -> String {
    format!(
        "arn:aws:sqs:{REGION}:{}:{name}",
        engine::emulator_account_id()
    )
}

/// A `ConnectionArn`, in the form the member is modelled with
/// (`^arn:aws…:events:…:([0-9]{12})?:connection\/<name>\/<id>$`), built here so a
/// row can name a connection this account does not hold.
fn connection_arn(account: &str, region: &str, name: &str, id: &str) -> String {
    format!("arn:aws:events:{region}:{account}:connection/{name}/{id}")
}

/// A rule carrying an `EventPattern`, so a row has a rule to resolve.
async fn given_rule(router: &MultiServiceRouter, bus: Option<&str>, name: &str) {
    let mut body = json!({ "Name": name, "EventPattern": r#"{"source":["com.demo"]}"# });
    if let Some(bus) = bus {
        body["EventBusName"] = json!(bus);
    }
    let (status, answered) = events(router, "PutRule", &body).await;
    assert_eq!(status, 200, "PutRule({name}) answered {status} {answered}");
}

/// An `API_KEY` connection, which needs no authorization exchange.
async fn given_connection(router: &MultiServiceRouter, name: &str) {
    let (status, answered) = events(
        router,
        "CreateConnection",
        &json!({
            "Name": name,
            "AuthorizationType": "API_KEY",
            "AuthParameters": {
                "ApiKeyAuthParameters": { "ApiKeyName": "k", "ApiKeyValue": "v" },
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConnection({name}) answered {status} {answered}"
    );
}

/// A request past a resource COUNT quota is refused with the one fixed sentence,
/// which discloses neither the resource nor the limit.
///
/// The rows differ along the quota reached (targets on one rule, event buses in
/// one Region) and along HOW the request reaches it: one request past the cap in
/// a single list, and a run of requests that walks up to it.
#[tokio::test]
async fn a_request_past_a_count_quota_is_refused_with_the_fixed_sentence() {
    // The sentence under test, written here rather than imported from the
    // implementation.
    let expected = "The requested resource exceeds the maximum number allowed.";

    // (case, the request that must be refused, text that must NOT appear in the
    //  refusal)
    let router = emulator();
    given_rule(&router, None, "fanout").await;
    let over_in_one_request: Vec<Value> = (0..=state::MAX_TARGETS_PER_RULE)
        .map(|index| json!({ "Id": format!("t{index}"), "Arn": target_arn("orders") }))
        .collect();

    let (status, refused) = events(
        &router,
        "PutTargets",
        &json!({ "Rule": "fanout", "Targets": over_in_one_request }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a target list past the per-rule cap must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "expected LimitExceededException, got {refused}"
    );
    assert_eq!(
        message(&refused),
        expected,
        "a quota refusal states the fixed sentence, got {:?}",
        message(&refused)
    );
    for leaked in ["fanout", &state::MAX_TARGETS_PER_RULE.to_string()] {
        assert!(
            !message(&refused).contains(leaked),
            "a quota refusal discloses neither the resource nor the limit, but {leaked:?} is \
             in {:?}",
            message(&refused)
        );
    }

    // The same quota, reached one target at a time: every target up to the cap
    // lands, and the one past it is refused in the same words.
    let router = emulator();
    given_rule(&router, None, "one-at-a-time").await;
    for index in 0..state::MAX_TARGETS_PER_RULE {
        let (status, answered) = events(
            &router,
            "PutTargets",
            &json!({
                "Rule": "one-at-a-time",
                "Targets": [{ "Id": format!("t{index}"), "Arn": target_arn("orders") }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "target {index} is within the cap, got {status} {answered}"
        );
    }
    let (status, refused) = events(
        &router,
        "PutTargets",
        &json!({
            "Rule": "one-at-a-time",
            "Targets": [{ "Id": "one-too-many", "Arn": target_arn("orders") }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "the target past the cap must be refused, got {status} {refused}"
    );
    assert_eq!(
        message(&refused),
        expected,
        "expected the fixed sentence, got {:?}",
        message(&refused)
    );

    // Another quota entirely — event buses in one Region — is refused in exactly
    // the same words. The run stops at the first refusal rather than assuming
    // which create reaches the quota.
    let router = emulator();
    let mut refused_at = None;
    for index in 0..=state::MAX_EVENT_BUSES {
        let (status, answered) = events(
            &router,
            "CreateEventBus",
            &json!({ "Name": format!("bus-{index}") }),
        )
        .await;
        if status != 200 {
            refused_at = Some((index, status, answered));
            break;
        }
    }
    let (index, status, refused) =
        refused_at.expect("a create past the per-Region bus quota must be refused");
    assert_eq!(
        status, 400,
        "bus {index} is past the quota and must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "expected LimitExceededException, got {refused}"
    );
    assert_eq!(
        message(&refused),
        expected,
        "every count quota states the same sentence, got {:?}",
        message(&refused)
    );
}

/// A `ScheduleExpression` this service will not take is refused by naming the
/// parameter alone — no reason, and nothing of the submitted text.
///
/// The rows differ along the FORM of the rejected expression: text that is no
/// expression at all, a known prefix spelled wrongly, a form with empty
/// arguments, and one with its parenthesis missing. The accepted rows are the two
/// forms the member documents, so the refusal cannot have come from rejecting
/// every value.
#[tokio::test]
async fn a_schedule_expression_refusal_names_the_parameter_and_nothing_else() {
    let expected = "Parameter ScheduleExpression is not valid.";
    // (case, the expression, whether this service takes it)
    let rows: [(&str, &str, bool); 9] = [
        ("text that is no expression at all", "every friday", false),
        ("a rate without its parentheses", "rate 5 minutes", false),
        ("a rate with nothing in it", "rate()", false),
        ("a cron with nothing in it", "cron()", false),
        ("a cron missing its closing parenthesis", "cron(0 20 * * ? *", false),
        ("an expression of another scheduler", "@daily", false),
        // The two forms `PutRule$ScheduleExpression` documents: "cron(0 20 * * ? *)"
        // or "rate(5 minutes)".
        ("the documented rate form", "rate(5 minutes)", true),
        ("the documented cron form", "cron(0 20 * * ? *)", true),
        ("a rate in another unit", "rate(1 hour)", true),
    ];

    for (index, (case, expression, accepted)) in rows.iter().enumerate() {
        let router = emulator();
        let (status, answered) = events(
            &router,
            "PutRule",
            &json!({
                "Name": format!("scheduled-{index}"),
                "ScheduleExpression": expression,
            }),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a documented form is accepted, got {status} {answered}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered}"
        );
        let actual = message(&answered);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// A second create under one connection name is refused by stating the name the
/// request carried, quoted, and that a connection with it exists already.
///
/// The rows differ along the FORM of the name (plain, dotted, dashed,
/// underscored, one character, the modelled 64-character maximum).
#[tokio::test]
async fn a_duplicate_connection_name_is_stated_quoted() {
    for name in [
        "orders",
        "orders.v2",
        "orders-v2",
        "orders_v2",
        "c",
        &"n".repeat(64),
    ] {
        let router = emulator();
        given_connection(&router, name).await;
        let (status, refused) = events(
            &router,
            "CreateConnection",
            &json!({
                "Name": name,
                "AuthorizationType": "BASIC",
                "AuthParameters": {
                    "BasicAuthParameters": { "Username": "u", "Password": "p" },
                },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: a taken name must be refused, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceAlreadyExistsException",
            "case {name:?}: expected ResourceAlreadyExistsException, got {refused}"
        );
        let expected = format!(
            "Failed to create the connection(s). A connection with name '{name}' already exists."
        );
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {name:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// A create that resolves a connection by ARN states the ARN the request SENT,
/// verbatim.
///
/// The rows differ along WHY the ARN resolves to nothing: a name this account
/// holds no connection under, a name it does hold under another id, another
/// account, another Region — the cases a name parsed out of the ARN could not
/// tell apart.
#[tokio::test]
async fn a_missing_connection_is_stated_by_the_arn_the_request_sent() {
    let router = emulator();
    let held = "held";
    given_connection(&router, held).await;
    let account = engine::emulator_account_id();

    // (case, the ConnectionArn the request carries)
    let rows: [(&str, String); 4] = [
        (
            "a connection name this account holds none under",
            connection_arn(account, REGION, "absent", "0123-4567"),
        ),
        (
            "a name this account holds, under another id",
            connection_arn(account, REGION, held, "0000-0000"),
        ),
        (
            "the same name in another account",
            connection_arn("210987654321", REGION, held, "0123-4567"),
        ),
        (
            "the same name in another Region",
            connection_arn(account, "eu-west-1", held, "0123-4567"),
        ),
    ];

    for (index, (case, arn)) in rows.iter().enumerate() {
        let (status, refused) = events(
            &router,
            "CreateApiDestination",
            &json!({
                "Name": format!("destination-{index}"),
                "ConnectionArn": arn,
                "InvocationEndpoint": "https://api.example.com/events",
                "HttpMethod": "POST",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused}"
        );
        let expected = format!(
            "Failed to create the api-destination(s). Connection '{arn}' does not exist."
        );
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The bus is resolved BEFORE the rule: a request naming a bus this account does
/// not hold reads that the bus is missing, and the same request against a bus
/// that exists reads that the rule is.
///
/// The rows differ along the operation that ran the lookup (a read, two state
/// writes, a target read, a target write, a target removal) and along the bus
/// named (one that exists, one that does not, one whose name carries the
/// partner-bus `/`).
#[tokio::test]
async fn a_missing_bus_is_reported_before_the_rule_it_was_asked_about() {
    let router = emulator();
    let bus = "orders-bus";
    let (status, answered) = events(&router, "CreateEventBus", &json!({ "Name": bus })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {answered}");
    given_rule(&router, Some(bus), "stored").await;

    let rule = "absent-rule";
    // (case, the EventBusName the request carries, the bus the lookup runs on,
    //  whether that bus exists)
    let buses: [(&str, Option<&str>, &str, bool); 4] = [
        ("the implicit default bus", None, DEFAULT_BUS, true),
        ("a custom bus that exists", Some(bus), bus, true),
        ("a bus that does not exist", Some("ghost-bus"), "ghost-bus", false),
        (
            "a bus in the partner-bus form, which does not exist",
            Some("aws.partner/example.com/orders"),
            "aws.partner/example.com/orders",
            false,
        ),
    ];

    for (case, member, named, exists) in buses {
        let with_bus = |mut body: Value| {
            if let Some(member) = member {
                body["EventBusName"] = json!(member);
            }
            body
        };
        let requests: [(&str, Value); 6] = [
            ("DescribeRule", with_bus(json!({ "Name": rule }))),
            ("DisableRule", with_bus(json!({ "Name": rule }))),
            ("EnableRule", with_bus(json!({ "Name": rule }))),
            ("ListTargetsByRule", with_bus(json!({ "Rule": rule }))),
            (
                "PutTargets",
                with_bus(json!({
                    "Rule": rule,
                    "Targets": [{ "Id": "t1", "Arn": target_arn("orders") }],
                })),
            ),
            (
                "RemoveTargets",
                with_bus(json!({ "Rule": rule, "Ids": ["t1"] })),
            ),
        ];
        // The missing resource is the BUS wherever the request named one this
        // account does not hold, and the RULE only where the bus is there to hold
        // it.
        let expected = if exists {
            format!("Rule {rule} does not exist on EventBus {named}.")
        } else {
            format!("Event bus {named} does not exist.")
        };

        for (operation, body) in requests {
            let (status, refused) = events(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: expected a refusal, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {refused}"
            );
            let actual = message(&refused);
            assert_eq!(
                actual, expected,
                "case {case:?}/{operation}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    // The rule that IS stored on that bus is served, so the refusals above are
    // about the resources they name rather than about every request.
    let (status, answered) = events(
        &router,
        "DescribeRule",
        &json!({ "Name": "stored", "EventBusName": bus }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the stored rule is readable on its own bus, got {status} {answered}"
    );
}

/// A global endpoint that would replicate with no role to replicate as is refused
/// with one entirely fixed sentence, on both writers of an endpoint.
///
/// The rows differ along how the configuration reaches that state (replication
/// left out, so it defaults to `ENABLED`; replication named explicitly) and along
/// the writer. The accepted rows are the two configurations that do hold —
/// replication off with no role, replication on with one — so the sentence cannot
/// have come from refusing every write.
#[tokio::test]
async fn replication_with_no_role_is_refused_with_a_fixed_sentence() {
    let expected = "When replication is enabled, role cannot be empty.";
    let account = engine::emulator_account_id();
    let role = format!("arn:aws:iam::{account}:role/replication");
    let routing = json!({
        "FailoverConfig": {
            "Primary": { "HealthCheck": "arn:aws:route53:::healthcheck/abcdef0123456789" },
            "Secondary": { "Route": "us-east-1" },
        },
    });
    let buses = json!([
        { "EventBusArn": format!("arn:aws:events:{REGION}:{account}:event-bus/orders") },
        { "EventBusArn": format!("arn:aws:events:us-east-1:{account}:event-bus/orders") },
    ]);

    // (case, the members beyond the name, whether this service takes it)
    let rows: [(&str, Value, bool); 4] = [
        (
            "replication left out, which defaults it to ENABLED",
            json!({ "RoutingConfig": routing, "EventBuses": buses }),
            false,
        ),
        (
            "replication named ENABLED with no role",
            json!({
                "RoutingConfig": routing,
                "EventBuses": buses,
                "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
            }),
            false,
        ),
        (
            "replication DISABLED, which needs no role",
            json!({
                "RoutingConfig": routing,
                "EventBuses": buses,
                "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
            }),
            true,
        ),
        (
            "replication ENABLED with the role it runs as",
            json!({
                "RoutingConfig": routing,
                "EventBuses": buses,
                "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
                "RoleArn": role,
            }),
            true,
        ),
    ];

    for (index, (case, members, accepted)) in rows.iter().enumerate() {
        let router = emulator();
        let name = format!("endpoint-{index}");
        let mut body = json!({ "Name": name });
        for (member, value) in members.as_object().expect("the members are an object") {
            body[member] = value.clone();
        }
        let (status, answered) = events(&router, "CreateEndpoint", &body).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: this configuration holds, got {status} {answered}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered}"
        );
        let actual = message(&answered);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // The same configuration reached by a WRITE: an endpoint created with
    // replication off is refused the same way when a write would switch it on
    // with no role.
    let router = emulator();
    let name = "written";
    let (status, created) = events(
        &router,
        "CreateEndpoint",
        &json!({
            "Name": name,
            "RoutingConfig": routing,
            "EventBuses": buses,
            "ReplicationConfig": { "State": state::REPLICATION_STATE_DISABLED },
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateEndpoint answered {status} {created}");
    let (status, refused) = events(
        &router,
        "UpdateEndpoint",
        &json!({
            "Name": name,
            "ReplicationConfig": { "State": state::REPLICATION_STATE_ENABLED },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a write that switches replication on with no role must be refused, got \
         {status} {refused}"
    );
    let actual = message(&refused);
    assert_eq!(
        actual, expected,
        "both writers state the same sentence, expected {expected:?}, got {actual:?}"
    );
}

/// One entry as a refusal of the `Entries` list prints it.
///
/// The ten members the validation layer in front of EventBridge prints, in its
/// order, with one the request did not carry printed as `null`. Written out here
/// rather than imported from the implementation, so the table states the form
/// independently of the code it is checking. Seven of the members are this
/// model's own (`smithy_query(query="shape", shape="PutEventsRequestEntry")`:
/// `Time Source Resources DetailType Detail EventBusName TraceHeader`); the other
/// three are members of the object that layer prints, which no member of a request
/// can set.
fn printed_entry(
    time: &str,
    source: &str,
    resources: &str,
    detail_type: &str,
    detail: &str,
    event_bus_name: &str,
    trace_header: &str,
) -> String {
    format!(
        "PutEventsRequestEntry(time={time}, source={source}, resources={resources}, \
         detailType={detail_type}, eventGroupId=null, detail={detail}, \
         eventBusName={event_bus_name}, traceHeader={trace_header}, kmsKeyIdentifier=null, \
         internalMetadata=null)"
    )
}

/// A clause on the `Entries` list states the LIST the caller submitted, not a
/// count of it.
///
/// The rows differ along which bound is broken (the maximum, the minimum), how far
/// past it the list is, and which members its entries carry — the part of the
/// rendering a count summary loses.
#[tokio::test]
async fn a_refused_entries_list_states_the_entries_that_were_submitted() {
    /// `PutEventsRequestEntryList` is modelled `@length(min: 1, max: 10)`.
    const MAX_ENTRIES: usize = 10;
    const MIN_ENTRIES: usize = 1;

    let minimal = json!({ "Source": "a", "DetailType": "b", "Detail": "{}" });
    let minimal_printed = printed_entry("null", "a", "null", "b", "{}", "null", "null");
    let whole = json!({
        // `awsJson1_1` carries a timestamp as epoch SECONDS; this is
        // 2026-03-04T10:00:00Z, the RFC 3339 instant `Time` is documented in.
        "Time": 1_772_618_400_i64,
        "Source": "com.demo.orders",
        "Resources": ["arn:aws:s3:::bucket", "arn:aws:s3:::other"],
        "DetailType": "Order Placed",
        "Detail": r#"{"id":1}"#,
        "EventBusName": "orders-bus",
        "TraceHeader": "Root=1-5759e988-bd862e3fe1be46a994272793",
    });
    let whole_printed = printed_entry(
        "2026-03-04T10:00:00Z",
        "com.demo.orders",
        "[arn:aws:s3:::bucket, arn:aws:s3:::other]",
        "Order Placed",
        r#"{"id":1}"#,
        "orders-bus",
        "Root=1-5759e988-bd862e3fe1be46a994272793",
    );

    // (case, the entries the request carries, how each of them prints, the
    //  requirement the clause states)
    let rows: Vec<(&str, Vec<Value>, Vec<String>, String)> = vec![
        (
            "one entry past the maximum, each carrying the required members only",
            vec![minimal.clone(); MAX_ENTRIES + 1],
            vec![minimal_printed.clone(); MAX_ENTRIES + 1],
            format!("Member must have length less than or equal to {MAX_ENTRIES}"),
        ),
        (
            "well past the maximum, with every member of the model set on one entry",
            [vec![whole.clone()], vec![minimal.clone(); MAX_ENTRIES + 4]].concat(),
            [
                vec![whole_printed.clone()],
                vec![minimal_printed.clone(); MAX_ENTRIES + 4],
            ]
            .concat(),
            format!("Member must have length less than or equal to {MAX_ENTRIES}"),
        ),
        (
            "every entry carrying every member of the model",
            vec![whole.clone(); MAX_ENTRIES + 1],
            vec![whole_printed.clone(); MAX_ENTRIES + 1],
            format!("Member must have length less than or equal to {MAX_ENTRIES}"),
        ),
        (
            "no entry at all, which is under the minimum",
            Vec::new(),
            Vec::new(),
            format!("Member must have length greater than or equal to {MIN_ENTRIES}"),
        ),
    ];

    let router = emulator();
    for (case, entries, printed, requirement) in rows {
        let (status, refused) = events(&router, "PutEvents", &json!({ "Entries": entries })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {refused}"
        );
        let expected = format!(
            "1 validation error detected: Value '[{}]' at 'entries' failed to satisfy \
             constraint: {requirement}",
            printed.join(", ")
        );
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// A list whose rendering is past the ceiling one clause may quote keeps the
/// framework's own count summary: a caller controls both how many entries it sends
/// and how large each one is, so a refusal is never sized by the request.
///
/// The rows differ along how far past the ceiling the rendering grows.
#[tokio::test]
async fn an_entries_list_too_large_to_quote_keeps_the_count_summary() {
    /// Headroom over the ceiling one clause of this tree is held to, so the
    /// assertion is about the bound rather than about its exact value.
    const WELL_OVER_ANY_CEILING: usize = 512 * 1024;

    // (case, how many entries, how many bytes of `Detail` each carries)
    let rows: [(&str, usize, usize); 2] = [
        ("eleven entries of tens of kilobytes each", 11, 64 * 1024),
        ("eleven entries carrying one large detail each", 11, 128 * 1024),
    ];

    let router = emulator();
    for (case, count, detail_bytes) in rows {
        let detail = json!({ "pad": "p".repeat(detail_bytes) }).to_string();
        let entries: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "Source": format!("com.demo.{index}"),
                    "DetailType": "Padded",
                    "Detail": detail,
                })
            })
            .collect();
        let (status, refused) = events(&router, "PutEvents", &json!({ "Entries": entries })).await;
        assert_eq!(status, 400, "case {case:?}: expected a refusal, got {status}");
        let actual = message(&refused);
        let expected = format!(
            "1 validation error detected: Value '{count} items' at 'entries' failed to satisfy \
             constraint: Member must have length less than or equal to 10"
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the count summary, got {:?}",
            &actual[..actual.len().min(160)]
        );
        assert!(
            actual.len() < WELL_OVER_ANY_CEILING,
            "case {case:?}: the refusal must not be sized by the request, got {} bytes for a \
             request carrying {} bytes of detail",
            actual.len(),
            count * detail_bytes
        );
    }
}
