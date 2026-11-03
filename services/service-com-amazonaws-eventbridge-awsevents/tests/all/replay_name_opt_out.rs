// SPDX-License-Identifier: CC0-1.0

//! What a REPLAYED event carries, and which rules of the destination bus it
//! therefore reaches, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`,
//! with SQS mounted on the same engine so a redelivery is observed where a
//! client observes it: on the queue a rule's target names.
//!
//! The RULES asserted here:
//!
//! * a replayed event carries the `replay-name` of the replay that re-published
//!   it, and it is matched against the rules of the destination bus WITH that
//!   field present — so a rule whose pattern requires `replay-name` to be absent
//!   receives the live event and NOT the replayed one, which is the opt-out a
//!   consumer uses to keep a replay from re-processing its history, while a rule
//!   that does not mention the field receives both;
//! * "only events that occurred between the EventStartTime and EventEndTime are
//!   replayed": of a batch published with several event times, exactly those
//!   whose own time lies in the window are re-published, each with its ORIGINAL
//!   time, and the rest are never re-published at all;
//! * `Destination.FilterArns`, "a list of ARNs for rules to replay events to",
//!   confines that routing to the rules it names — so a rule the filters leave
//!   out receives nothing even though its pattern matches, and a filter naming
//!   only a rule whose own pattern excludes replayed events redelivers nothing.
//!
//! The rows differ along which rule mentions `replay-name`, where the window
//! falls relative to each event's own time, and which rules the filters name.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-east-2";

/// The source and detail type every event below carries.
const SOURCE: &str = "com.demo.ledger";
const DETAIL_TYPE: &str = "Entry Posted";

/// The event times the batch is published with, as offsets in seconds from the
/// row's anchor: one well before the window, two inside it.
const OUTSIDE: i64 = -7_200;
const INSIDE_EARLY: i64 = -3_600;
const INSIDE_LATE: i64 = -3_540;
/// Half the width of the window the replay selects, around the 1 hour mark.
const BAND: i64 = 300;

/// The emulator: EventBridge and SQS on one engine.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router }
}

/// One `awsJson` RPC call against the service whose signing name is
/// `signing_name` — the credential scope is what routes it.
async fn rpc(
    emulator: &Emulator,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
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

async fn events(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "events", "AWSEvents", "1.1", operation, body).await
}

async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// A required string member of a response body.
fn member(case: &str, body: &Value, name: &str) -> String {
    body[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: no {name} in {body}"))
        .to_string()
}

/// Epoch seconds now, which is what an `awsJson1_1` time stamp carries.
fn epoch_now() -> i64 {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now()).secs()
}

/// A queue called `name`, carrying the policy a rule's target needs: EventBridge
/// "relies on resource-based policies" for a queue (`PutTargets`), so one that has
/// granted the service nothing receives nothing — the rule
/// `tests/all/queue_target_policy_authorization.rs` is the suite for. Answers its url
/// and the ARN a rule names it by.
async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let (status, body) = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(status, 200, "CreateQueue answered {status} {body}");
    let url = member(name, &body, "QueueUrl");
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(status, 200, "GetQueueAttributes answered {status} {body}");
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueArn in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "Policy": crate::target_dispatch::granting_events(&arn) },
        }),
    )
    .await;
    assert_eq!(status, 200, "SetQueueAttributes answered {status} {body}");
    (url, arn)
}

/// Every envelope waiting on the queue at `url`, parsed.
async fn drain(emulator: &Emulator, url: &str) -> Vec<Value> {
    let (status, body) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(status, 200, "ReceiveMessage answered {status} {body}");
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let text = message["Body"].as_str().unwrap_or_default();
            serde_json::from_str::<Value>(text)
                .unwrap_or_else(|error| panic!("a delivered body is not JSON ({error}): {text:?}"))
        })
        .collect()
}

/// One rule of the scenario: its name, the ARN a filter names it by, and the
/// url of the queue its one target is.
struct Rule {
    name: String,
    arn: String,
    queue: String,
}

