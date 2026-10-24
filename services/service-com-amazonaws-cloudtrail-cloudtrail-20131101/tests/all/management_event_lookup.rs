// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CloudTrail management-event log and `LookupEvents`.
//!
//! Every case goes over the wire: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: CloudTrail_20131101.<Operation>` through the same
//! `MultiServiceRouter` the production router serves, so what is asserted here is what a
//! client sees — including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * a call that CHANGES state is recorded in the account's management-event log —
//!   successes and refusals alike, the refusal carrying the error code it answered with
//!   — and `LookupEvents` reports the record;
//! * a REPORTING call records nothing, so no read changes what a later lookup sees;
//! * "The events list is sorted by time. The most recent event is listed first.";
//! * `StartTime` / `EndTime` bound the window INCLUSIVELY ("after or at" / "before or
//!   at"), only the last 90 days are retrievable, and `StartTime` after `EndTime` is an
//!   `InvalidTimeRangeException`;
//! * a lookup filters on at most ONE `LookupAttribute` ("Currently the list can contain
//!   only one item"), whose key is one of the eight the model's `LookupAttributeKey`
//!   declares, and each key filters on the event field it names;
//! * `EventCategory` takes only the value the model declares (`insight`), and an absent
//!   category asks for management events;
//! * `MaxResults` bounds the page (1 through 50, default 50), the token is opaque and
//!   STABLE — the same token answers the same page — and a token from a different lookup
//!   is refused with `InvalidNextTokenException`;
//! * events are scoped to the Region they were recorded in.
//!
//! Cases that need events at KNOWN instants write them straight into the shared log
//! ([`service_common::management_events`]) at whole-second times, which is the same store
//! a served call writes; the cases above them drive real calls, so the recorder and the
//! reader are both exercised end to end.
//!
//! The emulator fixture is the crate's shared one (`crate::emulator`); this file adds
//! only the helpers this suite needs.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_common::management_events::{
    AuditedResource, ServedApiCall, PRINCIPAL_TYPE_IAM_USER,
};

use crate::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases that check a lookup is Region-scoped.
const OTHER_REGION: &str = "eu-west-1";

/// The access key id the shared fixture signs every request with.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// This service's `eventSource`.
const EVENT_SOURCE: &str = "cloudtrail.amazonaws.com";

/// The retention window `LookupEvents` serves ("within the last 90 days").
const RETENTION: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// What this suite reads off an answered lookup.
impl Reply {
    /// The reported events, in the order they were reported.
    fn events(&self, case: &str) -> Vec<Value> {
        self.expect_ok(case)
            .get("Events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: the reply carries no Events list: {self:?}"))
    }

    /// The reported events' names, in the order they were reported.
    fn event_names(&self, case: &str) -> Vec<String> {
        self.events(case)
            .iter()
            .map(|event| text(event, "EventName"))
            .collect()
    }

    /// The reply's `NextToken`, if it carried one.
    fn next_token(&self, case: &str) -> Option<String> {
        self.expect_ok(case)
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string)
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

/// The operations this suite drives on the shared fixture.
impl Emulator {
    /// `LookupEvents` with `body`, from [`HOME_REGION`].
    async fn lookup(&self, body: &Value) -> Reply {
        self.call("LookupEvents", body).await
    }

    /// Write `call` into the shared log as served at `at`, and answer its event id.
    fn seed(&self, call: ServedApiCall, at: SystemTime) -> String {
        self.management_event_log().lock().record(call, at)
    }

    /// How many records the log holds.
    fn recorded(&self) -> usize {
        self.management_event_log().lock().len()
    }
}

