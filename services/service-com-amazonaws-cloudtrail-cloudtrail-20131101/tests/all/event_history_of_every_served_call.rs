// SPDX-License-Identifier: CC0-1.0

//! Event history of EVERY served call: that a read is recorded, that an unfiltered
//! window reports what was recorded, and that a record names the resource its call
//! addressed.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted is what a client sees end to end. Nothing is
//! seeded into the log: every record under test was made by a call this suite drove,
//! which is what makes the recorder and the reader both exercised here.
//!
//! The rules under test:
//!
//! * every management call an account's APIs SERVE is written to event history —
//!   read-only calls as much as writes, refusals as much as successes — and
//!   `LookupEvents` reports it, a read with `readOnly` true and no `responseElements`
//!   (the model's own `LookupAttributeKey` carries `ReadOnly`, which is the statement
//!   that read-only calls are recorded);
//! * a lookup carrying ONLY a window and `MaxResults` reports the account's management
//!   events for that window, newest first, at most `MaxResults` of them, each carrying
//!   `EventId` / `EventName` / `EventSource` / `CloudTrailEvent`;
//! * a record names the resource its call addressed — `Resource$ResourceName` is "the
//!   name of the resource referenced by the event … user-created names" — so a lookup by
//!   `ResourceName` or `ResourceType` selects the calls that touched that resource,
//!   whether the request addressed it by its bare name or by its ARN.
//!
//! How a window's edges are compared, how a token pages and how each other lookup
//! attribute filters are pinned by `tests/all/management_event_lookup.rs`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::emulator::{Emulator, Reply};

/// This service's `eventSource`, as CloudTrail stamps it on every event of it.
const EVENT_SOURCE: &str = "cloudtrail.amazonaws.com";

/// The retention window `LookupEvents` serves ("You can look up events that occurred in
/// a Region within the last 90 days").
const RETENTION_DAYS: u64 = 90;

/// What these cases read off a reply.
impl Reply {
    /// The reported events, in the order they were reported. (Named for this suite: an
    /// inherent method on the shared `Reply` cannot share a name with another suite's.)
    fn reported_events(&self, case: &str) -> Vec<Value> {
        self.expect_ok(case)
            .get("Events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: the reply carries no Events list: {self:?}"))
    }
}

/// A string member of a JSON object, or `""`.
fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `CloudTrailEvent` of a reported event, parsed — "a JSON string that contains a
/// representation of the event returned".
fn recorded_document(event: &Value, case: &str) -> Value {
    let raw = text(event, "CloudTrailEvent");
    serde_json::from_str(&raw).unwrap_or_else(|error| {
        panic!("case {case}: CloudTrailEvent must be a JSON string, got {raw:?} ({error})")
    })
}

/// `now` as the epoch seconds the wire carries a `Date` as.
fn epoch_seconds(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .expect("the instant is after the epoch")
            .as_secs(),
    )
    .expect("the instant fits an i64")
}

/// The events a lookup filtered on one attribute reports.
async fn lookup(emulator: &Emulator, key: &str, value: &str) -> Vec<Value> {
    let body = json!({
        "LookupAttributes": [{"AttributeKey": key, "AttributeValue": value}],
        "MaxResults": 50,
    });
    emulator
        .lookup_events(&body)
        .await
        .reported_events(&format!("{key}={value}"))
}

/// The lookups these cases drive.
impl Emulator {
    async fn lookup_events(&self, body: &Value) -> Reply {
        self.call("LookupEvents", body).await
    }
}

/// One row of the recording table: a call to drive, and what its record must say.
struct ServedCase {
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// Whether the call is SERVED (as against refused with a 4xx).
    served: bool,
    /// Whether the operation only REPORTS state.
    read_only: bool,
    /// The wire error code the record must carry; `None` for a served call.
    error_code: Option<&'static str>,
}