/// A rule called `name` on `bus` carrying `pattern`, with one SQS queue target.
async fn given_rule(emulator: &Emulator, bus: &str, name: &str, pattern: &Value) -> Rule {
    let (queue, queue_arn) = given_queue(emulator, name).await;
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus,
            "EventPattern": pattern.to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutRule answered {status} {body}");
    let arn = member(name, &body, "RuleArn");
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": name,
            "EventBusName": bus,
            "Targets": [{ "Id": "t1", "Arn": queue_arn }],
        }),
    )
    .await;
    assert_eq!(status, 200, "case {name}: PutTargets answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {name}: PutTargets reported a failed entry: {body}"
    );
    Rule {
        name: name.to_string(),
        arn,
        queue,
    }
}

/// The `id` of every envelope in `bodies`.
fn ids(bodies: &[Value]) -> Vec<String> {
    bodies
        .iter()
        .map(|body| body["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// A replayed event is matched against the destination bus's rules WITH its
/// `replay-name` present, so a rule that requires the field to be absent is the
/// opt-out that keeps a replay out of it — while the window decides which
/// archived events are re-published at all, and `FilterArns` which rules they
/// are offered to.
#[tokio::test]
async fn a_rule_requiring_no_replay_name_receives_the_live_event_but_not_the_replayed_one() {
    let emulator = emulator();
    let (status, body) = events(&emulator, "CreateEventBus", &json!({ "Name": "ledger" })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");
    let bus_arn = member("bus", &body, "EventBusArn");

    // Two rules whose patterns differ ONLY in whether they mention
    // `replay-name`: one takes whatever matches, the other opts out of replays.
    let both = given_rule(
        &emulator,
        "ledger",
        "takes-replays",
        &json!({ "source": [SOURCE] }),
    )
    .await;
    let live_only = given_rule(
        &emulator,
        "ledger",
        "live-only",
        &json!({ "source": [SOURCE], state::REPLAY_NAME_FIELD: [{ "exists": false }] }),
    )
    .await;

    // No `EventPattern` on the archive: "if you do not specify a pattern to
    // filter events sent to the archive, all events are sent to the archive".
    let (status, body) = events(
        &emulator,
        "CreateArchive",
        &json!({ "ArchiveName": "ledger-archive", "EventSourceArn": bus_arn }),
    )
    .await;
    assert_eq!(status, 200, "CreateArchive answered {status} {body}");
    let archive_arn = member("archive", &body, "ArchiveArn");

    // One batch, three event times: one well before the window and two inside
    // it. The anchor is shared with the window below, so a bound falls exactly
    // where this row means it to.
    let anchor = epoch_now();
    let offsets = [OUTSIDE, INSIDE_EARLY, INSIDE_LATE];
    let entries: Vec<Value> = offsets
        .iter()
        .enumerate()
        .map(|(index, offset)| {
            json!({
                "EventBusName": "ledger",
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "entry": index }).to_string(),
                "Time": anchor + offset,
            })
        })
        .collect();
    let (status, body) = events(&emulator, "PutEvents", &json!({ "Entries": entries })).await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {body}"
    );
    let published: Vec<String> = body["Entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["EventId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(published.len(), 3, "every event is ingested, got {published:?}");
    let in_window: Vec<String> = published.iter().skip(1).cloned().collect();

    let (status, body) = events(
        &emulator,
        "DescribeArchive",
        &json!({ "ArchiveName": "ledger-archive" }),
    )
    .await;
    assert_eq!(status, 200, "DescribeArchive answered {status} {body}");
    assert_eq!(
        body["EventCount"].as_i64().unwrap_or_default(),
        3,
        "the archive has to hold the published events before they can be replayed: {body}"
    );

    // The LIVE deliveries: both rules match a published event, and neither
    // envelope carries a replay name. The times are kept, so a redelivered
    // envelope can be compared against them.
    let mut live_times: Vec<(String, Value)> = Vec::new();
    for rule in [&both, &live_only] {
        let bodies = drain(&emulator, &rule.queue).await;
        assert_eq!(
            bodies.len(),
            3,
            "case live delivery to {}: every published event matches this rule, got {bodies:?}",
            rule.name
        );
        for body in &bodies {
            assert!(
                body.get(state::REPLAY_NAME_FIELD).is_none(),
                "case live delivery to {}: a live delivery carries no replay name, got {body}",
                rule.name
            );
        }
        if rule.name == both.name {
            live_times = bodies
                .iter()
                .map(|body| {
                    (
                        body["id"].as_str().unwrap_or_default().to_string(),
                        body["time"].clone(),
                    )
                })
                .collect();
        }
    }

    // The replay: a window around the 1 hour mark, offered to every rule of the
    // bus.
    let replay = "ledger-replay";
    let (status, body) = events(
        &emulator,
        "StartReplay",
        &json!({
            "ReplayName": replay,
            "EventSourceArn": archive_arn,
            "EventStartTime": anchor + INSIDE_EARLY - BAND,
            "EventEndTime": anchor + INSIDE_LATE + BAND,
            "Destination": { "Arn": bus_arn },
        }),
    )
    .await;
    assert_eq!(status, 200, "StartReplay answered {status} {body}");

    let redelivered = drain(&emulator, &both.queue).await;
    let mut got = ids(&redelivered);
    got.sort();
    let mut expected = in_window.clone();
    expected.sort();
    assert_eq!(
        got, expected,
        "case a rule that takes replays: exactly the events whose own time lies in the window \
         are redelivered, got {redelivered:?}"
    );
    for body in &redelivered {
        assert_eq!(
            body[state::REPLAY_NAME_FIELD],
            json!(replay),
            "case a rule that takes replays: a redelivered event carries the replay's name, got \
             {body}"
        );
        let id = body["id"].as_str().unwrap_or_default().to_string();
        let was = live_times
            .iter()
            .find(|(seen, _)| *seen == id)
            .map(|(_, time)| time.clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            body["time"], was,
            "case a rule that takes replays: a redelivered event keeps its ORIGINAL time, got \
             {body}"
        );
    }

    let opted_out = drain(&emulator, &live_only.queue).await;
    assert!(
        opted_out.is_empty(),
        "case a rule requiring no replay name: the replay must not reach it, got {opted_out:?}"
    );

    // And a second replay confined to the rule that opts out reaches nothing at
    // all: the filters leave the other rule out, and the named rule's own
    // pattern excludes a replayed event.
    let (status, body) = events(
        &emulator,
        "StartReplay",
        &json!({
            "ReplayName": "ledger-replay-filtered",
            "EventSourceArn": archive_arn,
            "EventStartTime": anchor + INSIDE_EARLY - BAND,
            "EventEndTime": anchor + INSIDE_LATE + BAND,
            "Destination": { "Arn": bus_arn, "FilterArns": [live_only.arn] },
        }),
    )
    .await;
    assert_eq!(status, 200, "the filtered StartReplay answered {status} {body}");
    for rule in [&both, &live_only] {
        let bodies = drain(&emulator, &rule.queue).await;
        assert!(
            bodies.is_empty(),
            "case a filter naming only the rule that opts out: nothing is redelivered to {}, got \
             {bodies:?}",
            rule.name
        );
    }

    // A SENTINEL, so the two silences above are not read off a window that was
    // simply too narrow: one more live event, published after them, reaches both
    // rules. A delivery observed by the next drain is what proves a drain of
    // this same shape would have seen a redelivery had there been one.
    let (status, body) = events(
        &emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": "ledger",
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "entry": "sentinel" }).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(status, 200, "the sentinel PutEvents answered {status} {body}");
    for rule in [&both, &live_only] {
        let bodies = drain(&emulator, &rule.queue).await;
        assert_eq!(
            bodies.len(),
            1,
            "case the sentinel: a live event published after the silences reaches {}, so the \
             drains above were wide enough to have seen a redelivery, got {bodies:?}",
            rule.name
        );
    }
}