/// A served call as a recorder reports one, with everything a lookup can filter on left
/// to the caller to vary.
fn served(event_name: &str, region: &str) -> ServedApiCall {
    ServedApiCall {
        event_name: event_name.to_string(),
        event_source: EVENT_SOURCE.to_string(),
        aws_region: region.to_string(),
        read_only: false,
        access_key_id: Some(ACCESS_KEY_ID.to_string()),
        user_name: Some("emulator".to_string()),
        principal_type: PRINCIPAL_TYPE_IAM_USER.to_string(),
        account_id: engine::emulator_account_id().to_string(),
        user_agent: None,
        resources: vec![AuditedResource::new(
            "AWS::CloudTrail::Trail",
            format!("arn:aws:cloudtrail:{region}:000000000000:trail/{event_name}-trail"),
        )],
        request_parameters: json!({"name": format!("{event_name}-trail")}),
        response_elements: None,
        error_code: None,
        error_message: None,
    }
}

/// A whole-second instant an hour ago, so a seeded event's time survives the wire
/// (epoch-seconds) exactly and every window bound below is compared without rounding.
fn base_instant() -> SystemTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch");
    UNIX_EPOCH + Duration::from_secs(now.as_secs() - 3_600)
}

/// `at` as the epoch seconds the wire carries a `Date` as.
fn epoch_seconds(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .expect("the instant is after the epoch")
            .as_secs(),
    )
    .expect("the instant fits an i64")
}

/// The `CloudTrailEvent` of a reported event, parsed — it is documented as "A JSON string
/// that contains a representation of the event returned".
fn cloud_trail_event(event: &Value, case: &str) -> Value {
    let raw = text(event, "CloudTrailEvent");
    serde_json::from_str(&raw).unwrap_or_else(|error| {
        panic!("case {case}: CloudTrailEvent must be a JSON string, got {raw:?} ({error})")
    })
}

/// A served write is recorded and reported, with every member of the modelled `Event`
/// shape the emulator can know — and `CloudTrailEvent` is a JSON document carrying the
/// call's own parameters and response.
#[tokio::test]
async fn a_served_write_is_recorded_and_reported() {
    let emulator = Emulator::new();
    let created = emulator
        .call(
            "CreateTrail",
            &json!({"Name": "audited-trail", "S3BucketName": "my-bucket"}),
        )
        .await;
    let trail_arn = text(created.expect_ok("CreateTrail"), "TrailARN");

    let reply = emulator.lookup(&json!({})).await;
    let events = reply.events("LookupEvents after CreateTrail");
    assert_eq!(
        events.len(),
        1,
        "case one write: expected exactly one recorded event, got {events:?}"
    );
    let event = &events[0];
    for (member, expected) in [
        ("EventName", "CreateTrail".to_string()),
        ("EventSource", EVENT_SOURCE.to_string()),
        ("ReadOnly", "false".to_string()),
        ("AccessKeyId", ACCESS_KEY_ID.to_string()),
        ("Username", "emulator".to_string()),
    ] {
        assert_eq!(
            text(event, member),
            expected,
            "case {member}: expected {expected:?}, got {event:?}"
        );
    }
    assert!(
        event
            .get("EventTime")
            .and_then(Value::as_f64)
            .is_some_and(|seconds| seconds > 0.0),
        "case EventTime: expected the instant the call was served, got {event:?}"
    );
    assert_eq!(
        event.get("Resources"),
        Some(&json!([{"ResourceType": "AWS::CloudTrail::Trail", "ResourceName": trail_arn}])),
        "case Resources: expected the created trail, got {event:?}"
    );

    let document = cloud_trail_event(event, "CreateTrail");
    for (field, expected) in [
        ("eventName", json!("CreateTrail")),
        ("eventSource", json!(EVENT_SOURCE)),
        ("awsRegion", json!(HOME_REGION)),
        ("readOnly", json!(false)),
        ("eventType", json!("AwsApiCall")),
        ("managementEvent", json!(true)),
        ("eventCategory", json!("Management")),
    ] {
        assert_eq!(
            document.get(field),
            Some(&expected),
            "case {field}: expected {expected}, got {document}"
        );
    }
    assert_eq!(
        document.pointer("/requestParameters/s3BucketName"),
        Some(&json!("my-bucket")),
        "case requestParameters: expected the call's own parameters, got {document}"
    );
    assert_eq!(
        document.pointer("/responseElements/trailARN"),
        Some(&json!(trail_arn)),
        "case responseElements: expected the call's own response, got {document}"
    );
    assert_eq!(
        document.get("eventID"),
        Some(&json!(text(event, "EventId"))),
        "case eventID: the document must carry the reported event id, got {document}"
    );
}

