// SPDX-License-Identifier: CC0-1.0

//! How an event bus's timestamps are reported, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through
//! the real `MultiServiceRouter`.
//!
//! ONE rule, asserted over a table of readers and a table of buses: a timestamp
//! member is reported only when the service holds the instant it describes. The
//! account's `default` bus is brought into existence by no call — it is there
//! before the first request — so it has no creation instant and reports neither
//! `CreationTime` nor `LastModifiedTime`, while every bus `CreateEventBus`
//! wrote reports both; a change to a bus is what `LastModifiedTime` reports
//! from then on, and a change never invents a creation instant.
//!
//! The rows differ along the reader (`ListEventBuses` unfiltered, by prefix and
//! paged one bus at a time; `DescribeEventBus` naming nothing, naming the bus
//! and naming its ARN), along which members a create carries, and along which
//! operation performs the change.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for, and so the region segment of
/// every ARN these tests build.
const REGION: &str = "us-west-2";

/// The bus the account holds without creating it: "your account's default event
/// bus receives events from Amazon Web Services services" (`EventBus`), so no
/// call of this API brought it into existence. Spelled out here rather than
/// imported, so a wrong name in the implementation cannot make an assertion
/// pass.
const DEFAULT_BUS: &str = "default";

/// The two timestamp members of `EventBus`
/// (`smithy_query(query="shape", shape="EventBus")`), both optional.
const TIMESTAMPS: [&str; 2] = ["CreationTime", "LastModifiedTime"];

/// The emulator: one router carrying only this service over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    Emulator { router }
}

/// This emulator's account id — the account segment of every ARN it mints.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// `arn:aws:events:<region>:<account>:event-bus/<name>`, built here rather than
/// read back out of a response.
fn bus_arn(name: &str) -> String {
    format!("arn:aws:events:{REGION}:{}:event-bus/{name}", account())
}

