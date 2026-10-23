// SPDX-License-Identifier: CC0-1.0

//! MemoryDB `DescribeEvents`, end to end over the wire.
//!
//! Every case here drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router, so what
//! is asserted is the emulator's HTTP behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * every call that creates, modifies or deletes a resource records ONE event per
//!   resource whose STORED state it moved — that resource's `SourceType`, its stored
//!   (folded) name and the instant of the change — and `DescribeEvents` reports exactly
//!   those events, so a store nothing has changed reports none;
//! * each event's `Message` states the operation that was performed and the resource(s)
//!   the request named for it, in AWS's `<Operation> <kind> <name> operation completed.`
//!   form, and a change to an ACL's membership records ONE event per member added or
//!   removed, naming that user AND that ACL;
//! * `SourceName` / `SourceType` narrow the listing to the events of one resource or
//!   one kind of resource, for every token the model's `SourceType` publishes, with
//!   `SourceName` matched against the identifier each event RECORDED exactly — a
//!   spelling the log does not hold selects nothing;
//! * the reported window is `StartTime`..`EndTime`, defaulting to the documented last
//!   hour or to `Duration` minutes before the end, and a `Duration` outside
//!   1..=20160 minutes (14 days, the age past which no event is kept) or an interval
//!   that ends before it begins is refused;
//! * the listing reports the window's events NEWEST FIRST and pages over the whole set
//!   in that order for any `MaxResults`;
//! * reading the listing changes nothing a later caller sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The suite-wide harness: ONE SigV4 request builder, ONE router constructor and ONE
// error-code reader for every suite in this crate.
use crate::cluster_lifecycle::{call, error_code, ok, router};
use crate::user_lifecycle::create_user;

/// The `SourceType` tokens the model publishes
/// (`crate::ops::types::SOURCE_TYPE_VALUES`, generated from
/// `smithy_query query="shape" shape="SourceType"`), taken from the crate under test so
/// the table cannot drift from the model. `node` and `subnet-group` are in it and are
/// recorded by nothing, which the filter cases below assert.
const SOURCE_TYPES: &[&str] =
    service_com_amazonaws_memorydb_amazonmemorydb::ops::types::SOURCE_TYPE_VALUES;

/// The largest `Duration` (in minutes) the listing serves: 14 days, from
/// `DescribeEvents` — "you can retrieve up to 14 days' worth of events if necessary".
/// Mirrors `crate::state::MAX_EVENT_DURATION_MINUTES`; exercised AT the bound and one
/// past it.
const MAX_DURATION_MINUTES: i64 = 14 * 24 * 60;

/// The page-size window every MemoryDB listing serves, from the service's own refusal
/// ("Must be between 1 and 50"), mirrored from `crate::state::MIN_MAX_RESULTS` /
/// `MAX_MAX_RESULTS`. Exercised AT each bound and one past it.
const MIN_MAX_RESULTS: i64 = 1;
const MAX_MAX_RESULTS: i64 = 50;

/// One event as the wire reports it, reduced to what the rule fixes.
type Reported = (String, String, String);

/// How an event message names the kind of resource it is about: the noun that reads
/// mid-sentence, which for an acronym is the acronym itself.
///
/// Spelled out HERE rather than taken from the crate under test, so each assertion
/// below is independent of the code that builds the message. The nouns follow the
/// `SourceType` tokens the model publishes (`crate::ops::types::SOURCE_TYPE_VALUES`).
fn noun(source_type: &str) -> &'static str {
    match source_type {
        "cluster" => "cluster",
        "acl" => "ACL",
        "user" => "user",
        "parameter-group" => "parameter group",
        other => panic!("no event is recorded for source type {other:?}"),
    }
}

/// The event one change must be reported as: its source type, its source name and the
/// message naming the operation that was performed and the resource(s) the request
/// named for it.
///
/// `operation` is the operation phrase's leading verb (`Create`, `Modify`, `Delete`),
/// and the message AWS records is `<Operation> <kind> <name> operation completed.` —
/// recorded MemoryDB behaviour, e.g. `Create user bob operation completed.` /
/// `Create ACL acl-one operation completed.`
fn expected(source_type: &str, source_name: &str, operation: &str) -> Reported {
    (
        source_type.to_string(),
        source_name.to_string(),
        format!(
            "{operation} {} {source_name} operation completed.",
            noun(source_type)
        ),
    )
}