/// A write that was REFUSED is recorded too, carrying the error code the caller was
/// answered with and no response elements.
#[tokio::test]
async fn a_refused_write_is_recorded_with_its_error_code() {
    let emulator = Emulator::new();
    let request = json!({"Name": "twice-trail", "S3BucketName": "my-bucket"});
    emulator.call("CreateTrail", &request).await.expect_ok("first CreateTrail");
    emulator
        .call("CreateTrail", &request)
        .await
        .expect_error("second CreateTrail", 400, "TrailAlreadyExistsException");

    let reply = emulator.lookup(&json!({})).await;
    let events = reply.events("LookupEvents after a refusal");
    assert_eq!(
        events.len(),
        2,
        "case refused write: both calls must be recorded, got {events:?}"
    );
    // The most recent event is listed first, and that is the refusal.
    let refused = cloud_trail_event(&events[0], "refused CreateTrail");
    assert_eq!(
        refused.get("errorCode"),
        Some(&json!("TrailAlreadyExistsException")),
        "case errorCode: expected the code the caller was answered with, got {refused}"
    );
    assert_eq!(
        refused.get("responseElements"),
        Some(&Value::Null),
        "case responseElements: a refused call answered with nothing, got {refused}"
    );
    let served = cloud_trail_event(&events[1], "served CreateTrail");
    assert_eq!(
        served.get("errorCode"),
        None,
        "case errorCode: a call that succeeded carries none, got {served}"
    );
}

/// One row of the recorder table: a call to drive, the error code the caller is answered
/// with, and the trail the call addresses.
struct ServedCase {
    case: &'static str,
    operation: &'static str,
    request: Value,
    /// The wire error code a refusal is answered with; `None` for a success. The HTTP
    /// STATUS is the driven operation's own business (its model's `httpError` for the
    /// error it answers); what this suite pins is the CODE, because that is what the
    /// audit record has to carry.
    failure: Option<&'static str>,
    addressed: &'static str,
}

