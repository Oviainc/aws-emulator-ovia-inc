// SPDX-License-Identifier: CC0-1.0

//! An EVENT BUS as the target of a rule, end to end over the wire through the
//! real `MultiServiceRouter` with SQS mounted beside EventBridge — including a
//! bus in another Region, which is why every call here names the Region it is
//! signed for.
//!
//! The RULE asserted here: a target whose `Arn` names an event bus is reached by
//! PUBLISHING the event onto that bus ("set that account's event bus as a target
//! of the rules in your account … specify that account's event bus as the `Arn`
//! value when you run `PutTargets`", `PutTargets`), where it is matched by THAT
//! bus's own rules and delivered to their targets. The copy is the same event
//! with a NEW event id: every other member of the envelope is carried over, the
//! `region` it was published in included, so a rule of the receiving Region sees
//! the Region the event came from.
//!
//! The rows differ along where the target bus lives (the publisher's Region,
//! another Region), along what the receiving bus's rules do with the copy (match
//! it, not match it), and along the cases that deliver nothing: a bus the account
//! does not hold, and a chain of bus targets past the bound this emulator puts on
//! one.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    ComAmazonawsEventbridgeAwseventsState, InvocationOutcome,
};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the event is PUBLISHED in — the source Region every delivered copy
/// must still name.
const SOURCE_REGION: &str = "eu-west-2";

/// The Region the receiving bus lives in.
const OTHER_REGION: &str = "us-east-1";

/// The source and detail type every rule here matches on.
const SOURCE: &str = "com.demo.orders";
const DETAIL_TYPE: &str = "Order Created";
const DETAIL: &str = r#"{"orderId":"A-1"}"#;

/// The emulator: EventBridge and SQS on one engine, because the far end of a
/// cross-region delivery is a queue of the receiving Region.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router, engine }
}

/// One `awsJson` RPC call signed for `region` — the credential scope is what
/// says which Region's resources the call reaches.
async fn rpc(
    emulator: &Emulator,
    region: &str,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
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

/// One EventBridge call in `region`, asserting a 200.
async fn events(emulator: &Emulator, region: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        emulator, region, "events", "AWSEvents", "1.1", operation, body,
    )
    .await;
    assert_eq!(
        status, 200,
        "{operation} in {region} answered {status} {parsed}"
    );
    parsed
}

/// One SQS call in `region`, asserting a 200.
async fn sqs(emulator: &Emulator, region: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        emulator, region, "sqs", "AmazonSQS", "1.0", operation, body,
    )
    .await;
    assert_eq!(
        status, 200,
        "{operation} in {region} answered {status} {parsed}"
    );
    parsed
}

/// A queue called `name` in `region`, carrying the policy a rule's target needs:
/// EventBridge "relies on resource-based policies" for a queue (`PutTargets`), so one
/// that has granted the service nothing receives nothing — the rule
/// `tests/all/queue_target_policy_authorization.rs` is the suite for. Answers its url
/// and its ARN.
async fn given_queue(emulator: &Emulator, region: &str, name: &str) -> (String, String) {
    let created = sqs(emulator, region, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {created}"))
        .to_string();
    let attributes = sqs(
        emulator,
        region,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {attributes}"))
        .to_string();
    sqs(
        emulator,
        region,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "Policy": crate::target_dispatch::granting_events(&arn) },
        }),
    )
    .await;
    (url, arn)
}

/// The ARN of the bus `name` in `region`, as a rule's target names it.
fn bus_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:events:{region}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// The role a bus target is invoked under. It is created nowhere: a `RoleArn` the
/// account holds no role for is evidence of nothing, so it never bars a delivery.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/events-cross-region",
        engine::emulator_account_id()
    )
}

/// A rule in `region` on `bus` matching the published event, carrying `targets`.
async fn given_rule(
    emulator: &Emulator,
    region: &str,
    bus: &str,
    name: &str,
    pattern: Value,
    targets: Value,
) {
    events(
        emulator,
        region,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus,
            "EventPattern": pattern.to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    let written = events(
        emulator,
        region,
        "PutTargets",
        &json!({ "Rule": name, "EventBusName": bus, "Targets": targets }),
    )
    .await;
    assert_eq!(
        written["FailedEntryCount"],
        json!(0),
        "case {name}: PutTargets reported a failed entry: {written}"
    );
}

/// The pattern that matches the event every test below publishes.
fn matching_pattern() -> Value {
    json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] })
}

