// SPDX-License-Identifier: CC0-1.0

//! The ENTRY-LEVEL semantics of `PutEvents` and `RemoveTargets`, end to end over
//! the wire (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`)
//! through the real `MultiServiceRouter` — so every case exercises routing,
//! deserialization, the handler, the persisted store and serialization.
//!
//! Each test asserts a RULE over a TABLE of inputs rather than one recorded
//! shape:
//!
//! * a per-entry failure reports a defect of the ENTRY, so an entry naming a bus
//!   this account does not hold is ingested — an `EventId`, no failure — and
//!   merely routed nowhere;
//! * the published per-entry size bound is charged on the REQUEST and is
//!   INCLUSIVE: entries totalling exactly the maximum are ingested and keep
//!   their bytes, and only more than that is refused — as a whole call;
//! * an entry's `Detail` may nest JSON as deep as the operation documents,
//!   whatever containers it nests, and a document within that bound is parsed
//!   (so a rule reaching into it matches) rather than reported malformed;
//! * removing a target is idempotent: an `Id` the rule does not carry asks for a
//!   state that already holds, so nothing fails and a repeat call answers the
//!   same way.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// The largest size one entry may have, in bytes, and it is INCLUSIVE: "the
/// total entry size must be less than 1MB" (`PutEvents`'s own documentation,
/// `smithy_query(query="operation", operation="PutEvents")`), where that
/// megabyte is 2^20 bytes. Spelled out here rather than imported, so a wrong
/// bound in the implementation cannot make these rows pass.
const DOCUMENTED_MAX_ENTRY_BYTES: usize = 1_048_576;

/// How deeply an entry's `Detail` may nest JSON containers: "PutEvents will only
/// process nested JSON up to 1000 levels deep" (the same documentation).
/// Spelled out here for the same reason.
const DOCUMENTED_MAX_DETAIL_DEPTH: usize = 1000;

/// The bytes the documented size calculation charges for an entry that carries a
/// `Time` ("Calculating PutEvents event entry size": 14 bytes if the `Time` is
/// provided, plus the length of `Source`, `DetailType`, `Detail` and each
/// resource ARN).
const TIME_SIZE_BYTES: usize = 14;

/// The per-entry code for a `Detail` that is not a JSON document this service
/// can route: "The JSON provided is not valid" (`PutEventsResultEntry$ErrorCode`).
const MALFORMED_DETAIL: &str = "MalformedDetail";

/// The `Source` and `DetailType` every published entry below carries.
const SOURCE: &str = "com.demo.widgets";
const DETAIL_TYPE: &str = "Order Created";

/// The emulator: one router carrying only this service over one shared engine,
/// plus that engine, so a test can read the state the handlers wrote.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