/// The event ONE membership change must be reported as: against the ACL whose stored
/// `UserNames` moved, with a message naming BOTH the user and the ACL —
/// `Add user <u> to ACL <a> operation completed.` /
/// `Remove user <u> from ACL <a> operation completed.` (recorded MemoryDB behaviour).
fn expected_membership(acl_name: &str, operation: &str, user_name: &str) -> Reported {
    let joiner = match operation {
        "Add" => "to",
        "Remove" => "from",
        other => panic!("no membership event is recorded for operation {other:?}"),
    };
    (
        "acl".to_string(),
        acl_name.to_string(),
        format!(
            "{operation} user {user_name} {joiner} ACL {acl_name} operation completed."
        ),
    )
}

/// `Events` of a `DescribeEvents` body as `(SourceType, SourceName, Message)`, in wire
/// order.
fn reported(body: &Value, case: &str) -> Vec<Reported> {
    body["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Events in {body}"))
        .iter()
        .map(|event| {
            let member = |name: &str| {
                event
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: expected {name} in {event}"))
                    .to_string()
            };
            (member("SourceType"), member("SourceName"), member("Message"))
        })
        .collect()
}

/// `Events[*].Date` of a body, as the epoch seconds the `awsJson1_1` wire carries.
fn dates(body: &Value, case: &str) -> Vec<f64> {
    body["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Events in {body}"))
        .iter()
        .map(|event| {
            event
                .get("Date")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case:?}: expected Date in {event}"))
        })
        .collect()
}

/// `DescribeEvents` with `body`, which must succeed.
async fn describe(router: &MultiServiceRouter, body: &Value, case: &str) -> Value {
    ok(router, "DescribeEvents", body, case).await
}

/// The whole event log, in the order the service reports it.
async fn whole_log(router: &MultiServiceRouter, case: &str) -> Vec<Reported> {
    // A window wide enough for every event the suite records, so the listing is the
    // whole log rather than the default last hour.
    let body = json!({ "StartTime": 0, "MaxResults": MAX_MAX_RESULTS });
    reported(&describe(router, &body, case).await, case)
}

/// Now, in the epoch seconds the `awsJson1_1` wire carries a `TStamp` as.
fn now_secs() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("the clock is at or after the epoch")
            .as_secs(),
    )
    .expect("the current epoch second fits in an i64")
}

/// Drive one change of every kind this service persists, and return the events they
/// must be reported as, in the order the listing reports them: NEWEST FIRST.
///
/// The rows differ along everything the rule generalises over: the kind of resource
/// (each `SourceType` any write path in this service touches), the kind of change
/// (create, modify, delete), the spelling of the name (a mixed-case name is stored and
/// so reported folded) and the count (a call that moves the stored state of more than
/// one resource — a delete that also leaves an ACL's `UserNames` — records one event
/// for each of them).
async fn drive_every_kind_of_change(router: &MultiServiceRouter) -> Vec<Reported> {
    create_user(router, "alice").await;
    // A mixed-case name: a MemoryDB resource name is a case-insensitive identifier, so
    // the event carries the folded spelling the store keys by.
    create_user(router, "Bob-2").await;
    ok(
        router,
        "CreateACL",
        &json!({ "ACLName": "Acl-One", "UserNames": ["alice"] }),
        "CreateACL",
    )
    .await;
    ok(
        router,
        "UpdateACL",
        &json!({ "ACLName": "acl-one", "UserNamesToAdd": ["bob-2"] }),
        "UpdateACL",
    )
    .await;
    ok(
        router,
        "CreateParameterGroup",
        &json!({ "ParameterGroupName": "pg-one", "Family": "memorydb_redis7" }),
        "CreateParameterGroup",
    )
    .await;
    ok(
        router,
        "UpdateParameterGroup",
        &json!({
            "ParameterGroupName": "pg-one",
            "ParameterNameValues": [
                { "ParameterName": "maxmemory-policy", "ParameterValue": "allkeys-lru" }
            ],
        }),
        "UpdateParameterGroup",
    )
    .await;
    ok(
        router,
        "CreateCluster",
        &json!({
            "ClusterName": "cl-one",
            "NodeType": "db.t4g.small",
            "ACLName": "acl-one",
        }),
        "CreateCluster",
    )
    .await;
    ok(
        router,
        "UpdateCluster",
        &json!({ "ClusterName": "cl-one", "Description": "a different description" }),
        "UpdateCluster",
    )
    .await;
    ok(
        router,
        "DeleteCluster",
        &json!({ "ClusterName": "cl-one" }),
        "DeleteCluster",
    )
    .await;
    // A delete of a user that an ACL still lists: the user is gone AND that ACL's
    // `UserNames` — where membership is stored — is a member shorter, so each resource
    // whose stored state moved gets its own event.
    ok(
        router,
        "DeleteUser",
        &json!({ "UserName": "alice" }),
        "DeleteUser",
    )
    .await;

    // The changes above in the order they were made; the listing reports the reverse.
    let mut recorded = vec![
        expected("user", "alice", "Create"),
        expected("user", "bob-2", "Create"),
        // An ACL's initial membership is part of the resource the create made, so
        // `CreateACL` with a member is ONE `acl` event for the ACL it created...
        expected("acl", "acl-one", "Create"),
        // ...while `UpdateACL` MOVES an existing ACL's membership, so it records one
        // event per member it added, naming that user and that ACL.
        expected_membership("acl-one", "Add", "bob-2"),
        expected("parameter-group", "pg-one", "Create"),
        expected("parameter-group", "pg-one", "Modify"),
        expected("cluster", "cl-one", "Create"),
        expected("cluster", "cl-one", "Modify"),
        expected("cluster", "cl-one", "Delete"),
        // `DeleteUser`: the user, and the ACL whose membership it left — a membership
        // change like any other, so the event names the user and the ACL.
        expected("user", "alice", "Delete"),
        expected_membership("acl-one", "Remove", "alice"),
    ];
    recorded.reverse();
    recorded
}