/// EVERY implemented operation that changes state records its call — a table over the
/// four of them, each driven both to a success and to a refusal, with the refusal
/// carrying the code the caller was answered with and every record naming the trail it
/// addressed.
#[tokio::test]
async fn every_mutating_operation_records_its_call() {
    let emulator = Emulator::new();
    let trail = "audited-lifecycle";
    let absent = "no-such-trail";
    let arn_of = |name: &str| {
        format!(
            "arn:aws:cloudtrail:{HOME_REGION}:{}:trail/{name}",
            engine::emulator_account_id()
        )
    };
    let create = json!({"Name": trail, "S3BucketName": "my-bucket"});
    let row = |case, operation, request, failure, addressed| ServedCase {
        case,
        operation,
        request,
        failure,
        addressed,
    };

    // The order walks the trail's lifecycle, so each row's outcome is the one the row
    // before it set up.
    let cases = [
        row("create", "CreateTrail", create.clone(), None, trail),
        row(
            "create the same name twice",
            "CreateTrail",
            create.clone(),
            Some("TrailAlreadyExistsException"),
            trail,
        ),
        row(
            "start logging",
            "StartLogging",
            json!({"Name": trail}),
            None,
            trail,
        ),
        row(
            "stop logging",
            "StopLogging",
            json!({"Name": trail}),
            None,
            trail,
        ),
        row("delete", "DeleteTrail", json!({"Name": trail}), None, trail),
        row(
            "start logging on a trail that is gone",
            "StartLogging",
            json!({"Name": absent}),
            Some("TrailNotFoundException"),
            absent,
        ),
        row(
            "stop logging on a trail that is gone",
            "StopLogging",
            json!({"Name": absent}),
            Some("TrailNotFoundException"),
            absent,
        ),
        row(
            "delete a trail that is gone",
            "DeleteTrail",
            json!({"Name": absent}),
            Some("TrailNotFoundException"),
            absent,
        ),
    ];

    for (index, row) in cases.iter().enumerate() {
        let ServedCase {
            case,
            operation,
            request,
            failure,
            addressed,
        } = row;
        let reply = emulator.call(operation, request).await;
        match failure {
            None => {
                reply.expect_ok(case);
            }
            Some(code) => assert!(
                (400..500).contains(&reply.status) && reply.error_code() == *code,
                "case {case:?}: expected a 4xx {code}, got {reply:?}"
            ),
        }

        let events = emulator.lookup(&json!({})).await.events(case);
        assert_eq!(
            events.len(),
            index + 1,
            "case {case:?}: every served call must add exactly one record, got {events:?}"
        );
        // The most recent event is listed first, so it is this row's call.
        let recorded = &events[0];
        assert_eq!(
            text(recorded, "EventName"),
            *operation,
            "case {case:?}: expected the operation's own event, got {recorded:?}"
        );
        assert_eq!(
            text(recorded, "ReadOnly"),
            "false",
            "case {case:?}: a call that changes state is a write event, got {recorded:?}"
        );
        assert_eq!(
            recorded.get("Resources"),
            Some(&json!([{
                "ResourceType": "AWS::CloudTrail::Trail",
                "ResourceName": arn_of(addressed),
            }])),
            "case {case:?}: expected the addressed trail, got {recorded:?}"
        );
        let document = cloud_trail_event(recorded, case);
        assert_eq!(
            document.get("errorCode").and_then(Value::as_str),
            *failure,
            "case {case:?}: expected errorCode {failure:?}, got {document}"
        );
        assert_eq!(
            document.pointer("/requestParameters/name"),
            Some(&json!(addressed)),
            "case {case:?}: expected the request's own parameters, got {document}"
        );
    }
}

/// A REPORTING call records nothing: reading the trail registry, or looking events up,
/// leaves every later lookup answering exactly what it answered before.
#[tokio::test]
async fn reporting_calls_do_not_change_what_a_later_lookup_sees() {
    let emulator = Emulator::new();
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": "read-trail", "S3BucketName": "my-bucket"}),
        )
        .await
        .expect_ok("CreateTrail");

    let before = emulator.lookup(&json!({})).await.events("first lookup");
    for (operation, body) in [
        ("GetTrail", json!({"Name": "read-trail"})),
        ("GetTrailStatus", json!({"Name": "read-trail"})),
        ("DescribeTrails", json!({})),
        ("ListTrails", json!({})),
        ("LookupEvents", json!({})),
    ] {
        emulator
            .call(operation, &body)
            .await
            .expect_ok(&format!("{operation} as a read"));
        let after = emulator.lookup(&json!({})).await.events("lookup after a read");
        assert_eq!(
            after, before,
            "case {operation}: a read must not change what a later lookup sees"
        );
    }
    assert_eq!(
        emulator.recorded(),
        1,
        "case reads: only the one write may be recorded"
    );
}

/// The reported order is by time, most recent first — whatever order the records were
/// written to the log in.
#[tokio::test]
async fn events_are_reported_most_recent_first() {
    let emulator = Emulator::new();
    let base = base_instant();
    // Written OUT of time order on purpose: the reported order is the events' times,
    // not the order the log happens to hold them in.
    for (name, offset) in [
        ("CallB", 20),
        ("CallD", 40),
        ("CallA", 10),
        ("CallE", 50),
        ("CallC", 30),
    ] {
        emulator.seed(
            served(name, HOME_REGION),
            base + Duration::from_secs(offset),
        );
    }

    let names = emulator
        .lookup(&json!({}))
        .await
        .event_names("ordering by time");
    assert_eq!(
        names,
        vec!["CallE", "CallD", "CallC", "CallB", "CallA"],
        "case ordering: expected the most recent event first, got {names:?}"
    );
}