/// RULE: every call an account's APIs serve is in event history, read-only calls
/// included, and a read is recorded as a read.
///
/// The rows span what the rule generalises over: the call's KIND (a read and a write), the
/// read's SHAPE (one that names a resource, one that names a list, one that names nothing
/// at all), and its OUTCOME (served and refused) — because the recording is a property of
/// serving the call and not of any one operation.
#[tokio::test]
async fn every_served_call_is_recorded_whatever_its_kind_shape_and_outcome() {
    let emulator = Emulator::new();
    let trail = "audited-reads";
    let trail_arn = text(
        emulator
            .call(
                "CreateTrail",
                &json!({"Name": trail, "S3BucketName": "my-bucket"}),
            )
            .await
            .expect_ok("CreateTrail"),
        "TrailARN",
    );

    // The rows: a call to drive, and what its record must say.
    let row = |case, operation, request, served, read_only, error_code| ServedCase {
        case,
        operation,
        request,
        served,
        read_only,
        error_code,
    };
    let rows = vec![
        row(
            "a write",
            "CreateTrail",
            json!({"Name": "audited-second", "S3BucketName": "my-bucket"}),
            true,
            false,
            None,
        ),
        row(
            "a read naming one resource",
            "GetTrail",
            json!({"Name": trail}),
            true,
            true,
            None,
        ),
        row(
            "a read naming a list of resources",
            "DescribeTrails",
            json!({"trailNameList": [trail]}),
            true,
            true,
            None,
        ),
        row(
            "a read naming no resource at all",
            "ListTrails",
            json!({}),
            true,
            true,
            None,
        ),
        row(
            "a read addressing its resource by ARN",
            "ListTags",
            json!({"ResourceIdList": [trail_arn.clone()]}),
            true,
            true,
            None,
        ),
        row(
            "a REFUSED read",
            "GetTrailStatus",
            json!({"Name": "no-such-trail"}),
            false,
            true,
            Some("TrailNotFoundException"),
        ),
    ];

    for ServedCase {
        case,
        operation,
        request,
        served,
        read_only,
        error_code,
    } in rows
    {
        let reply = emulator.call(operation, &request).await;
        if served {
            reply.expect_ok(case);
        } else {
            assert!(
                (400..500).contains(&reply.status),
                "case {case:?}: expected a 4xx refusal, got {reply:?}"
            );
        }

        let found = lookup(&emulator, "EventName", operation).await;
        assert!(
            !found.is_empty(),
            "case {case:?}: a served call must be recorded under its own name, got \
             {found:?}"
        );
        // "The most recent event is listed first", so the call just made is the first row.
        let recorded = &found[0];
        assert_eq!(
            (
                text(recorded, "EventName"),
                text(recorded, "EventSource"),
                text(recorded, "ReadOnly"),
            ),
            (
                operation.to_string(),
                EVENT_SOURCE.to_string(),
                read_only.to_string(),
            ),
            "case {case:?}: expected the operation's own event, got {recorded:?}"
        );
        let document = recorded_document(recorded, case);
        assert_eq!(
            document.get("errorCode").and_then(Value::as_str),
            error_code,
            "case {case:?}: expected errorCode {error_code:?}, got {document}"
        );
        // A read reports state and answers with nothing a record carries, so CloudTrail
        // records it with a null `responseElements`.
        if read_only {
            assert_eq!(
                document.get("responseElements"),
                Some(&Value::Null),
                "case {case:?}: a read carries no response elements, got {document}"
            );
        }
        assert_eq!(
            document.get("readOnly"),
            Some(&json!(read_only)),
            "case {case:?}: the envelope and the document must agree on readOnly, got \
             {document}"
        );
    }
}