/// Every write path records one event for each resource it changed, and the listing
/// reports them newest first.
#[tokio::test]
async fn every_change_is_reported_as_an_event_for_the_resource_it_changed() {
    let router = router();
    let case = "every kind of change";
    let expected_log = drive_every_kind_of_change(&router).await;
    let body = describe(&router, &json!({ "StartTime": 0, "MaxResults": 50 }), case).await;

    assert_eq!(
        reported(&body, case),
        expected_log,
        "case {case:?}: expected one event per changed resource, got {body}"
    );
    let reported_dates = dates(&body, case);
    assert!(
        reported_dates.windows(2).all(|pair| pair[0] >= pair[1]),
        "case {case:?}: expected the events newest first, got dates {reported_dates:?}"
    );
    // Every reported date is the instant of the change, so it lies in the window the
    // suite ran in rather than being a fixed or absent value.
    let upper = now_secs() + 1;
    let lower = upper - 3600;
    assert!(
        reported_dates
            .iter()
            .all(|date| *date >= lower as f64 && *date <= upper as f64),
        "case {case:?}: expected every Date inside {lower}..={upper}, got {reported_dates:?}"
    );
}

/// A store nothing has changed reports no events, and reading the listing changes
/// nothing a later caller sees.
#[tokio::test]
async fn a_store_nothing_changed_reports_no_events() {
    let router = router();
    let case = "untouched store";
    let body = describe(&router, &json!({}), case).await;
    assert_eq!(
        reported(&body, case),
        Vec::<Reported>::new(),
        "case {case:?}: expected no events before any change, got {body}"
    );

    // One change, one event — and a second read reports exactly the same listing, so
    // the read neither records nor consumes anything.
    create_user(&router, "solo-user").await;
    let first = whole_log(&router, "first read").await;
    let second = whole_log(&router, "second read").await;
    assert_eq!(
        first,
        vec![expected("user", "solo-user", "Create")],
        "case {case:?}: expected the one recorded event, got {first:?}"
    );
    assert_eq!(
        second, first,
        "case {case:?}: expected a read to leave the log alone, got {second:?} then {first:?}"
    );
}