/// `StartTime` and `EndTime` bound the window inclusively, a window in the future is
/// empty, and nothing outside the 90-day retention window is retrievable.
#[tokio::test]
async fn window_bounds_are_inclusive_and_exact() {
    let emulator = Emulator::new();
    let base = base_instant();
    let at = |offset: u64| base + Duration::from_secs(offset);
    for (name, offset) in [("CallA", 10), ("CallB", 20), ("CallC", 30)] {
        emulator.seed(served(name, HOME_REGION), at(offset));
    }
    // Recorded before the window the service serves, so a lookup never reports it. It is
    // written AFTER the three above so the log keeps it (a record only prunes what the
    // instant of the write has already made due).
    emulator.seed(
        served("CallAncient", HOME_REGION),
        SystemTime::now() - RETENTION - Duration::from_secs(60),
    );
    let future = SystemTime::now() + Duration::from_secs(3_600);

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no bounds at all", json!({}), vec!["CallC", "CallB", "CallA"]),
        (
            "both bounds on an event's own instant",
            json!({"StartTime": epoch_seconds(at(20)), "EndTime": epoch_seconds(at(20))}),
            vec!["CallB"],
        ),
        (
            "StartTime on the oldest event's instant is inclusive",
            json!({"StartTime": epoch_seconds(at(10))}),
            vec!["CallC", "CallB", "CallA"],
        ),
        (
            "StartTime one second later excludes it",
            json!({"StartTime": epoch_seconds(at(11))}),
            vec!["CallC", "CallB"],
        ),
        (
            "EndTime on the newest event's instant is inclusive",
            json!({"EndTime": epoch_seconds(at(30))}),
            vec!["CallC", "CallB", "CallA"],
        ),
        (
            "EndTime one second earlier excludes it",
            json!({"EndTime": epoch_seconds(at(29))}),
            vec!["CallB", "CallA"],
        ),
        (
            "a window between two events is empty",
            json!({"StartTime": epoch_seconds(at(21)), "EndTime": epoch_seconds(at(29))}),
            vec![],
        ),
        (
            "a window in the future is empty",
            json!({
                "StartTime": epoch_seconds(future),
                "EndTime": epoch_seconds(future + Duration::from_secs(60)),
            }),
            vec![],
        ),
        (
            "a window older than the retention window is empty",
            json!({
                "StartTime": epoch_seconds(SystemTime::now() - RETENTION - Duration::from_secs(600)),
                "EndTime": epoch_seconds(SystemTime::now() - RETENTION - Duration::from_secs(30)),
            }),
            vec![],
        ),
    ];

    for (case, body, expected) in cases {
        let names = emulator.lookup(&body).await.event_names(case);
        assert_eq!(
            names, expected,
            "case {case:?}: expected {expected:?}, got {names:?}"
        );
    }
}

/// `StartTime` after `EndTime` is refused; the two being equal is a one-instant window,
/// not an error.
#[tokio::test]
async fn a_start_after_the_end_is_refused() {
    let emulator = Emulator::new();
    let base = base_instant();
    emulator.seed(served("CallA", HOME_REGION), base);

    for (case, start, end) in [
        ("one second inverted", 1_i64, 0_i64),
        ("an hour inverted", 3_600, 0),
        ("a day inverted", 86_400, 1),
    ] {
        let body = json!({
            "StartTime": epoch_seconds(base) + start,
            "EndTime": epoch_seconds(base) + end,
        });
        emulator
            .lookup(&body)
            .await
            .expect_error(case, 400, "InvalidTimeRangeException");
    }
    let body = json!({"StartTime": epoch_seconds(base), "EndTime": epoch_seconds(base)});
    let names = emulator.lookup(&body).await.event_names("equal bounds");
    assert_eq!(
        names,
        vec!["CallA"],
        "case equal bounds: an instant-wide window is valid, got {names:?}"
    );
}