/// RULE: a lookup carrying only a window and a page size reports the account's management
/// events for that window — newest first, at most `MaxResults` of them, each carrying the
/// members the modelled `Event` declares.
///
/// The rows differ along the WINDOW's width (an hour, a day, a week, the whole retention
/// window, and one entirely in the future) and along `MaxResults` (each published bound of
/// the modelled `@range`, one inside it, and the member omitted).
#[tokio::test]
async fn an_unfiltered_lookup_over_a_window_reports_the_recorded_events() {
    let emulator = Emulator::new();
    // Eight served calls, of both kinds, so a page of five has a remainder.
    for index in 0..4 {
        let name = format!("window-trail-{index}");
        emulator
            .call(
                "CreateTrail",
                &json!({"Name": name, "S3BucketName": "my-bucket"}),
            )
            .await
            .expect_ok(&name);
        emulator
            .call("GetTrail", &json!({"Name": name}))
            .await
            .expect_ok(&name);
    }

    let now = SystemTime::now();
    // (case, how far back the window starts, whether events are expected in it)
    let windows: [(&str, Duration, bool); 4] = [
        ("the last hour", Duration::from_secs(3_600), true),
        ("the last day", Duration::from_secs(24 * 3_600), true),
        ("the last week", Duration::from_secs(7 * 24 * 3_600), true),
        (
            "all but the last day of the retention window",
            Duration::from_secs((RETENTION_DAYS - 1) * 24 * 3_600),
            true,
        ),
    ];
    // `MaxResults` is modelled `@range 1..50`, and "the default is 50".
    let page_sizes: [Option<i64>; 4] = [Some(1), Some(5), Some(50), None];

    for (window, back, expected) in windows {
        for page_size in page_sizes {
            let case = format!("{window} at MaxResults {page_size:?}");
            let start = epoch_seconds(now - back);
            let end = epoch_seconds(now + Duration::from_secs(60));
            let mut body = json!({"StartTime": start, "EndTime": end});
            if let Some(page_size) = page_size {
                body["MaxResults"] = json!(page_size);
            }
            let events = emulator.lookup_events(&body).await.reported_events(&case);
            assert_eq!(
                !events.is_empty(),
                expected,
                "case {case:?}: expected a {} page, got {} events",
                if expected { "non-empty" } else { "empty" },
                events.len()
            );
            let limit = page_size.unwrap_or(50);
            assert!(
                i64::try_from(events.len()).expect("a page length fits an i64") <= limit,
                "case {case:?}: a page holds at most {limit} events, got {}",
                events.len()
            );

            let mut previous = f64::MAX;
            for event in &events {
                for member in ["EventId", "EventName", "EventSource", "CloudTrailEvent"] {
                    assert!(
                        !text(event, member).is_empty(),
                        "case {case:?}: every event carries {member}, got {event:?}"
                    );
                }
                let at = event
                    .get("EventTime")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| panic!("case {case:?}: an event carries no EventTime"));
                assert!(
                    at <= previous,
                    "case {case:?}: the most recent event is listed first, got {at} after \
                     {previous}"
                );
                assert!(
                    at >= start as f64 && at <= end as f64,
                    "case {case:?}: every reported event lies inside the window \
                     [{start}, {end}], got {at}"
                );
                previous = at;
                // "A JSON string that contains a representation of the event returned."
                let document = recorded_document(event, &case);
                assert_eq!(
                    document.get("eventName").and_then(Value::as_str),
                    event.get("EventName").and_then(Value::as_str),
                    "case {case:?}: the envelope and the document must name one \
                     operation, got {document}"
                );
            }
        }
    }

    // A window entirely in the future holds nothing: no recorded event lies ahead of the
    // call that reports it.
    let ahead = json!({
        "StartTime": epoch_seconds(now + Duration::from_secs(3_600)),
        "EndTime": epoch_seconds(now + Duration::from_secs(7_200)),
    });
    let events = emulator
        .lookup_events(&ahead)
        .await
        .reported_events("a future window");
    assert!(
        events.is_empty(),
        "case a future window: expected no events, got {events:?}"
    );
}

/// RULE: a record states the instant it was served and the principal that made the call
/// the way a client reads them — `eventTime` in the whole-second ISO 8601 form CloudTrail
/// publishes for the member, and `userIdentity` naming the IAM entity the credential
/// speaks for (its ARN and unique id), which is the identity STS answers
/// `GetCallerIdentity` with for the same credential.
///
/// The rows are a read and a write, because neither field depends on the operation.
#[tokio::test]
async fn a_record_states_its_instant_and_its_caller_the_way_a_client_reads_them() {
    let emulator = Emulator::new();
    let trail = "recorded-identity";
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": trail, "S3BucketName": "my-bucket"}),
        )
        .await
        .expect_ok("CreateTrail");
    emulator
        .call("GetTrail", &json!({"Name": trail}))
        .await
        .expect_ok("GetTrail");

    for operation in ["CreateTrail", "GetTrail"] {
        let found = lookup(&emulator, "EventName", operation).await;
        let document = recorded_document(&found[0], operation);

        // `yyyy-MM-dd'T'HH:mm:ss'Z'`: whole seconds, no fractional part — the form a
        // consumer parses the member with.
        let at = document
            .get("eventTime")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {operation}: the record carries no eventTime"));
        assert_eq!(
            (at.len(), at.contains('.'), at.ends_with('Z')),
            (20, false, true),
            "case {operation}: expected a whole-second ISO 8601 instant, got {at:?}"
        );

        // The principal the emulator's long-term credential speaks for: one IAM user,
        // named identically wherever it is reported.
        for (field, expected) in [
            ("type", json!("IAMUser")),
            ("userName", json!("emulator")),
            (
                "arn",
                json!(service_common::caller_principal::ambient_user_arn()),
            ),
            (
                "principalId",
                json!(service_common::caller_principal::ambient_user_id()),
            ),
        ] {
            assert_eq!(
                document.pointer(&format!("/userIdentity/{field}")),
                Some(&expected),
                "case {operation}: expected userIdentity.{field} {expected}, got {document}"
            );
        }
    }
}