/// Publish the one event these tests route, in `SOURCE_REGION`, and answer its
/// `EventId`.
async fn publish(emulator: &Emulator) -> String {
    let published = events(
        emulator,
        SOURCE_REGION,
        "PutEvents",
        &json!({
            "Entries": [{ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": DETAIL }],
        }),
    )
    .await;
    assert_eq!(
        published["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {published}"
    );
    published["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("no EventId in {published}"))
        .to_string()
}

/// Every message body waiting on the queue at `url` in `region`.
async fn drain(emulator: &Emulator, region: &str, url: &str) -> Vec<String> {
    let received = sqs(
        emulator,
        region,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// What EventBridge recorded about each handover: `(target arn, outcome)`.
fn invocations(emulator: &Emulator) -> Vec<(String, InvocationOutcome)> {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .invocations
        .iter()
        .map(|invocation| (invocation.target_arn.clone(), invocation.outcome.clone()))
        .collect()
}

/// A bus target delivers the event onto the bus it names, where that bus's own
/// rules match the copy — over a TABLE whose rows differ along the Region the
/// receiving bus lives in.
#[tokio::test]
async fn a_bus_target_publishes_the_event_onto_the_bus_it_names() {
    // (case, the Region the receiving bus is created in)
    let rows = [
        ("a bus of the publisher's own Region", SOURCE_REGION),
        ("a bus of another Region", OTHER_REGION),
    ];

    for (case, region) in rows {
        let emulator = emulator();
        events(
            &emulator,
            region,
            "CreateEventBus",
            &json!({ "Name": "receiving" }),
        )
        .await;
        let (url, queue_arn) = given_queue(&emulator, region, "receiving-queue").await;
        given_rule(
            &emulator,
            region,
            "receiving",
            "receives",
            matching_pattern(),
            json!([{ "Id": "q", "Arn": queue_arn }]),
        )
        .await;
        let target = bus_arn(region, "receiving");
        given_rule(
            &emulator,
            SOURCE_REGION,
            "default",
            "forwards",
            matching_pattern(),
            json!([{ "Id": "bus", "Arn": target, "RoleArn": role_arn() }]),
        )
        .await;

        let published = publish(&emulator).await;

        let recorded = invocations(&emulator);
        assert_eq!(
            recorded,
            vec![
                (target.clone(), InvocationOutcome::Invoked),
                (queue_arn.clone(), InvocationOutcome::Invoked),
            ],
            "case {case:?}: expected the bus target and then the receiving rule's queue \
             target to be invoked, got {recorded:?}"
        );

        let bodies = drain(&emulator, region, &url).await;
        assert_eq!(
            bodies.len(),
            1,
            "case {case:?}: expected the receiving rule's target to hold one copy, \
             got {bodies:?}"
        );
        let copy: Value = serde_json::from_str(&bodies[0])
            .unwrap_or_else(|error| panic!("case {case:?}: the copy is not JSON ({error})"));
        // The copy carries the SOURCE Region and a NEW event id; everything else
        // the publication fixed is carried over.
        assert_eq!(
            copy["region"], json!(SOURCE_REGION),
            "case {case:?}: expected the copy to name the source Region, got {copy}"
        );
        let delivered_id = copy["id"].as_str().unwrap_or_default();
        assert!(
            !delivered_id.is_empty() && delivered_id != published,
            "case {case:?}: expected a new event id on the copy, got {delivered_id:?} for a \
             publication of {published:?}"
        );
        for member in ["source", "detail-type", "account", "time", "version"] {
            assert!(
                copy.get(member).is_some_and(|value| !value.is_null()),
                "case {case:?}: expected the copy to carry {member}, got {copy}"
            );
        }
        assert_eq!(
            copy["detail"], json!({ "orderId": "A-1" }),
            "case {case:?}: expected the published detail on the copy, got {copy}"
        );
        assert_eq!(
            copy["source"], json!(SOURCE),
            "case {case:?}: expected the published source on the copy, got {copy}"
        );
    }
}

/// What the receiving bus's rules decide is theirs alone: a copy no rule of that
/// bus matches is delivered nowhere, and the publication is still a success.
#[tokio::test]
async fn a_copy_no_rule_of_the_receiving_bus_matches_is_delivered_nowhere() {
    let emulator = emulator();
    events(
        &emulator,
        OTHER_REGION,
        "CreateEventBus",
        &json!({ "Name": "receiving" }),
    )
    .await;
    let (url, queue_arn) = given_queue(&emulator, OTHER_REGION, "near-miss-queue").await;
    given_rule(
        &emulator,
        OTHER_REGION,
        "receiving",
        "near-miss",
        json!({ "source": ["com.other.app"] }),
        json!([{ "Id": "q", "Arn": queue_arn }]),
    )
    .await;
    let target = bus_arn(OTHER_REGION, "receiving");
    given_rule(
        &emulator,
        SOURCE_REGION,
        "default",
        "forwards",
        matching_pattern(),
        json!([{ "Id": "bus", "Arn": target, "RoleArn": role_arn() }]),
    )
    .await;

    publish(&emulator).await;

    let recorded = invocations(&emulator);
    assert_eq!(
        recorded,
        vec![(target, InvocationOutcome::Invoked)],
        "expected only the bus target to be invoked, got {recorded:?}"
    );
    let bodies = drain(&emulator, OTHER_REGION, &url).await;
    assert!(
        bodies.is_empty(),
        "a rule of the receiving bus that does not match the copy delivers nothing, \
         got {bodies:?}"
    );
}

/// A bus the account does not hold is nothing to publish onto: the handover is
/// recorded as not attempted rather than counted as a delivery.
#[tokio::test]
async fn a_bus_target_naming_no_bus_of_this_account_is_not_attempted() {
    let rows = [
        (
            "a bus no CreateEventBus wrote",
            bus_arn(OTHER_REGION, "never-created"),
        ),
        (
            // The bus exists, but in another Region: the bus a target names is
            // the one in the Region its own ARN names.
            "a bus of a Region the bus was not created in",
            bus_arn("ap-south-1", "receiving"),
        ),
    ];

    for (case, target) in rows {
        let emulator = emulator();
        events(
            &emulator,
            OTHER_REGION,
            "CreateEventBus",
            &json!({ "Name": "receiving" }),
        )
        .await;
        given_rule(
            &emulator,
            SOURCE_REGION,
            "default",
            "forwards",
            matching_pattern(),
            json!([{ "Id": "bus", "Arn": target, "RoleArn": role_arn() }]),
        )
        .await;

        publish(&emulator).await;

        let recorded = invocations(&emulator);
        let attempted = recorded
            .iter()
            .map(|(arn, outcome)| (arn.clone(), matches!(outcome, InvocationOutcome::Invoked)))
            .collect::<Vec<_>>();
        assert_eq!(
            attempted,
            vec![(target.clone(), false)],
            "case {case:?}: expected the handover to {target} to be recorded as not \
             invoked, got {recorded:?}"
        );
    }
}

/// A CHAIN of bus targets is followed, and only as far as this emulator's bound
/// on one: the copy that has already crossed that many buses is not re-published
/// again, so two buses naming each other cannot loop for ever.
#[tokio::test]
async fn a_chain_of_bus_targets_is_followed_up_to_its_bound() {
    let emulator = emulator();
    // Three buses in a row, each forwarding to the next, with a queue at every
    // step so where the chain stopped is observable.
    let mut queues: Vec<(String, String)> = Vec::new();
    for name in ["first", "second", "third"] {
        events(
            &emulator,
            OTHER_REGION,
            "CreateEventBus",
            &json!({ "Name": name }),
        )
        .await;
        let (url, queue_arn) = given_queue(&emulator, OTHER_REGION, &format!("{name}-queue")).await;
        queues.push((name.to_string(), url));
        let mut targets = vec![json!({ "Id": "q", "Arn": queue_arn })];
        if let Some(next) = ["second", "third", ""]
            .get(queues.len() - 1)
            .filter(|next| !next.is_empty())
        {
            targets.push(json!({
                "Id": "bus",
                "Arn": bus_arn(OTHER_REGION, next),
                "RoleArn": role_arn(),
            }));
        }
        given_rule(
            &emulator,
            OTHER_REGION,
            name,
            &format!("{name}-rule"),
            matching_pattern(),
            Value::Array(targets),
        )
        .await;
    }
    given_rule(
        &emulator,
        SOURCE_REGION,
        "default",
        "forwards",
        matching_pattern(),
        json!([{
            "Id": "bus",
            "Arn": bus_arn(OTHER_REGION, "first"),
            "RoleArn": role_arn(),
        }]),
    )
    .await;

    publish(&emulator).await;

    // Two hops are followed — onto `first` and onto `second` — and the third is
    // not, which is the bound stated once in the handler.
    let expected = [("first", 1usize), ("second", 1), ("third", 0)];
    for (bus, copies) in expected {
        let url = queues
            .iter()
            .find(|(name, _)| name == bus)
            .map(|(_, url)| url.clone())
            .unwrap_or_else(|| panic!("case {bus:?}: the queue of this bus was created"));
        let bodies = drain(&emulator, OTHER_REGION, &url).await;
        assert_eq!(
            bodies.len(),
            copies,
            "case {bus:?}: expected {copies} copy/copies on this bus's queue, got {bodies:?}"
        );
    }
}