/// Every `LookupAttributeKey` the model declares filters on the event field it names.
#[tokio::test]
async fn each_lookup_attribute_filters_on_its_own_field() {
    let emulator = Emulator::new();
    let base = base_instant();

    let mut write = served("CallWrite", HOME_REGION);
    write.resources = vec![AuditedResource::new("AWS::CloudTrail::Trail", "write-trail")];
    emulator.seed(write, base + Duration::from_secs(10));

    let mut read = served("CallRead", HOME_REGION);
    read.read_only = true;
    read.user_name = Some("auditor".to_string());
    read.access_key_id = Some("AKIDREADER".to_string());
    read.event_source = "cloudtrail.amazonaws.com".to_string();
    read.resources = vec![AuditedResource::new("AWS::S3::Bucket", "read-bucket")];
    let read_event_id = emulator.seed(read, base + Duration::from_secs(20));

    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("EventName", "CallWrite", vec!["CallWrite"]),
        ("EventName", "CallMissing", vec![]),
        ("EventId", read_event_id.as_str(), vec!["CallRead"]),
        ("EventId", "00000000-0000-4000-8000-000000000000", vec![]),
        ("ReadOnly", "true", vec!["CallRead"]),
        ("ReadOnly", "false", vec!["CallWrite"]),
        ("Username", "auditor", vec!["CallRead"]),
        ("Username", "emulator", vec!["CallWrite"]),
        ("AccessKeyId", "AKIDREADER", vec!["CallRead"]),
        ("AccessKeyId", ACCESS_KEY_ID, vec!["CallWrite"]),
        ("ResourceType", "AWS::S3::Bucket", vec!["CallRead"]),
        ("ResourceType", "AWS::CloudTrail::Trail", vec!["CallWrite"]),
        ("ResourceName", "write-trail", vec!["CallWrite"]),
        ("ResourceName", "no-such-resource", vec![]),
        ("EventSource", EVENT_SOURCE, vec!["CallRead", "CallWrite"]),
        ("EventSource", "lambda.amazonaws.com", vec![]),
    ];

    for (key, value, expected) in cases {
        let case = format!("{key}={value}");
        let body = json!({"LookupAttributes": [{"AttributeKey": key, "AttributeValue": value}]});
        let names = emulator.lookup(&body).await.event_names(&case);
        assert_eq!(
            names, expected,
            "case {case:?}: expected {expected:?}, got {names:?}"
        );
    }
}

/// A lookup filters on at most one attribute, the key must be one the model declares,
/// and `ReadOnly` takes only the two values an event reports it as.
#[tokio::test]
async fn an_invalid_lookup_attribute_is_refused() {
    let emulator = Emulator::new();
    emulator.seed(served("CallA", HOME_REGION), base_instant());

    let cases: Vec<(&str, Value)> = vec![
        (
            "two attributes",
            json!([
                {"AttributeKey": "EventName", "AttributeValue": "CallA"},
                {"AttributeKey": "EventSource", "AttributeValue": EVENT_SOURCE},
            ]),
        ),
        (
            "three attributes",
            json!([
                {"AttributeKey": "EventName", "AttributeValue": "CallA"},
                {"AttributeKey": "EventSource", "AttributeValue": EVENT_SOURCE},
                {"AttributeKey": "ReadOnly", "AttributeValue": "false"},
            ]),
        ),
        (
            "a key the model does not declare",
            json!([{"AttributeKey": "EventRegion", "AttributeValue": HOME_REGION}]),
        ),
        (
            "a key that differs only in case",
            json!([{"AttributeKey": "eventname", "AttributeValue": "CallA"}]),
        ),
        (
            "ReadOnly with a value that is not a boolean",
            json!([{"AttributeKey": "ReadOnly", "AttributeValue": "yes"}]),
        ),
        (
            "ReadOnly with a capitalised boolean",
            json!([{"AttributeKey": "ReadOnly", "AttributeValue": "True"}]),
        ),
    ];

    for (case, attributes) in cases {
        emulator
            .lookup(&json!({"LookupAttributes": attributes}))
            .await
            .expect_error(case, 400, "InvalidLookupAttributesException");
    }
}