/// `SourceName` and `SourceType` narrow the listing to the events of one resource, one
/// kind of resource, or both.
#[tokio::test]
async fn a_source_filter_narrows_the_listing() {
    let router = router();
    let whole = drive_every_kind_of_change(&router).await;

    // EVERY token the model's `SourceType` publishes, including the two no write path
    // in this service records (`node`, `subnet-group`), which must select nothing
    // rather than everything.
    for source_type in SOURCE_TYPES {
        let case = format!("SourceType={source_type:?}");
        let body = describe(
            &router,
            &json!({ "StartTime": 0, "MaxResults": 50, "SourceType": source_type }),
            &case,
        )
        .await;
        let want: Vec<Reported> = whole
            .iter()
            .filter(|event| event.0 == *source_type)
            .cloned()
            .collect();
        assert_eq!(
            reported(&body, &case),
            want,
            "case {case:?}: expected only that kind's events, got {body}"
        );
    }

    // A name as the log RECORDED it, the same name in other spellings (upper, mixed —
    // each names no recorded event, because the filter matches the recorded identifier
    // exactly rather than folding it), a name nothing recorded, and a name paired with a
    // type that both does and does not hold its events.
    let name_cases: &[(&str, Option<&str>)] = &[
        ("alice", None),
        ("ALICE", None),
        ("Alice", None),
        ("acl-one", None),
        ("ACL-One", None),
        ("bob-2", None),
        ("Bob-2", None),
        ("cl-one", None),
        ("no-such-resource", None),
        ("alice", Some("user")),
        ("ALICE", Some("user")),
        ("alice", Some("acl")),
        ("acl-one", Some("acl")),
        ("ACL-ONE", Some("acl")),
        ("pg-one", Some("parameter-group")),
        ("pg-one", Some("node")),
    ];
    for (source_name, source_type) in name_cases {
        let case = format!("SourceName={source_name:?} SourceType={source_type:?}");
        let mut request = json!({
            "StartTime": 0,
            "MaxResults": 50,
            "SourceName": source_name,
        });
        if let Some(source_type) = source_type {
            request["SourceType"] = json!(source_type);
        }
        let body = describe(&router, &request, &case).await;
        let want: Vec<Reported> = whole
            .iter()
            .filter(|event| {
                event.1 == *source_name && source_type.is_none_or(|wanted| event.0 == wanted)
            })
            .cloned()
            .collect();
        assert_eq!(
            reported(&body, &case),
            want,
            "case {case:?}: expected only that resource's events, got {body}"
        );
    }
}

/// The reported window is `StartTime`..`EndTime`, defaulting to the documented last
/// hour and to `Duration` minutes before the end.
#[tokio::test]
async fn the_time_window_selects_which_events_are_reported() {
    let router = router();
    let whole = drive_every_kind_of_change(&router).await;
    let now = now_secs();
    let hour = 3_600;

    // Each row bounds the window differently; `all` says whether the events this suite
    // just recorded fall inside it.
    let cases: &[(&str, Value, bool)] = &[
        // No bound at all: the documented default of the last hour.
        ("default window", json!({}), true),
        // The smallest and the largest `Duration` the service serves.
        ("Duration=1", json!({ "Duration": 1 }), true),
        (
            "Duration at the 14-day bound",
            json!({ "Duration": MAX_DURATION_MINUTES }),
            true,
        ),
        // An explicit interval around now, and one that ends before the events.
        (
            "StartTime and EndTime around now",
            json!({ "StartTime": now - hour, "EndTime": now + hour }),
            true,
        ),
        (
            "window closed an hour ago",
            json!({ "EndTime": now - hour }),
            false,
        ),
        (
            "window closed at the epoch",
            json!({ "StartTime": 0, "EndTime": 0 }),
            false,
        ),
        // A start before the events with no end: everything up to now.
        ("StartTime at the epoch", json!({ "StartTime": 0 }), true),
        // A start AFTER the events but inside an explicit interval reaching into the
        // future: nothing yet to report, with the window a fractional timestamp names.
        (
            "StartTime one second ahead",
            json!({ "StartTime": (now + 1) as f64 + 0.5, "EndTime": now + hour }),
            false,
        ),
        // `StartTime` is the stronger statement of the same bound, so it is what the
        // window starts at when both it and `Duration` are supplied.
        (
            "StartTime wins over Duration",
            json!({ "StartTime": 0, "Duration": 1 }),
            true,
        ),
    ];
    for (case, mut request, inside) in cases.iter().cloned() {
        request["MaxResults"] = json!(MAX_MAX_RESULTS);
        let body = describe(&router, &request, case).await;
        let want: Vec<Reported> = if inside { whole.clone() } else { Vec::new() };
        assert_eq!(
            reported(&body, case),
            want,
            "case {case:?}: expected {} events in the window, got {body}",
            want.len()
        );
    }
}

/// The listing pages over the whole log newest first for any `MaxResults`,
/// and a page's `NextToken` names the first event of the next page.
#[tokio::test]
async fn the_listing_pages_over_the_whole_log() {
    let router = router();
    let whole = drive_every_kind_of_change(&router).await;

    for page_size in [1_i64, 2, 3, 5, 11, MAX_MAX_RESULTS] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<Reported> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({ "StartTime": 0, "MaxResults": page_size });
            if let Some(token) = next_token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let body = describe(&router, &request, &case).await;
            let page = reported(&body, &case);
            assert!(
                i64::try_from(page.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: expected at most {page_size} events on a page, got {}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: expected the walk to end, got {pages} pages"
            );
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the pages to cover the whole log in order, got {walked:?}"
        );
    }
}