/// RULE: a record names the resource its call addressed, so a lookup by `ResourceName` or
/// `ResourceType` selects the calls that touched that resource — and an identifier is
/// folded to the resource's NAME whether the request wrote the bare name or the ARN.
///
/// The rows span the resource KINDS this service's calls address and the two FORMS an
/// identifier takes, with one row per operation that addresses the resource a different
/// way.
#[tokio::test]
async fn a_lookup_by_resource_finds_every_call_that_addressed_it() {
    let emulator = Emulator::new();
    let trail = "resource-lookup";
    let trail_arn = text(
        emulator
            .call(
                "CreateTrail",
                &json!({"Name": trail, "S3BucketName": "my-bucket"}),
            )
            .await
            .expect_ok("CreateTrail"),
        "TrailARN",
    );
    // Addressed by its bare NAME …
    emulator
        .call("StartLogging", &json!({"Name": trail}))
        .await
        .expect_ok("StartLogging");
    emulator
        .call(
            "PutEventSelectors",
            &json!({
                "TrailName": trail,
                "EventSelectors": [{"ReadWriteType": "All", "IncludeManagementEvents": true}],
            }),
        )
        .await
        .expect_ok("PutEventSelectors");
    // … and by its ARN, which names the same resource.
    emulator
        .call("GetEventSelectors", &json!({"TrailName": trail_arn.clone()}))
        .await
        .expect_ok("GetEventSelectors");
    emulator
        .call(
            "AddTags",
            &json!({"ResourceId": trail_arn.clone(), "TagsList": [{"Key": "team", "Value": "audit"}]}),
        )
        .await
        .expect_ok("AddTags");

    // A second resource KIND, created by name and read back by ARN: the store's ARN
    // carries the id the service minted, so that is the name the read's record reports.
    let store = "resource-lookup-store";
    let store_arn = text(
        emulator
            .call("CreateEventDataStore", &json!({"Name": store}))
            .await
            .expect_ok("CreateEventDataStore"),
        "EventDataStoreArn",
    );
    let store_id = store_arn
        .rsplit('/')
        .next()
        .expect("an event data store ARN carries its id")
        .to_string();
    emulator
        .call(
            "GetEventDataStore",
            &json!({"EventDataStore": store_arn.clone()}),
        )
        .await
        .expect_ok("GetEventDataStore");

    // (case, the attribute, the value, the operations whose records must name it)
    let rows: Vec<(&str, &str, String, Vec<&str>)> = vec![
        (
            "a trail by the name every form of its identifier folds to",
            "ResourceName",
            trail.to_string(),
            vec![
                "CreateTrail",
                "StartLogging",
                "PutEventSelectors",
                "GetEventSelectors",
                "AddTags",
            ],
        ),
        (
            "every trail-typed record",
            "ResourceType",
            "AWS::CloudTrail::Trail".to_string(),
            vec!["CreateTrail", "StartLogging", "PutEventSelectors"],
        ),
        (
            "an event data store by the name it was created under",
            "ResourceName",
            store.to_string(),
            vec!["CreateEventDataStore"],
        ),
        (
            "an event data store by the id its ARN carries",
            "ResourceName",
            store_id.clone(),
            vec!["GetEventDataStore"],
        ),
        (
            "every event-data-store-typed record",
            "ResourceType",
            "AWS::CloudTrail::EventDataStore".to_string(),
            vec!["CreateEventDataStore", "GetEventDataStore"],
        ),
    ];

    for (case, key, value, expected) in rows {
        let found = lookup(&emulator, key, &value).await;
        let names: Vec<String> = found
            .iter()
            .map(|event| text(event, "EventName"))
            .collect();
        for operation in &expected {
            assert!(
                names.contains(&(*operation).to_string()),
                "case {case:?}: a lookup for {key}={value} must report {operation}, got \
                 {names:?}"
            );
        }
        // Each reported envelope agrees with its own document, and names the resource the
        // lookup asked for.
        for event in &found {
            let document = recorded_document(event, case);
            assert_eq!(
                document.get("eventName").and_then(Value::as_str),
                event.get("EventName").and_then(Value::as_str),
                "case {case:?}: the envelope's EventName must equal the document's \
                 eventName, got {document}"
            );
            let resources = event
                .get("Resources")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert!(
                resources.iter().any(|resource| text(resource, key) == value),
                "case {case:?}: every reported record names {key} {value}, got \
                 {resources:?}"
            );
        }
    }

    // An accepted attribute that names no resource answers an empty list — not a refusal:
    // `InvalidLookupAttributesException` is for an attribute the service will not accept.
    let reply = emulator
        .lookup_events(&json!({
            "LookupAttributes": [
                {"AttributeKey": "ResourceName", "AttributeValue": "no-such-resource"},
            ],
        }))
        .await;
    let events = reply.reported_events("a resource nothing addressed");
    assert!(
        events.is_empty(),
        "case a resource nothing addressed: expected an empty list, got {events:?}"
    );
}
