// SPDX-License-Identifier: CC0-1.0

//! The archive `ArchiveState` every reader reports, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through
//! the real `MultiServiceRouter`.
//!
//! The RULE asserted here: an archive is provisioned by the call that writes it,
//! so `CreateArchive` and `UpdateArchive` answer the archive's SETTLED
//! `ArchiveState` and every reader of that archive — the write's own response,
//! `DescribeArchive` and the `ListArchives` summary — reports the one state
//! stored for it; no non-terminal `ArchiveState` is ever observable, so a client
//! polling for a terminal one is already done when the write returns, and the
//! archive captures what its bus routes from the create on.
//!
//! The table spans the class rather than one case: the rows differ along the bus
//! the archive is bound to (the default bus and a custom one), whether it filters
//! at all and how narrowly, its `RetentionDays` (its published lower bound, one
//! past it, and a year) and whether it is encrypted under a customer key, and
//! along which members the update that follows carries.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// Every value of `ArchiveState` that is NOT the settled one an archive of this
/// emulator reaches, from
/// `smithy_query(query="shape", shape="ArchiveState")` — `CREATE_FAILED`,
/// `CREATING`, `DISABLED`, `ENABLED`, `UPDATE_FAILED`, `UPDATING` — less
/// `ENABLED`.
const UNSETTLED_ARCHIVE_STATES: [&str; 5] = [
    "CREATE_FAILED",
    "CREATING",
    "DISABLED",
    "UPDATE_FAILED",
    "UPDATING",
];

/// One `awsJson1_1` call to EventBridge over the router.
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

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The ARN of the bus `name` on this account and region, as `EventSourceArn`
/// names it.
fn bus_arn(name: &str) -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// A custom event bus called `name`.
async fn given_bus(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = events(router, "CreateEventBus", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEventBus answered {status} {body}"
    );
}