/// A member whose value names no listing is refused with the service's own modelled
/// 400, never served as a silently empty or unbounded page.
#[tokio::test]
async fn a_request_that_names_no_listing_is_refused() {
    let router = router();
    drive_every_kind_of_change(&router).await;
    let now = now_secs();

    // Each row: the request, the status it must answer, and the modelled error code.
    let cases: &[(&str, Value, u16, &str)] = &[
        // The page-size window, at each bound and one past it.
        (
            "MaxResults below the window",
            json!({ "MaxResults": MIN_MAX_RESULTS - 1 }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "MaxResults above the window",
            json!({ "MaxResults": MAX_MAX_RESULTS + 1 }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "MaxResults at the lower bound",
            json!({ "MaxResults": MIN_MAX_RESULTS }),
            200,
            "",
        ),
        (
            "MaxResults at the upper bound",
            json!({ "MaxResults": MAX_MAX_RESULTS }),
            200,
            "",
        ),
        // The `Duration` window, at each bound and one past it.
        (
            "Duration of zero minutes",
            json!({ "Duration": 0 }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "negative Duration",
            json!({ "Duration": -5 }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "Duration past the 14-day bound",
            json!({ "Duration": MAX_DURATION_MINUTES + 1 }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "Duration at the 14-day bound",
            json!({ "Duration": MAX_DURATION_MINUTES }),
            200,
            "",
        ),
        // An interval that ends before it begins holds no instant.
        (
            "StartTime after EndTime",
            json!({ "StartTime": now, "EndTime": now - 3_600 }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "StartTime equal to EndTime",
            json!({ "StartTime": now, "EndTime": now }),
            200,
            "",
        ),
        // A `SourceType` the model does not publish — including the token the sibling
        // service ElastiCache uses for the same idea.
        (
            "SourceType the model does not publish",
            json!({ "SourceType": "cache-cluster" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "SourceType in another letter case",
            json!({ "SourceType": "Cluster" }),
            400,
            "InvalidParameterValueException",
        ),
        // A token that names no position in this listing.
        (
            "NextToken naming no position",
            json!({ "StartTime": 0, "NextToken": "99999999999999999999" }),
            400,
            "InvalidParameterValueException",
        ),
    ];
    for (case, request, status, code) in cases.iter().cloned() {
        let (answered, body) = call(&router, "DescribeEvents", &request).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} with {body}"
        );
        if status == 200 {
            continue;
        }
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
    }
}

/// Every recorded message states the operation that was performed and the resource(s)
/// the request named for it, and ends with AWS's ` operation completed.`
///
/// The invariant over the WHOLE log rather than one row: whatever the kind of resource
/// or the kind of change, the message opens with an operation phrase this service can
/// have performed and names the resource the event is recorded against.
#[tokio::test]
async fn every_message_names_an_operation_and_the_resource_it_was_performed_on() {
    let router = router();
    // The operation phrases a MemoryDB event message can open with, from the recorded
    // behaviour this suite's `expected` / `expected_membership` spell out: the lifecycle
    // verbs, and the two a membership change uses.
    const OPERATIONS: &[&str] = &["Create", "Modify", "Delete", "Add", "Remove"];
    const TAIL: &str = " operation completed.";

    drive_every_kind_of_change(&router).await;
    let log = whole_log(&router, "message form").await;
    assert!(
        !log.is_empty(),
        "case \"message form\": expected the driven changes to be recorded, got {log:?}"
    );
    for (source_type, source_name, message) in &log {
        let case = format!("{source_type} event for {source_name}");
        assert!(
            message.ends_with(TAIL),
            "case {case:?}: expected the message to end with {TAIL:?}, got {message:?}"
        );
        assert!(
            OPERATIONS
                .iter()
                .any(|operation| message.starts_with(&format!("{operation} "))),
            "case {case:?}: expected the message to open with one of {OPERATIONS:?}, got \
             {message:?}"
        );
        assert!(
            message.contains(&format!("{} {source_name} ", noun(source_type))),
            "case {case:?}: expected the message to name {} {source_name}, got {message:?}",
            noun(source_type)
        );
    }
}

/// A change to an ACL's membership records ONE event per member added or removed,
/// against the ACL whose stored `UserNames` moved, and each such message names BOTH the
/// user and the ACL the request named for it.
///
/// The rows differ along what the rule generalises over: the direction of the change
/// (added, removed, both in one call), the COUNT of members one call moves, the spelling
/// of the names (an event carries the folded name the store keys by) and the operation
/// that makes the change — `UpdateACL`, and the `DeleteUser` that takes a member out of
/// every ACL that listed it.
#[tokio::test]
async fn a_membership_change_records_one_event_per_member_naming_both_resources() {
    let router = router();
    let acl = "members-acl";
    for user in ["alpha", "bravo", "Charlie", "delta"] {
        create_user(&router, user).await;
    }
    // A mixed-case ACL name: a MemoryDB resource name is a case-insensitive identifier,
    // so every event below carries the folded spelling.
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "Members-Acl" }),
        "CreateACL",
    )
    .await;

    // Each row: the `UpdateACL` request, and the events it must record, oldest first.
    let rows: Vec<(&str, Value, Vec<Reported>)> = vec![
        (
            "one member added",
            json!({ "ACLName": acl, "UserNamesToAdd": ["alpha"] }),
            vec![expected_membership(acl, "Add", "alpha")],
        ),
        (
            "two members added by one call, one named in mixed case",
            json!({ "ACLName": acl, "UserNamesToAdd": ["bravo", "Charlie"] }),
            vec![
                expected_membership(acl, "Add", "bravo"),
                expected_membership(acl, "Add", "charlie"),
            ],
        ),
        (
            "one member removed, named in mixed case",
            json!({ "ACLName": "MEMBERS-ACL", "UserNamesToRemove": ["ALPHA"] }),
            vec![expected_membership(acl, "Remove", "alpha")],
        ),
        (
            "a member added and two removed by one call",
            json!({
                "ACLName": acl,
                "UserNamesToAdd": ["delta"],
                "UserNamesToRemove": ["bravo", "charlie"],
            }),
            vec![
                expected_membership(acl, "Add", "delta"),
                expected_membership(acl, "Remove", "bravo"),
                expected_membership(acl, "Remove", "charlie"),
            ],
        ),
    ];

    let mut oldest_first = vec![expected("acl", acl, "Create")];
    for (case, request, recorded) in rows {
        ok(&router, "UpdateACL", &request, case).await;
        oldest_first.extend(recorded);
        let mut want = oldest_first.clone();
        want.reverse();
        let body = describe(
            &router,
            &json!({
                "StartTime": 0,
                "MaxResults": MAX_MAX_RESULTS,
                "SourceType": "acl",
                "SourceName": acl,
            }),
            case,
        )
        .await;
        assert_eq!(
            reported(&body, case),
            want,
            "case {case:?}: expected one event per member changed, newest first, got {body}"
        );
    }

    // A member taken out of the ACL by a `DeleteUser` is the same kind of change, so it
    // is recorded the same way: the user's own delete event, and one membership event
    // per ACL the user left.
    let case = "a member removed by deleting the user";
    ok(
        &router,
        "DeleteUser",
        &json!({ "UserName": "delta" }),
        case,
    )
    .await;
    oldest_first.push(expected_membership(acl, "Remove", "delta"));
    let mut want = oldest_first.clone();
    want.reverse();
    let body = describe(
        &router,
        &json!({
            "StartTime": 0,
            "MaxResults": MAX_MAX_RESULTS,
            "SourceType": "acl",
            "SourceName": acl,
        }),
        case,
    )
    .await;
    assert_eq!(
        reported(&body, case),
        want,
        "case {case:?}: expected the membership the delete ended to be recorded, got {body}"
    );

    // The membership events belong to the ACL whose stored state moved: a user's own
    // listing holds only the events of the user resource itself.
    for (user, mut user_events) in [
        ("alpha", vec![expected("user", "alpha", "Create")]),
        (
            "delta",
            vec![
                expected("user", "delta", "Create"),
                expected("user", "delta", "Delete"),
            ],
        ),
    ] {
        let case = format!("user listing for {user}");
        user_events.reverse();
        let body = describe(
            &router,
            &json!({
                "StartTime": 0,
                "MaxResults": MAX_MAX_RESULTS,
                "SourceType": "user",
                "SourceName": user,
            }),
            &case,
        )
        .await;
        assert_eq!(
            reported(&body, &case),
            user_events,
            "case {case:?}: expected only the user's own events, got {body}"
        );
    }
}