/// `EventCategory` takes only the value the model declares, and it selects the Insights
/// events — of which a recorded management call is not one.
#[tokio::test]
async fn event_category_selects_insights_events_only() {
    let emulator = Emulator::new();
    emulator.seed(served("CallA", HOME_REGION), base_instant());

    // The catalog the handler maps comes from the model's `EventCategory` enum; if a
    // later revision adds a value, this pins that the mapping has to grow with it.
    assert_eq!(
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::EVENT_CATEGORY_VALUES,
        ["insight"],
        "case the modelled catalog: EventCategory's value set changed"
    );

    for case in ["management", "Management", "Insight", "INSIGHT", "data", ""] {
        emulator
            .lookup(&json!({"EventCategory": case}))
            .await
            .expect_error(
                &format!("EventCategory={case:?}"),
                400,
                "InvalidEventCategoryException",
            );
    }

    let names = emulator
        .lookup(&json!({"EventCategory": "insight"}))
        .await
        .event_names("EventCategory=insight");
    assert!(
        names.is_empty(),
        "case EventCategory=insight: a management call is not an Insights event, got {names:?}"
    );
    let names = emulator
        .lookup(&json!({}))
        .await
        .event_names("no EventCategory");
    assert_eq!(
        names,
        vec!["CallA"],
        "case no EventCategory: expected the management events, got {names:?}"
    );
}

/// The rewording of a boundary refusal is keyed on `LookupEvents`'s OWN members: another
/// operation's refusal on a member of the same name, nested inside its own shape, keeps
/// the framework's wording rather than inheriting this operation's modelled code.
#[tokio::test]
async fn another_operations_event_category_keeps_the_frameworks_refusal() {
    let emulator = Emulator::new();

    // `AggregationConfiguration$EventCategory` (reached through
    // `PutEventConfiguration`) is a different member of the same name: its value set is
    // `EventCategoryAggregation` (`Data`), so `Management` violates it — at the path
    // `aggregationConfigurations.1.member.eventCategory`, not at the root.
    let reply = emulator
        .call(
            "PutEventConfiguration",
            &json!({
                "TrailName": "demo-trail",
                "AggregationConfigurations": [{
                    "Templates": ["API_ACTIVITY"],
                    "EventCategory": "Management",
                }],
            }),
        )
        .await;
    reply.expect_error(
        "a nested EventCategory of another operation",
        400,
        "ValidationException",
    );
}

/// `MaxResults` bounds the page, the token walks the remainder exactly once, and asking
/// twice with the same token answers the same page.
#[tokio::test]
async fn max_results_bounds_the_page_and_the_token_is_stable() {
    let emulator = Emulator::new();
    let base = base_instant();
    let names: Vec<String> = (1..=5).map(|index| format!("Call{index}")).collect();
    for (index, name) in names.iter().enumerate() {
        emulator.seed(
            served(name, HOME_REGION),
            base + Duration::from_secs(10 * (index as u64 + 1)),
        );
    }
    let newest_first: Vec<String> = names.iter().rev().cloned().collect();

    for page_size in [1_usize, 2, 3, 5, 50] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let case = format!("MaxResults={page_size} page {pages}");
            let reply = emulator.lookup(&body).await;
            let page = reply.event_names(&case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} events, got {page:?}"
            );
            // The SAME token answers the SAME page: a token is a position, not a cursor
            // that advances by being used.
            let again = emulator.lookup(&body).await.event_names(&case);
            assert_eq!(
                again, page,
                "case {case:?}: the same token must answer the same page"
            );
            walked.extend(page);
            pages += 1;
            match reply.next_token(&case) {
                Some(next) => token = Some(next),
                None => break,
            }
            assert!(
                pages <= names.len(),
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, newest_first,
            "case MaxResults={page_size}: paging must report every event once, most \
             recent first"
        );
    }

    // A page that holds everything left carries no token.
    let reply = emulator.lookup(&json!({"MaxResults": 5})).await;
    assert_eq!(
        reply.next_token("the whole result set"),
        None,
        "case the whole result set: expected no token, got {reply:?}"
    );
}