/// What `DescribeArchive` reports for `name`.
async fn describe_archive(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = events(router, "DescribeArchive", &json!({ "ArchiveName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeArchive answered {status} {body}"
    );
    body
}

/// The `ListArchives` summary of `name`, which the listing has to report.
async fn listed_archive(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = events(router, "ListArchives", &json!({ "NamePrefix": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListArchives answered {status} {body}"
    );
    body["Archives"]
        .as_array()
        .and_then(|archives| archives.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the listing reports no {name:?}: {body}"))
}

/// The `ArchiveName`s a `ListArchives` body reports.
fn archive_names(body: &Value) -> Vec<String> {
    body["Archives"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|archive| {
            archive["ArchiveName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert that every reader of `name` reports the settled `ENABLED` state, and
/// that the `State` filter agrees: the settled state selects the archive and no
/// other `ArchiveState` does.
async fn assert_settled(router: &MultiServiceRouter, case: &str, name: &str, answered: &Value) {
    assert_eq!(
        answered["State"],
        json!(state::ARCHIVE_STATE_ENABLED),
        "case {case:?}: the write answers {}, got {answered}",
        state::ARCHIVE_STATE_ENABLED
    );
    // Read STRAIGHT after the write: a settled state leaves nothing to poll for.
    let described = describe_archive(router, case, name).await;
    assert_eq!(
        described["State"],
        json!(state::ARCHIVE_STATE_ENABLED),
        "case {case:?}: DescribeArchive reports {}, got {described}",
        state::ARCHIVE_STATE_ENABLED
    );
    let summary = listed_archive(router, case, name).await;
    assert_eq!(
        summary["State"],
        json!(state::ARCHIVE_STATE_ENABLED),
        "case {case:?}: the ListArchives summary reports {}, got {summary}",
        state::ARCHIVE_STATE_ENABLED
    );
    // A second read agrees with the first: reading an archive moves nothing.
    let again = describe_archive(router, case, name).await;
    assert_eq!(
        again["State"], described["State"],
        "case {case:?}: the state must not change because the archive was read, got {again}"
    );

    let (status, selected) = events(
        router,
        "ListArchives",
        &json!({ "State": state::ARCHIVE_STATE_ENABLED }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListArchives answered {status} {selected}"
    );
    assert!(
        archive_names(&selected).contains(&name.to_string()),
        "case {case:?}: the {} filter selects the archive, got {selected}",
        state::ARCHIVE_STATE_ENABLED
    );
    for unsettled in UNSETTLED_ARCHIVE_STATES {
        let (status, body) = events(router, "ListArchives", &json!({ "State": unsettled })).await;
        assert_eq!(
            status, 200,
            "case {case:?}/{unsettled}: ListArchives answered {status} {body}"
        );
        assert!(
            !archive_names(&body).contains(&name.to_string()),
            "case {case:?}: no archive is in {unsettled}, got {body}"
        );
    }
}

/// One row of the table: how the archive is created and how it is then updated.
struct ArchiveCase {
    case: &'static str,
    /// The custom bus to create first, or `None` for the account's default bus.
    bus: Option<&'static str>,
    /// Extra members of the `CreateArchive` request.
    created_with: Value,
    /// Members of the `UpdateArchive` request that follows.
    updated_with: Value,
}

/// Every reader of an archive reports the settled `ENABLED` state, whatever the
/// archive was created with and whatever an update changes.
#[tokio::test]
async fn every_archive_reader_reports_the_settled_state() {
    // `RetentionDays` is modelled `range 0..` with "default value is 0. If set to
    // 0, events are retained indefinitely", so the rows carry its published lower
    // bound, one past it, and a year.
    let rows = [
        ArchiveCase {
            case: "the default bus, no filter and no options, then a description",
            bus: None,
            created_with: json!({}),
            updated_with: json!({ "Description": "the archive of everything" }),
        },
        ArchiveCase {
            case: "a custom bus with a filter and the retention bound of 0",
            bus: Some("settled-bus"),
            created_with: json!({
                "Description": "as created",
                "EventPattern": json!({ "source": ["com.demo.orders"] }).to_string(),
                "RetentionDays": 0,
            }),
            updated_with: json!({
                "EventPattern": json!({ "source": ["com.demo.invoices"] }).to_string(),
            }),
        },
        ArchiveCase {
            case: "a custom bus, one day of retention, then a wider retention",
            bus: Some("kept-bus"),
            created_with: json!({ "RetentionDays": 1 }),
            updated_with: json!({ "RetentionDays": 365 }),
        },
        ArchiveCase {
            case: "a year of retention and a customer managed key, then every member at once",
            bus: Some("encrypted-bus"),
            created_with: json!({ "RetentionDays": 365, "KmsKeyIdentifier": "archive-key" }),
            updated_with: json!({
                "Description": "everything",
                "EventPattern": json!({ "detail-type": ["Thing Happened"] }).to_string(),
                "RetentionDays": 0,
                "KmsKeyIdentifier": "another-key",
            }),
        },
        ArchiveCase {
            case: "a narrow filter cleared by an update, which is the absence of a filter",
            bus: Some("cleared-bus"),
            created_with: json!({
                "EventPattern": json!({ "source": ["com.demo.orders"] }).to_string(),
            }),
            updated_with: json!({ "EventPattern": "" }),
        },
    ];

    for ArchiveCase {
        case,
        bus,
        created_with,
        updated_with,
    } in rows
    {
        let router = emulator();
        let bus_name = bus.unwrap_or(state::DEFAULT_EVENT_BUS_NAME);
        if let Some(bus) = bus {
            given_bus(&router, case, bus).await;
        }
        let name = "settled-archive";

        let mut request = json!({ "ArchiveName": name, "EventSourceArn": bus_arn(bus_name) });
        for (member, value) in created_with.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        let (status, created) = events(&router, "CreateArchive", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateArchive answered {status} {created}"
        );
        assert_settled(&router, case, name, &created).await;

        let mut request = json!({ "ArchiveName": name });
        for (member, value) in updated_with.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        let (status, updated) = events(&router, "UpdateArchive", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateArchive answered {status} {updated}"
        );
        assert_settled(&router, case, name, &updated).await;
    }
}

/// The settled state is the archive's real one, not a label: an archive captures
/// the events of its bus from the create on, and by the pattern an update wrote
/// from that update on.
#[tokio::test]
async fn a_settled_archive_captures_from_the_write_on() {
    // (case, the pattern the archive is created with, the source published, how
    //  many events it holds afterwards)
    let rows: [(&str, Option<&str>, &str, i64); 3] = [
        ("an archive that filters nothing", None, "com.demo.orders", 1),
        (
            "an archive whose filter matches what is published",
            Some(r#"{"source":["com.demo.orders"]}"#),
            "com.demo.orders",
            1,
        ),
        (
            "an archive whose filter does not match it",
            Some(r#"{"source":["com.demo.orders"]}"#),
            "com.demo.shipments",
            0,
        ),
    ];

    for (case, pattern, source, expected) in rows {
        let router = emulator();
        given_bus(&router, case, "capturing-bus").await;
        let name = "capturing-archive";
        let mut request =
            json!({ "ArchiveName": name, "EventSourceArn": bus_arn("capturing-bus") });
        if let Some(pattern) = pattern {
            request["EventPattern"] = json!(pattern);
        }
        let (status, created) = events(&router, "CreateArchive", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateArchive answered {status} {created}"
        );

        // Published with no wait at all after the create.
        publish(&router, case, "capturing-bus", source).await;
        let described = describe_archive(&router, case, name).await;
        assert_eq!(
            described["EventCount"],
            json!(expected),
            "case {case:?}: expected {expected} captured events, got {described}"
        );
        assert_eq!(
            described["SizeBytes"].as_i64().unwrap_or_default() > 0,
            expected > 0,
            "case {case:?}: SizeBytes is read off what the archive holds, got {described}"
        );

        // And by the pattern an update wrote, from that update on.
        let (status, updated) = events(
            &router,
            "UpdateArchive",
            &json!({
                "ArchiveName": name,
                "EventPattern": json!({ "source": [source] }).to_string(),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateArchive answered {status} {updated}"
        );
        publish(&router, case, "capturing-bus", source).await;
        let described = describe_archive(&router, case, name).await;
        assert_eq!(
            described["EventCount"],
            json!(expected + 1),
            "case {case:?}: the update's pattern governs what is captured next, got {described}"
        );
    }
}

/// Publish one event with `source` onto `bus`.
async fn publish(router: &MultiServiceRouter, case: &str, bus: &str, source: &str) {
    let (status, body) = events(
        router,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": bus,
                "Source": source,
                "DetailType": "Thing Happened",
                "Detail": r#"{"size":1}"#,
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutEvents reported a failed entry: {body}"
    );
}