async fn call(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, \
         SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.us-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
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

/// A member of a response object, with an absent member and an explicit null
/// treated alike: both mean "not reported".
fn member(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// Every bus `ListEventBuses` reports for `request`, paging `NextToken` to
/// exhaustion so a bus is never missed because it fell past a page boundary.
async fn list_buses(emulator: &Emulator, case: &str, request: &Value) -> Vec<Value> {
    let mut page_request = request.clone();
    let mut buses: Vec<Value> = Vec::new();
    // One more round than there can be pages, so a token that never clears
    // fails the case instead of looping forever.
    for _ in 0..32 {
        let (status, body) = call(emulator, "ListEventBuses", &page_request).await;
        assert_eq!(
            status, 200,
            "case {case}: ListEventBuses answered {status} {body}"
        );
        buses.extend(
            body["EventBuses"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected an EventBuses list, got {body}"))
                .iter()
                .cloned(),
        );
        let Some(token) = body.get("NextToken").and_then(Value::as_str) else {
            return buses;
        };
        page_request["NextToken"] = json!(token);
    }
    panic!("case {case}: ListEventBuses never stopped handing back a NextToken");
}

/// The entry `ListEventBuses` reports for the bus named `name`.
fn entry_for(case: &str, buses: &[Value], name: &str) -> Value {
    buses
        .iter()
        .find(|bus| bus["Name"].as_str() == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected an entry for {name}, got {buses:?}"))
}

/// Assert that `entry` names the bus `name` and reports neither timestamp: the
/// representation of a bus no call created.
fn assert_uncreated(case: &str, name: &str, entry: &Value) {
    assert_eq!(
        entry["Name"],
        json!(name),
        "case {case}: expected Name {name}, got {entry}"
    );
    assert_eq!(
        entry["Arn"],
        json!(bus_arn(name)),
        "case {case}: expected Arn {}, got {entry}",
        bus_arn(name)
    );
    for timestamp in TIMESTAMPS {
        assert_eq!(
            member(entry, timestamp),
            Value::Null,
            "case {case}: no call created {name}, so it must report no {timestamp}, got {entry}"
        );
    }
}

/// The instant a timestamp member carries, in epoch seconds.
fn instant(case: &str, entry: &Value, name: &str) -> f64 {
    member(entry, name)
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a {name} instant, got {entry}"))
}

/// Every reader of a bus answers the same way about the bus no call created: it
/// is named, it carries an account- and region-scoped ARN, and it reports
/// neither timestamp — while the buses created beside it report both.
#[tokio::test]
async fn every_reader_reports_the_uncreated_bus_without_timestamps() {
    let emulator = emulator();
    // Created beside the default bus, so each reader is answering about a list
    // that holds both kinds at once, and so the paged rows below span more than
    // one page.
    let created = ["alpha", "orders", "zeta"];
    for name in created {
        let (status, body) = call(&emulator, "CreateEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "CreateEventBus {name} answered {status} {body}"
        );
    }

    // The rows differ along which reader is asked, and how: the whole list, a
    // prefix that admits only the default bus, a prefix that admits every bus,
    // and the list paged one bus at a time.
    let list_rows: Vec<(&str, Value)> = vec![
        ("ListEventBuses, no filter", json!({})),
        (
            "ListEventBuses, a prefix matching only the default bus",
            json!({ "NamePrefix": DEFAULT_BUS }),
        ),
        (
            "ListEventBuses, a one-character prefix",
            json!({ "NamePrefix": "d" }),
        ),
        ("ListEventBuses, one bus per page", json!({ "Limit": 1 })),
        ("ListEventBuses, two buses per page", json!({ "Limit": 2 })),
    ];
    for (case, request) in list_rows {
        let buses = list_buses(&emulator, case, &request).await;
        assert_uncreated(case, DEFAULT_BUS, &entry_for(case, &buses, DEFAULT_BUS));
        // Only the buses a prefix admits are listed, and each of them is one a
        // call created, so each reports both timestamps.
        for bus in &buses {
            let name = bus["Name"].as_str().unwrap_or_default();
            if name == DEFAULT_BUS {
                continue;
            }
            for timestamp in TIMESTAMPS {
                assert!(
                    member(bus, timestamp).as_f64().is_some(),
                    "case {case}: {name} was created by a call, so it must report {timestamp}, \
                     got {bus}"
                );
            }
        }
    }

    // The same bus through `DescribeEventBus`: naming nothing ("if you omit
    // this, the default event bus is displayed"), naming it, and naming its ARN.
    let describe_rows: Vec<(&str, Value)> = vec![
        ("DescribeEventBus, no Name", json!({})),
        (
            "DescribeEventBus, the bare name",
            json!({ "Name": DEFAULT_BUS }),
        ),
        (
            "DescribeEventBus, the bus ARN",
            json!({ "Name": bus_arn(DEFAULT_BUS) }),
        ),
    ];
    for (case, request) in describe_rows {
        let (status, body) = call(&emulator, "DescribeEventBus", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: DescribeEventBus answered {status} {body}"
        );
        assert_uncreated(case, DEFAULT_BUS, &body);
    }
}

/// A bus a call created reports both timestamps, equal while nothing has
/// changed it, through both readers. The rows differ along which members the
/// create carried.
#[tokio::test]
async fn a_created_bus_reports_the_instant_its_create_ran() {
    let emulator = emulator();
    let creates: Vec<(&str, Value)> = vec![
        ("the name alone", json!({ "Name": "bare" })),
        (
            "a description",
            json!({ "Name": "described", "Description": "the orders bus" }),
        ),
        (
            "a customer managed key",
            json!({
                "Name": "encrypted",
                "KmsKeyIdentifier": "1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
        ),
        (
            "a dead-letter queue and a log configuration",
            json!({
                "Name": "configured",
                "DeadLetterConfig": {
                    "Arn": format!("arn:aws:sqs:{REGION}:{}:spill", account()),
                },
                "LogConfig": { "IncludeDetail": "FULL", "Level": "TRACE" },
            }),
        ),
        (
            "tags",
            json!({ "Name": "tagged", "Tags": [{ "Key": "team", "Value": "orders" }] }),
        ),
    ];

    for (case, request) in creates {
        let name = request["Name"].as_str().expect("the row names the bus");
        let (status, body) = call(&emulator, "CreateEventBus", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: CreateEventBus answered {status} {body}"
        );

        let (status, described) =
            call(&emulator, "DescribeEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case}: DescribeEventBus answered {status} {described}"
        );
        let creation = instant(case, &described, "CreationTime");
        assert!(
            creation > 0.0,
            "case {case}: expected a CreationTime after the epoch, got {creation}"
        );
        assert_eq!(
            member(&described, "LastModifiedTime"),
            member(&described, "CreationTime"),
            "case {case}: a bus nothing has changed was last modified when it was created, \
             got {described}"
        );

        let listed = list_buses(&emulator, case, &json!({ "NamePrefix": name })).await;
        let entry = entry_for(case, &listed, name);
        for timestamp in TIMESTAMPS {
            assert_eq!(
                member(&entry, timestamp),
                member(&described, timestamp),
                "case {case}: the two readers must agree on {timestamp}, got {entry} then \
                 {described}"
            );
        }
    }
}

/// Changing the bus no call created is what `LastModifiedTime` reports from then
/// on — and the change still invents no `CreationTime`. The rows differ along
/// the operation that performs the change.
#[tokio::test]
async fn a_change_to_the_uncreated_bus_reports_when_it_changed_and_no_creation_time() {
    let changes: Vec<(&str, &str, Value)> = vec![
        (
            "UpdateEventBus writing a description",
            "UpdateEventBus",
            json!({ "Name": DEFAULT_BUS, "Description": "the account's own bus" }),
        ),
        (
            "UpdateEventBus writing a log configuration",
            "UpdateEventBus",
            json!({
                "Name": DEFAULT_BUS,
                "LogConfig": { "IncludeDetail": "NONE", "Level": "INFO" },
            }),
        ),
        (
            "PutPermission installing a statement",
            "PutPermission",
            json!({
                "EventBusName": DEFAULT_BUS,
                "StatementId": "Shared",
                "Action": "events:PutEvents",
                "Principal": "111122223333",
            }),
        ),
    ];

    for (case, operation, request) in changes {
        // A fresh store per row, so each row observes only its own change.
        let emulator = emulator();
        let (status, body) = call(&emulator, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: {operation} answered {status} {body}"
        );

        let (status, described) =
            call(&emulator, "DescribeEventBus", &json!({ "Name": DEFAULT_BUS })).await;
        assert_eq!(
            status, 200,
            "case {case}: DescribeEventBus answered {status} {described}"
        );
        assert_eq!(
            member(&described, "CreationTime"),
            Value::Null,
            "case {case}: a change is not a create, so the bus must still report no \
             CreationTime, got {described}"
        );
        let modified = instant(case, &described, "LastModifiedTime");
        assert!(
            modified > 0.0,
            "case {case}: expected the change's instant as LastModifiedTime, got {modified}"
        );

        let listed = list_buses(&emulator, case, &json!({})).await;
        let entry = entry_for(case, &listed, DEFAULT_BUS);
        for timestamp in TIMESTAMPS {
            assert_eq!(
                member(&entry, timestamp),
                member(&described, timestamp),
                "case {case}: the two readers must agree on {timestamp}, got {entry} then \
                 {described}"
            );
        }
    }
}