/// `MaxResults` outside the bounds the model declares (`range 1..50`) is refused at the
/// boundary, before the handler runs, with the error THIS operation's model declares for a
/// limit that is not valid: `InvalidMaxResultsException` ("this exception is thrown if the
/// limit specified is not valid"), HTTP 400. `ValidationException` is not an error
/// `LookupEvents` declares, so a client catching the modelled one would miss the refusal
/// altogether; the narrowing is wired per operation — only those whose model carries the
/// shape — in `src/service.rs`.
#[tokio::test]
async fn a_max_results_outside_its_bounds_is_refused() {
    let emulator = Emulator::new();
    emulator.seed(served("CallA", HOME_REGION), base_instant());

    for case in [0, -1, 51, 1_000] {
        emulator
            .lookup(&json!({"MaxResults": case}))
            .await
            .expect_error(
                &format!("MaxResults={case}"),
                400,
                "InvalidMaxResultsException",
            );
    }
    for case in [1, 25, 50] {
        emulator
            .lookup(&json!({"MaxResults": case}))
            .await
            .expect_ok(&format!("MaxResults={case}"));
    }
}

/// A token this lookup did not mint — malformed, or minted for a different filter — is
/// refused rather than paging through a different result set.
#[tokio::test]
async fn a_token_from_another_lookup_is_refused() {
    let emulator = Emulator::new();
    let base = base_instant();
    for (index, name) in ["CallA", "CallB", "CallC"].iter().enumerate() {
        emulator.seed(
            served(name, HOME_REGION),
            base + Duration::from_secs(10 * (index as u64 + 1)),
        );
    }

    let token = emulator
        .lookup(&json!({"MaxResults": 1}))
        .await
        .next_token("the first page")
        .expect("the first page of three carries a token");

    let cases: Vec<(&str, Value)> = vec![
        ("an empty token", json!({"MaxResults": 1, "NextToken": ""})),
        (
            "a token that is not this encoding",
            json!({"MaxResults": 1, "NextToken": "not-a-token"}),
        ),
        (
            "a token with a tampered body",
            json!({"MaxResults": 1, "NextToken": format!("{token}00")}),
        ),
        (
            "a valid token replayed with a different time window",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "StartTime": epoch_seconds(base),
            }),
        ),
        (
            "a valid token replayed with a lookup attribute",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "LookupAttributes": [{"AttributeKey": "EventName", "AttributeValue": "CallA"}],
            }),
        ),
        (
            "a valid token replayed with another event category",
            json!({"MaxResults": 1, "NextToken": token.clone(), "EventCategory": "insight"}),
        ),
    ];

    for (case, body) in cases {
        emulator
            .lookup(&body)
            .await
            .expect_error(case, 400, "InvalidNextTokenException");
    }

    // The token still works for the lookup it was minted for.
    let names = emulator
        .lookup(&json!({"MaxResults": 1, "NextToken": token}))
        .await
        .event_names("the token's own lookup");
    assert_eq!(
        names,
        vec!["CallB"],
        "case the token's own lookup: expected the next page, got {names:?}"
    );
}

/// A lookup reports the events recorded in the Region it was made in.
#[tokio::test]
async fn events_are_scoped_to_their_region() {
    let emulator = Emulator::new();
    let base = base_instant();
    emulator.seed(served("CallHome", HOME_REGION), base + Duration::from_secs(10));
    emulator.seed(
        served("CallOther", OTHER_REGION),
        base + Duration::from_secs(20),
    );

    for (region, expected) in [(HOME_REGION, "CallHome"), (OTHER_REGION, "CallOther")] {
        let reply = emulator.call_in("LookupEvents", region, &json!({})).await;
        let names = reply.event_names(region);
        assert_eq!(
            names,
            vec![expected],
            "case {region}: a lookup reports its own Region's events, got {names:?}"
        );
    }
}