async fn call(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.eu-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header("authorization", format!("AWS4-HMAC-SHA256 Credential={credential}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a
/// success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` an error body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// One record of the routing log `PutEvents` appended to: the event's id, the
/// bus it was routed on, the `Detail` text it carried and the rules it matched —
/// read from state, so what the service did with an entry is observed rather
/// than inferred from the response.
struct Routed {
    event_id: String,
    bus: String,
    detail: Option<String>,
    matched_rules: Vec<String>,
}

fn routed(emulator: &Emulator) -> Vec<Routed> {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .delivered
        .iter()
        .map(|event| Routed {
            event_id: event.event_id.clone(),
            bus: event.bus.clone(),
            detail: event.detail.clone(),
            matched_rules: event.matched_rules.clone(),
        })
        .collect()
}

/// A rule matching every event that carries a `source`, on `bus`.
async fn put_catch_all_rule(emulator: &Emulator, name: &str, bus: Option<&str>) {
    let mut request = json!({ "Name": name, "EventPattern": r#"{"source":[{"exists":true}]}"# });
    if let Some(bus) = bus {
        request["EventBusName"] = json!(bus);
    }
    let (status, body) = call(emulator, "PutRule", &request).await;
    assert_eq!(status, 200, "PutRule {name} answered {status} {body}");
}

/// The `EventId` of the one entry of a single-entry `PutEvents` answer, with the
/// whole answer asserted to be an ingestion: 200, no failures, an id.
fn ingested(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(status, 200, "case {case:?}: PutEvents answered {status}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected FailedEntryCount 0, got {}",
        body["FailedEntryCount"]
    );
    let entry = &body["Entries"][0];
    let id = entry["EventId"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty(),
        "case {case:?}: expected an ingested entry to carry an EventId, got {entry}"
    );
    assert!(
        entry.get("ErrorCode").is_none_or(Value::is_null),
        "case {case:?}: expected no per-entry failure, got {entry}"
    );
    id
}

/// The `ErrorCode` of the one entry of a single-entry `PutEvents` answer, with
/// the whole answer asserted to be a per-entry refusal: 200, one failure, no id.
fn refused(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(status, 200, "case {case:?}: PutEvents answered {status}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(1),
        "case {case:?}: expected FailedEntryCount 1, got {}",
        body["FailedEntryCount"]
    );
    let entry = &body["Entries"][0];
    assert!(
        entry.get("EventId").is_none_or(Value::is_null),
        "case {case:?}: expected a refused entry to carry no EventId, got {entry}"
    );
    assert!(
        entry
            .get("ErrorMessage")
            .and_then(Value::as_str)
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: expected a refused entry to say why, got {entry}"
    );
    entry["ErrorCode"].as_str().unwrap_or_default().to_string()
}

/// A per-entry failure reports a defect of the ENTRY. An entry's destination is
/// not one: an entry naming a bus this account does not hold is INGESTED, with
/// its own `EventId` and no failure, and the event is simply routed nowhere. The
/// rows differ along the form of the bus name and whether a bus of that name
/// exists.
#[tokio::test]
async fn an_entry_naming_an_absent_bus_is_ingested_and_routed_nowhere() {
    let emulator = emulator();
    let (status, body) = call(&emulator, "CreateEventBus", &json!({ "Name": "orders-bus" })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");
    put_catch_all_rule(&emulator, "on-default", None).await;
    put_catch_all_rule(&emulator, "on-orders", Some("orders-bus")).await;

    // (case, the `EventBusName` the entry carries, the bus the event is routed
    // on when one of that name exists). A name differing from a stored bus only
    // in case names nothing, because bus names are case sensitive.
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("the implicit default bus", None, Some("default")),
        ("the default bus, named explicitly", Some("default"), Some("default")),
        ("a custom bus that exists", Some("orders-bus"), Some("orders-bus")),
        ("a bus that was never created", Some("never-created"), None),
        ("an absent name in dotted form", Some("ghost.bus.dotted"), None),
        ("an absent name in underscored form", Some("ghost_bus"), None),
        ("a stored name differing only in case", Some("Orders-Bus"), None),
        ("an absent name carrying digits and dashes", Some("ghost-bus-2026"), None),
    ];

    let mut expected_log: Vec<(String, String)> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for (case, named_bus, routed_on) in &rows {
        let mut entry = json!({
            "Source": SOURCE,
            "DetailType": DETAIL_TYPE,
            "Detail": r#"{"amount":1}"#,
        });
        if let Some(bus) = named_bus {
            entry["EventBusName"] = json!(bus);
        }
        let (status, body) = call(&emulator, "PutEvents", &json!({ "Entries": [entry] })).await;
        let id = ingested(case, status, &body);
        if let Some(bus) = routed_on {
            expected_log.push((id.clone(), (*bus).to_string()));
        }
        ids.push(id);
    }

    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every ingested entry gets its own EventId, got {ids:?}"
    );

    // Only the events that reached a bus of this account are in the routing log,
    // each under the id the response reported and on the bus it named: an event
    // published to an absent bus is dropped, not recorded somewhere else.
    let log: Vec<(String, String)> = routed(&emulator)
        .into_iter()
        .map(|event| (event.event_id, event.bus))
        .collect();
    assert_eq!(
        log, expected_log,
        "expected the routing log {expected_log:?}, got {log:?}"
    );
}

/// The documented size of one entry: 14 bytes when it carries a `Time`, plus the
/// UTF-8 length of `Source`, `DetailType` and `Detail`, plus the length of every
/// resource ARN ("Calculating PutEvents event entry size").
fn documented_size(entry: &Value) -> usize {
    let text = |member: &str| {
        entry
            .get(member)
            .and_then(Value::as_str)
            .map_or(0, str::len)
    };
    let resources: usize = entry
        .get("Resources")
        .and_then(Value::as_array)
        .map(|arns| arns.iter().map(|arn| arn.as_str().map_or(0, str::len)).sum())
        .unwrap_or_default();
    usize::from(entry.get("Time").is_some()) * TIME_SIZE_BYTES
        + text("Source")
        + text("DetailType")
        + text("Detail")
        + resources
}

/// An entry whose documented size is exactly `size` bytes: the members the row
/// asks for, with `Detail` padded to make up the difference.
fn entry_of_size(size: usize, with_time: bool, resources: &[String]) -> Value {
    let mut entry = json!({ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": "{}" });
    if with_time {
        entry["Time"] = json!(1_767_225_600_i64);
    }
    if !resources.is_empty() {
        entry["Resources"] = json!(resources);
    }
    let shell = r#"{"pad":""}"#;
    let overhead = documented_size(&entry) - "{}".len() + shell.len();
    let padding = size
        .checked_sub(overhead)
        .unwrap_or_else(|| panic!("a {size}-byte entry cannot hold {overhead} bytes of members"));
    entry["Detail"] = json!(format!(r#"{{"pad":"{}"}}"#, "a".repeat(padding)));
    assert_eq!(
        documented_size(&entry),
        size,
        "the row's entry must measure {size} bytes"
    );
    entry
}

/// The published per-entry size bound is on the REQUEST and INCLUSIVE: entries
/// totalling exactly the documented maximum are ingested and keep every byte
/// they carried, and only MORE than that is refused — as a whole call, not as a
/// per-entry failure. The rows straddle the bound and differ along how many
/// entries carry the bytes and which members they sit in.
#[tokio::test]
async fn a_request_is_ingested_up_to_the_published_size_bound_inclusive() {
    let emulator = emulator();
    put_catch_all_rule(&emulator, "catch-all", None).await;
    let account = engine::emulator_account_id();
    let arns = vec![
        format!("arn:aws:sqs:{REGION}:{account}:orders"),
        "arn:aws:s3:::demo-bucket".to_string(),
    ];
    let half = DOCUMENTED_MAX_ENTRY_BYTES / 2;

    // (case, the documented size of each entry of the request, whether the
    // entries carry a Time, their resource ARNs)
    let rows: Vec<(&str, Vec<usize>, bool, &[String])> = vec![
        ("one entry well under the bound", vec![4_096], false, &[]),
        (
            "one entry one byte under the bound",
            vec![DOCUMENTED_MAX_ENTRY_BYTES - 1],
            false,
            &[],
        ),
        (
            "one entry exactly at the bound",
            vec![DOCUMENTED_MAX_ENTRY_BYTES],
            false,
            &[],
        ),
        (
            "one entry at the bound, with the Time charge",
            vec![DOCUMENTED_MAX_ENTRY_BYTES],
            true,
            &[],
        ),
        (
            "one entry at the bound, with resource ARNs charged",
            vec![DOCUMENTED_MAX_ENTRY_BYTES],
            false,
            &arns,
        ),
        (
            "two entries totalling exactly the bound",
            vec![half, DOCUMENTED_MAX_ENTRY_BYTES - half],
            false,
            &[],
        ),
        (
            "ten entries well under the bound together",
            vec![1_024; 10],
            false,
            &[],
        ),
        (
            "one entry one byte past the bound",
            vec![DOCUMENTED_MAX_ENTRY_BYTES + 1],
            false,
            &[],
        ),
        (
            "one entry one byte past the bound, with the Time charge",
            vec![DOCUMENTED_MAX_ENTRY_BYTES + 1],
            true,
            &[],
        ),
        (
            "two entries one byte past the bound together",
            vec![half, DOCUMENTED_MAX_ENTRY_BYTES - half + 1],
            false,
            &[],
        ),
        (
            "one entry well past the bound",
            vec![DOCUMENTED_MAX_ENTRY_BYTES + 4_096],
            false,
            &[],
        ),
    ];

    let mut expected_details: Vec<String> = Vec::new();
    let mut refusals: Vec<String> = Vec::new();
    for (case, sizes, with_time, resources) in &rows {
        let entries: Vec<Value> = sizes
            .iter()
            .map(|size| entry_of_size(*size, *with_time, resources))
            .collect();
        let total: usize = entries.iter().map(documented_size).sum();
        let (status, body) =
            call(&emulator, "PutEvents", &json!({ "Entries": entries.clone() })).await;
        if total > DOCUMENTED_MAX_ENTRY_BYTES {
            assert_eq!(
                status, 400,
                "case {case:?}: {total} bytes must refuse the whole request, got {status}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            assert!(
                body.get("Entries").is_none() && body.get("FailedEntryCount").is_none(),
                "case {case:?}: a refused request reports no per-entry envelope, got {body}"
            );
            let said = message(&body);
            assert!(
                !said.is_empty() && !said.contains(&total.to_string()),
                "case {case:?}: the refusal states the condition, not the request's \
                 {total} bytes, got {said:?}"
            );
            refusals.push(said.to_string());
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: {total} bytes is within the bound, got {status}"
        );
        assert_eq!(
            body["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected FailedEntryCount 0, got {body}"
        );
        let results = body["Entries"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            entries.len(),
            "case {case:?}: expected one result per entry, got {body}"
        );
        for (index, result) in results.iter().enumerate() {
            assert!(
                result["EventId"].as_str().is_some_and(|id| !id.is_empty()),
                "case {case:?} entry {index}: expected an EventId, got {result}"
            );
        }
        for entry in &entries {
            expected_details.push(entry["Detail"].as_str().unwrap_or_default().to_string());
        }
    }

    // The refusal is a verdict on the request's total, so every row that passes
    // the bound reads the same: the wording is a constant of the condition.
    let mut spoken = refusals.clone();
    spoken.sort();
    spoken.dedup();
    assert_eq!(
        spoken.len(),
        1,
        "expected one wording for every oversized request, got {refusals:?}"
    );

    // Every ingested entry reached the bus with the bytes it was published with:
    // an entry at the bound delivers intact rather than being truncated.
    let recorded: Vec<Option<String>> = routed(&emulator)
        .into_iter()
        .map(|event| event.detail)
        .collect();
    assert_eq!(
        recorded.len(),
        expected_details.len(),
        "expected {} routed events, got {}",
        expected_details.len(),
        recorded.len()
    );
    for (index, (actual, expected)) in recorded.iter().zip(&expected_details).enumerate() {
        assert_eq!(
            actual.as_deref().map(str::len),
            Some(expected.len()),
            "event {index}: expected a Detail of {} bytes, got {:?}",
            expected.len(),
            actual.as_deref().map(str::len)
        );
        assert_eq!(
            actual.as_deref(),
            Some(expected.as_str()),
            "event {index}: the routed Detail must be the published one"
        );
    }
}

/// The containers a `Detail` nests, for the depth table: the shape says what is
/// nested, the depth says how many levels — counting the outermost object, which
/// `Detail` must be ("a valid JSON object", `PutEventsRequestEntry$Detail`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Objects,
    Arrays,
    Mixed,
}

/// A `Detail` nesting `depth` levels of `shape`, with `"a"` as every object key
/// so one pattern reaches into any of them, and a LEAF beside the nesting at the
/// outermost level.
///
/// The leaf is what the depth table's rule matches on: a pattern term compares a
/// LEAF value ("exists only works on leaf nodes. It does not work on intermediate
/// nodes"), so a document the handler did not really parse — reported as an empty
/// object — matches nothing, whatever the nesting below the leaf looks like. It
/// adds no depth: it sits in the outermost object, which is level 0 either way.
fn nested_detail(shape: Shape, depth: usize) -> String {
    assert!(depth >= 1, "a Detail object nests at least one level");
    let mut document = "1".to_string();
    for level in (0..depth).rev() {
        // The outermost level is always an object; below it the shape decides.
        let as_object = match shape {
            Shape::Objects => true,
            Shape::Arrays => level == 0,
            Shape::Mixed => level % 2 == 0,
        };
        document = if as_object {
            if level == 0 {
                format!(r#"{{"a":{document},"{PARSE_PROBE_KEY}":{PARSE_PROBE_VALUE}}}"#)
            } else {
                format!(r#"{{"a":{document}}}"#)
            }
        } else {
            format!("[{document}]")
        };
    }
    document
}

/// The leaf member of a nested `Detail` the depth table's rule matches on, and
/// its value ([`nested_detail`]).
const PARSE_PROBE_KEY: &str = "parsed";
const PARSE_PROBE_VALUE: i32 = 7;

/// The stack the depth table drives the handler on: the size the emulator's own
/// router gives each tokio WORKER thread (`core/router/src/main.rs`'s
/// `WORKER_STACK_SIZE`), which is where request handling — and so the deepest
/// read of a document a request carries — actually runs when a client calls the
/// emulator. A test-harness thread is sized for ordinary test code (a couple of
/// megabytes), which a document at the published nesting bound is right at the
/// edge of, so a test that drives the handler directly supplies the server's own
/// figure rather than relying on the harness's.
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Run `work` on a thread sized like one of the emulator's own workers (see
/// [`WORKER_STACK_BYTES`]), under a runtime of its own. The future is built
/// INSIDE that thread, so nothing of the emulator has to cross a thread
/// boundary to get there.
fn on_a_worker_sized_thread<F>(work: fn() -> F)
where
    F: ::std::future::Future<Output = ()> + 'static,
{
    ::std::thread::Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(move || {
            ::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("the test runtime starts")
                .block_on(work());
        })
        .expect("the worker-sized thread starts")
        .join()
        .expect("the worker-sized thread finishes");
}

/// An entry's `Detail` may nest JSON as deep as the operation documents —
/// whatever containers it nests — and a document within that bound is PARSED,
/// so a rule reaching into it matches the event; only a document deeper than the
/// bound is refused. The rows span the bound (including the depth a parser's own
/// default stops at), each over objects, arrays and mixed nesting.
#[test]
fn a_detail_nests_to_the_documented_depth_whatever_it_nests() {
    on_a_worker_sized_thread(nesting_depth_rows);
}

async fn nesting_depth_rows() {
    let emulator = emulator();
    // The pattern reaches one level INTO the detail, so it matches only if the
    // document was really parsed rather than taken as an empty object. It
    // compares the document's LEAF member, because a pattern term matches a leaf
    // value: "exists only works on leaf nodes. It does not work on intermediate
    // nodes", so a clause on the nesting itself would say nothing about the parse.
    let (status, body) = call(
        &emulator,
        "PutRule",
        &json!({
            "Name": "reaches-into-detail",
            "EventPattern": json!({
                "detail": { PARSE_PROBE_KEY: [PARSE_PROBE_VALUE] },
            })
            .to_string(),
        }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");
    // A target, so a matched event's whole envelope — the parsed detail included
    // — is rendered and handed over, not merely recorded.
    let account = engine::emulator_account_id();
    let (status, body) = call(
        &emulator,
        "PutTargets",
        &json!({
            "Rule": "reaches-into-detail",
            "Targets": [{
                "Id": "queue",
                "Arn": format!("arn:aws:sqs:{REGION}:{account}:deep-detail"),
            }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");

    // (case, nesting depth, whether the entry is ingested)
    let depths: Vec<(&str, usize, bool)> = vec![
        ("a flat object", 1, true),
        ("two levels", 2, true),
        ("127 levels", 127, true),
        ("128 levels", 128, true),
        ("129 levels", 129, true),
        ("500 levels", 500, true),
        ("one level under the bound", DOCUMENTED_MAX_DETAIL_DEPTH - 1, true),
        ("exactly at the bound", DOCUMENTED_MAX_DETAIL_DEPTH, true),
        ("one level past the bound", DOCUMENTED_MAX_DETAIL_DEPTH + 1, false),
        ("well past the bound", DOCUMENTED_MAX_DETAIL_DEPTH + 64, false),
    ];

    let mut expected_matches = 0usize;
    for shape in [Shape::Objects, Shape::Arrays, Shape::Mixed] {
        for (label, depth, accepted) in &depths {
            let case = format!("{shape:?}: {label}");
            let detail = nested_detail(shape, *depth);
            let (status, body) = call(
                &emulator,
                "PutEvents",
                &json!({
                    "Entries": [{
                        "Source": SOURCE,
                        "DetailType": DETAIL_TYPE,
                        "Detail": detail,
                    }],
                }),
            )
            .await;
            if *accepted {
                ingested(&case, status, &body);
                expected_matches += 1;
            } else {
                let code = refused(&case, status, &body);
                assert_eq!(
                    code, MALFORMED_DETAIL,
                    "case {case:?}: expected ErrorCode {MALFORMED_DETAIL}, got {code:?}"
                );
                continue;
            }

            // The accepted document was parsed: the rule that reaches into
            // `detail` matched this event, and the text was recorded as sent.
            let log = routed(&emulator);
            let last = log
                .last()
                .unwrap_or_else(|| panic!("case {case:?}: the ingested event must be recorded"));
            assert_eq!(
                last.matched_rules,
                vec!["reaches-into-detail".to_string()],
                "case {case:?}: expected the rule reaching into detail to match, got {:?}",
                last.matched_rules
            );
            assert_eq!(
                last.detail.as_deref(),
                Some(detail.as_str()),
                "case {case:?}: the routed Detail must be the published one"
            );
        }
    }
    assert_eq!(
        routed(&emulator).len(),
        expected_matches,
        "expected {expected_matches} routed events, got {}",
        routed(&emulator).len()
    );
}

/// Removing a target is idempotent: an `Id` the rule does not carry asks for a
/// state that already holds, so nothing fails — `FailedEntryCount` 0 and an
/// empty `FailedEntries` — and the same call made twice answers the same way and
/// leaves the same list. The rows differ along how many ids are asked for, how
/// many name a stored target, and the form of the ids that name none.
#[tokio::test]
async fn removing_a_target_is_idempotent_about_an_unattached_id() {
    // The ids stored on the rule before each row runs.
    let stored = ["t1", "t2", "t3"];
    // (case, the ids to remove, the ids left on the rule afterwards)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one stored id", vec!["t2"], vec!["t1", "t3"]),
        ("every stored id", vec!["t1", "t2", "t3"], vec![]),
        ("an id the rule never carried", vec!["never-existed"], vec!["t1", "t2", "t3"]),
        (
            "a stored id and an unattached one together",
            vec!["t1", "never-existed"],
            vec!["t2", "t3"],
        ),
        ("no id of the request naming a target", vec!["ghost-a", "ghost-b"], vec!["t1", "t2", "t3"]),
        // Target ids are case sensitive, so this one names nothing.
        ("a stored id differing only in case", vec!["T1"], vec!["t1", "t2", "t3"]),
        ("an id repeated within one request", vec!["t3", "t3"], vec!["t1", "t2"]),
        (
            "an id in the dotted form, naming nothing",
            vec!["never.existed.dotted"],
            vec!["t1", "t2", "t3"],
        ),
    ];

    for (case, ids, left) in rows {
        let emulator = emulator();
        let rule = "removal-probe";
        put_catch_all_rule(&emulator, rule, None).await;
        let account = engine::emulator_account_id();
        let targets: Vec<Value> = stored
            .iter()
            .map(|id| json!({ "Id": id, "Arn": format!("arn:aws:sqs:{REGION}:{account}:{id}") }))
            .collect();
        let (status, body) = call(
            &emulator,
            "PutTargets",
            &json!({ "Rule": rule, "Targets": targets }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: PutTargets answered {status} {body}");

        // The same removal twice: a removal of ids that are now gone is the same
        // call as one of ids that were never there, so both answer no failures
        // and the second changes nothing.
        for attempt in 1..=2 {
            let (status, body) = call(
                &emulator,
                "RemoveTargets",
                &json!({ "Rule": rule, "Ids": ids }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: RemoveTargets answered {status} {body}"
            );
            assert_eq!(
                body["FailedEntryCount"],
                json!(0),
                "case {case:?} attempt {attempt}: expected FailedEntryCount 0, got {body}"
            );
            assert_eq!(
                body["FailedEntries"],
                json!([]),
                "case {case:?} attempt {attempt}: expected an empty FailedEntries, got {body}"
            );

            // Round trip: the reader reports exactly the targets the removal
            // left, in their stored order.
            let (status, body) =
                call(&emulator, "ListTargetsByRule", &json!({ "Rule": rule })).await;
            assert_eq!(
                status, 200,
                "case {case:?} attempt {attempt}: ListTargetsByRule answered {status} {body}"
            );
            let remaining: Vec<String> = body["Targets"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|target| target["Id"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                remaining, left,
                "case {case:?} attempt {attempt}: expected the targets {left:?}, got {remaining:?}"
            );
        }
    }

    // A rule that is not there is still a missing ENTITY, which is the modelled
    // refusal of the whole call rather than a per-entry row.
    let emulator = emulator();
    let (status, body) = call(
        &emulator,
        "RemoveTargets",
        &json!({ "Rule": "never-created", "Ids": ["t1"] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a removal on a rule that does not exist is refused, got {status} {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "ResourceNotFoundException",
        "expected ResourceNotFoundException for an absent rule, got {body}"
    );
}
