// SPDX-License-Identifier: CC0-1.0

//! The archive REGISTRY and the API-destination listing end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the
//! real `MultiServiceRouter`: `ListArchives` enumerates what `CreateArchive`
//! wrote, `UpdateArchive` changes one, `DeleteArchive` removes one and
//! `ListApiDestinations` enumerates the destinations.
//!
//! The RULES asserted here:
//!
//! * `ListArchives` reports the archives this account holds, projected from the
//!   SAME stored records `DescribeArchive` reports, in name order; ONE filter
//!   narrows the listing (`NamePrefix`, `EventSourceArn` or `State`) and naming
//!   more than one is refused, because "filter parameters are exclusive"; a page
//!   is bounded by `Limit` and resumed with the `NextToken` the previous page
//!   answered;
//! * `UpdateArchive` REPLACES each member the request carries and leaves every
//!   member it does not name as it was; the new `EventPattern` is what the
//!   archive ingests by from then on and is mirrored into the managed rule that
//!   serves the archive on its source bus, and the archive is answered in a
//!   non-terminal `UPDATING` state that reaches `ENABLED` on its own;
//! * `DeleteArchive` removes the archive and the managed rule that fed it, frees
//!   the name for a new archive, leaves every other archive alone, and refuses a
//!   name this account holds no archive for;
//! * `ListApiDestinations` reports the destinations this account holds with the
//!   state each one's connection gives it, narrowed by `NamePrefix` and
//!   `ConnectionArn` — the two members narrow TOGETHER, because nothing in this
//!   operation's contract makes them exclusive;
//! * a replay never leaves the event bus its events were archived from: a
//!   `Destination.Arn` naming another bus and a `Destination.FilterArns` naming a
//!   rule that is not on the destination bus are each refused with AWS's sentence
//!   for that member, so the wording is asserted and not only the code.
//!
//! Each table spans the class rather than one case: the rows differ along which
//! filter narrows the listing and what it selects, which members an update
//! carries, whether the archive it is applied to filters at all, and which
//! archive of several is deleted.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// How long a test waits before reading a resource whose next state the clock
/// owes it: the span itself plus enough slack that a loaded machine does not make
/// the wait the flaky part of the assertion.
fn past_the_transition(span: Duration) -> Duration {
    span + Duration::from_millis(250)
}

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

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// The `message` a failure body carries.
fn error_message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// A custom event bus called `name`; answers the ARN it reports.
async fn given_bus(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = events(router, "CreateEventBus", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateEventBus answered {status} {body}"
    );
    body["EventBusArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no EventBusArn in {body}"))
        .to_string()
}

/// An archive called `name` on the bus `bus_arn` names, created with `extra`
/// members merged into the request; answers the ARN it reports.
async fn given_archive(
    router: &MultiServiceRouter,
    name: &str,
    bus_arn: &str,
    extra: &Value,
) -> String {
    let mut request = json!({ "ArchiveName": name, "EventSourceArn": bus_arn });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    let (status, body) = events(router, "CreateArchive", &request).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateArchive answered {status} {body}"
    );
    body["ArchiveArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no ArchiveArn in {body}"))
        .to_string()
}

/// The `ArchiveName`s a `ListArchives` body reports, in the order it reported
/// them.
fn archive_names(body: &Value) -> Vec<String> {
    body["Archives"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|archive| archive["ArchiveName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `Name`s a `ListApiDestinations` body reports, in the order it reported
/// them.
fn destination_names(body: &Value) -> Vec<String> {
    body["ApiDestinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|destination| destination["Name"].as_str().unwrap_or_default().to_string())
        .collect()
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

/// What `DescribeArchive` reports for `name`.
async fn describe_archive(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = events(router, "DescribeArchive", &json!({ "ArchiveName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeArchive answered {status} {body}"
    );
    body
}

/// How many events `name` holds, as `DescribeArchive` counts them.
async fn event_count(router: &MultiServiceRouter, case: &str, name: &str) -> i64 {
    let described = describe_archive(router, case, name).await;
    described["EventCount"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no EventCount in {described}"))
}

/// `ListArchives` reports the stored archives in name order, ONE filter narrows
/// the listing, and the page is bounded and resumable.
#[tokio::test]
async fn list_archives_reports_the_stored_archives_and_one_filter_narrows_them() {
    let router = emulator();
    let alpha = given_bus(&router, "alpha-bus").await;
    let beta = given_bus(&router, "beta-bus").await;
    // Three archives spanning the shapes an archive comes in: a filtered one, an
    // unfiltered one with a retention, and one on the OTHER bus.
    let _ = given_archive(
        &router,
        "ord-alpha",
        &alpha,
        &json!({ "EventPattern": json!({ "source": ["com.demo.orders"] }).to_string() }),
    )
    .await;
    let _ = given_archive(&router, "ord-beta", &alpha, &json!({ "RetentionDays": 7 })).await;
    let _ = given_archive(&router, "inv-gamma", &beta, &json!({})).await;

    // An archive is answered in `CREATING`, so the `State` filter discriminates on
    // the state the archive is in at the instant of the read: this listing happens
    // before the clock has made `ENABLED` due.
    let (status, creating) = events(
        &router,
        "ListArchives",
        &json!({ "State": state::ARCHIVE_STATE_CREATING }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the CREATING listing answered {status} {creating}"
    );
    assert!(
        !archive_names(&creating).is_empty(),
        "an archive is CREATING before the transition falls due, got {creating}"
    );

    tokio::time::sleep(past_the_transition(state::ARCHIVE_TRANSITION)).await;

    // (case, the request, the names it has to report in order)
    let rows: [(&str, Value, Vec<&str>); 7] = [
        (
            "no filter at all",
            json!({}),
            vec!["inv-gamma", "ord-alpha", "ord-beta"],
        ),
        (
            "a NamePrefix two of the three names start with",
            json!({ "NamePrefix": "ord" }),
            vec!["ord-alpha", "ord-beta"],
        ),
        (
            "a NamePrefix that is a whole name",
            json!({ "NamePrefix": "inv-gamma" }),
            vec!["inv-gamma"],
        ),
        (
            "a NamePrefix no name starts with",
            json!({ "NamePrefix": "zzz" }),
            vec![],
        ),
        (
            "the EventSourceArn of the bus two archives are bound to",
            json!({ "EventSourceArn": alpha }),
            vec!["ord-alpha", "ord-beta"],
        ),
        (
            "the EventSourceArn of the other bus",
            json!({ "EventSourceArn": beta }),
            vec!["inv-gamma"],
        ),
        (
            "the State every archive has reached",
            json!({ "State": state::ARCHIVE_STATE_ENABLED }),
            vec!["inv-gamma", "ord-alpha", "ord-beta"],
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = events(&router, "ListArchives", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListArchives answered {status} {body}"
        );
        assert_eq!(
            archive_names(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
        assert!(
            body.get("NextToken").is_none() || body["NextToken"].is_null(),
            "case {case:?}: a listing that returned everything carries no token, got {body}"
        );
    }

    // The summary is projected from the same record the read reports.
    let (status, body) = events(&router, "ListArchives", &json!({ "NamePrefix": "ord-beta" })).await;
    assert_eq!(status, 200, "the summary listing answered {status} {body}");
    let summary = body["Archives"][0].clone();
    let described = describe_archive(&router, "the summary", "ord-beta").await;
    for member in [
        "ArchiveName",
        "EventSourceArn",
        "State",
        "RetentionDays",
        "EventCount",
        "SizeBytes",
        "CreationTime",
    ] {
        assert_eq!(
            summary[member], described[member],
            "case {member:?}: the listing and the read have to agree, got {summary} and {described}"
        );
    }

    // `Limit` bounds the page and the token it answers resumes it, so the pages
    // together are the whole listing and no name appears twice.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..3 {
        let mut request = json!({ "Limit": 1 });
        if let Some(token) = token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let (status, body) = events(&router, "ListArchives", &request).await;
        assert_eq!(
            status, 200,
            "case page {page}: ListArchives answered {status} {body}"
        );
        let names = archive_names(&body);
        assert_eq!(
            names.len(),
            1,
            "case page {page}: a Limit of 1 is one archive, got {body}"
        );
        seen.extend(names);
        token = body["NextToken"].as_str().map(str::to_string);
        assert_eq!(
            token.is_some(),
            page < 2,
            "case page {page}: a token is answered only while archives remain, got {body}"
        );
    }
    assert_eq!(
        seen,
        vec!["inv-gamma", "ord-alpha", "ord-beta"],
        "the pages together are the whole listing in name order, got {seen:?}"
    );
}

/// What `ListArchives` refuses: more than one filter, a `State` the enum does not
/// have, and a token this service did not issue.
#[tokio::test]
async fn list_archives_refuses_more_than_one_filter() {
    let router = emulator();
    let bus = given_bus(&router, "refusal-bus").await;
    let _ = given_archive(&router, "only-archive", &bus, &json!({})).await;

    // (case, the request) — every pair of the three filters, then all three.
    let rows: [(&str, Value); 6] = [
        (
            "a NamePrefix and a State",
            json!({ "NamePrefix": "only", "State": state::ARCHIVE_STATE_ENABLED }),
        ),
        (
            "a NamePrefix and an EventSourceArn",
            json!({ "NamePrefix": "only", "EventSourceArn": bus }),
        ),
        (
            "a State and an EventSourceArn",
            json!({ "State": state::ARCHIVE_STATE_ENABLED, "EventSourceArn": bus }),
        ),
        (
            "all three filters at once",
            json!({
                "NamePrefix": "only",
                "State": state::ARCHIVE_STATE_ENABLED,
                "EventSourceArn": bus,
            }),
        ),
        (
            "a State the enum does not have",
            json!({ "State": "SLEEPING" }),
        ),
        (
            "a NextToken this service did not issue",
            json!({ "NextToken": "not-a-token" }),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = events(&router, "ListArchives", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
    }
}

/// `UpdateArchive` replaces the members the request carries, leaves the rest
/// alone, and the new `EventPattern` is what the archive ingests by afterwards.
#[tokio::test]
async fn update_archive_replaces_only_the_members_it_carries() {
    let orders = json!({ "source": ["com.demo.orders"] }).to_string();
    let invoices = json!({ "source": ["com.demo.invoices"] }).to_string();

    // (case, the update request, what Description / EventPattern / RetentionDays /
    //  KmsKeyIdentifier have to read back as afterwards)
    let rows: [(&str, Value, Value); 5] = [
        (
            "a description alone, which leaves the filter as it was",
            json!({ "Description": "the orders archive" }),
            json!({
                "Description": "the orders archive",
                "EventPattern": orders,
                "RetentionDays": 3,
            }),
        ),
        (
            "a narrower pattern alone, which leaves the description as it was",
            json!({ "EventPattern": invoices }),
            json!({
                "Description": "as created",
                "EventPattern": invoices,
                "RetentionDays": 3,
            }),
        ),
        (
            "a retention alone",
            json!({ "RetentionDays": 30 }),
            json!({
                "Description": "as created",
                "EventPattern": orders,
                "RetentionDays": 30,
            }),
        ),
        (
            "an empty pattern, which is the absence of a filter",
            json!({ "EventPattern": "" }),
            json!({ "Description": "as created", "RetentionDays": 3 }),
        ),
        (
            "every member at once, including the key it is encrypted under",
            json!({
                "Description": "everything",
                "EventPattern": invoices,
                "RetentionDays": 0,
                "KmsKeyIdentifier": "the-archive-key",
            }),
            json!({
                "Description": "everything",
                "EventPattern": invoices,
                "RetentionDays": 0,
                "KmsKeyIdentifier": format!(
                    "arn:aws:kms:{REGION}:{}:key/the-archive-key",
                    engine::emulator_account_id()
                ),
            }),
        ),
    ];

    for (case, update, expected) in rows {
        let router = emulator();
        let bus = given_bus(&router, "update-bus").await;
        let created = given_archive(
            &router,
            "updated-archive",
            &bus,
            &json!({
                "Description": "as created",
                "EventPattern": orders,
                "RetentionDays": 3,
            }),
        )
        .await;
        tokio::time::sleep(past_the_transition(state::ARCHIVE_TRANSITION)).await;
        let before = describe_archive(&router, case, "updated-archive").await;

        let mut request = json!({ "ArchiveName": "updated-archive" });
        for (member, value) in update.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        let (status, answered) = events(&router, "UpdateArchive", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateArchive answered {status} {answered}"
        );
        assert_eq!(
            answered["ArchiveArn"],
            json!(created),
            "case {case:?}: the update answers the archive's own ARN, got {answered}"
        );
        assert_eq!(
            answered["State"],
            json!(state::ARCHIVE_STATE_UPDATING),
            "case {case:?}: an updated archive is {}, got {answered}",
            state::ARCHIVE_STATE_UPDATING
        );
        assert_eq!(
            answered["CreationTime"], before["CreationTime"],
            "case {case:?}: the archive was not created again, got {answered}"
        );

        // It reaches `ENABLED` again on its own.
        tokio::time::sleep(past_the_transition(state::ARCHIVE_TRANSITION)).await;
        let after = describe_archive(&router, case, "updated-archive").await;
        assert_eq!(
            after["State"],
            json!(state::ARCHIVE_STATE_ENABLED),
            "case {case:?}: the archive settles back into {}, got {after}",
            state::ARCHIVE_STATE_ENABLED
        );
        for member in [
            "Description",
            "EventPattern",
            "RetentionDays",
            "KmsKeyIdentifier",
        ] {
            let wanted = expected.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                after.get(member).cloned().unwrap_or(Value::Null),
                wanted,
                "case {case:?}: expected {member} to read back as {wanted}, got {after}"
            );
        }

        // The managed rule that serves the archive carries the archive's pattern
        // plus the clause that keeps a replayed event out of it, whatever the
        // archive's own pattern now is.
        let rule_name = "Events-Archive-updated-archive";
        let (status, rule) = events(
            &router,
            "DescribeRule",
            &json!({ "Name": rule_name, "EventBusName": "update-bus" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeRule answered {status} {rule}"
        );
        let mirrored: Value = serde_json::from_str(rule["EventPattern"].as_str().unwrap_or("null"))
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the rule's pattern is not JSON ({error}): {rule}")
            });
        assert_eq!(
            mirrored["replay-name"],
            json!([{ "exists": false }]),
            "case {case:?}: a replayed event is kept out of the archive, got {mirrored}"
        );
        let archive_pattern: Value = after["EventPattern"]
            .as_str()
            .map(|text| serde_json::from_str(text).unwrap_or(Value::Null))
            .unwrap_or(json!({}));
        for (member, value) in archive_pattern.as_object().cloned().unwrap_or_default() {
            assert_eq!(
                mirrored[&member], value,
                "case {case:?}: the rule mirrors the archive's {member} clause, got {mirrored}"
            );
        }
    }
}

/// The pattern an update writes governs what the archive ingests FROM THEN ON: an
/// event the old pattern matched and the new one does not is no longer captured,
/// and one the new pattern matches is.
#[tokio::test]
async fn the_updated_pattern_governs_what_is_archived_afterwards() {
    let router = emulator();
    let bus = given_bus(&router, "ingest-bus").await;
    let _ = given_archive(
        &router,
        "ingest-archive",
        &bus,
        &json!({ "EventPattern": json!({ "source": ["com.demo.orders"] }).to_string() }),
    )
    .await;
    tokio::time::sleep(past_the_transition(state::ARCHIVE_TRANSITION)).await;

    publish(&router, "before the update", "ingest-bus", "com.demo.orders").await;
    assert_eq!(
        event_count(&router, "before the update", "ingest-archive").await,
        1,
        "the archive captures what its own pattern matches"
    );

    let (status, body) = events(
        &router,
        "UpdateArchive",
        &json!({
            "ArchiveName": "ingest-archive",
            "EventPattern": json!({ "source": ["com.demo.invoices"] }).to_string(),
        }),
    )
    .await;
    assert_eq!(status, 200, "UpdateArchive answered {status} {body}");
    tokio::time::sleep(past_the_transition(state::ARCHIVE_TRANSITION)).await;

    // (case, the source published, how many events the archive holds afterwards)
    let rows: [(&str, &str, i64); 4] = [
        ("the source the OLD pattern matched", "com.demo.orders", 1),
        ("the source the NEW pattern matches", "com.demo.invoices", 2),
        ("a source neither pattern matches", "com.demo.other", 2),
        (
            "the new source once more, which is captured again",
            "com.demo.invoices",
            3,
        ),
    ];
    for (case, source, expected) in rows {
        publish(&router, case, "ingest-bus", source).await;
        assert_eq!(
            event_count(&router, case, "ingest-archive").await,
            expected,
            "case {case:?}: expected the archive to hold {expected} events"
        );
    }
}

/// What `UpdateArchive` refuses.
#[tokio::test]
async fn update_archive_refuses_a_missing_archive_and_a_pattern_nothing_can_apply() {
    let router = emulator();
    let bus = given_bus(&router, "refused-update-bus").await;
    let _ = given_archive(&router, "present-archive", &bus, &json!({})).await;

    // (case, the request, the expected wire code)
    let rows: [(&str, Value, &str); 4] = [
        (
            "a name this account holds no archive for",
            json!({ "ArchiveName": "absent-archive" }),
            "ResourceNotFoundException",
        ),
        (
            "a pattern that is not JSON at all",
            json!({ "ArchiveName": "present-archive", "EventPattern": "{not json" }),
            "InvalidEventPatternException",
        ),
        (
            "a pattern whose top level is not an object",
            json!({ "ArchiveName": "present-archive", "EventPattern": "[1, 2]" }),
            "InvalidEventPatternException",
        ),
        (
            "a pattern whose clause is not a list of matchers",
            json!({
                "ArchiveName": "present-archive",
                "EventPattern": json!({ "source": "com.demo.orders" }).to_string(),
            }),
            "InvalidEventPatternException",
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = events(&router, "UpdateArchive", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }

    // Every refused update leaves the archive exactly as it was.
    let described = describe_archive(&router, "after the refusals", "present-archive").await;
    assert!(
        described.get("EventPattern").is_none() || described["EventPattern"].is_null(),
        "a refused update stores nothing, got {described}"
    );
}

/// `DeleteArchive` removes one archive and the managed rule that fed it, frees
/// the name, and leaves every other archive alone.
#[tokio::test]
async fn delete_archive_removes_the_archive_and_the_rule_that_fed_it() {
    // (case, the name deleted, the members it was created with)
    let rows: [(&str, &str, Value); 3] = [
        ("an archive that filters nothing", "plain-archive", json!({})),
        (
            "an archive with a pattern of its own",
            "filtered-archive",
            json!({ "EventPattern": json!({ "source": ["com.demo.orders"] }).to_string() }),
        ),
        (
            "an archive with a retention and a description",
            "retained-archive",
            json!({ "RetentionDays": 14, "Description": "kept for a fortnight" }),
        ),
    ];

    for (case, deleted, extra) in rows {
        let router = emulator();
        let bus = given_bus(&router, "delete-bus").await;
        let _ = given_archive(&router, deleted, &bus, &extra).await;
        // A second archive on the same bus, which the delete has to leave alone.
        let _ = given_archive(&router, "bystander-archive", &bus, &json!({})).await;
        tokio::time::sleep(past_the_transition(state::ARCHIVE_TRANSITION)).await;

        let (status, body) = events(&router, "DeleteArchive", &json!({ "ArchiveName": deleted })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteArchive answered {status} {body}"
        );

        // It is gone from the read, from the listing, and the rule that fed it is
        // gone from its bus.
        let (status, read) = events(&router, "DescribeArchive", &json!({ "ArchiveName": deleted })).await;
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case {case:?}: a deleted archive is not found, got {status} {read}"
        );
        let (status, listed) = events(&router, "ListArchives", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListArchives answered {status} {listed}"
        );
        assert_eq!(
            archive_names(&listed),
            vec!["bystander-archive".to_string()],
            "case {case:?}: only the other archive is left, got {listed}"
        );
        let (status, rules) = events(
            &router,
            "ListRules",
            &json!({ "EventBusName": "delete-bus" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListRules answered {status} {rules}"
        );
        let remaining: Vec<String> = rules["Rules"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|rule| rule["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            remaining,
            vec!["Events-Archive-bystander-archive".to_string()],
            "case {case:?}: the deleted archive's managed rule went with it, got {rules}"
        );

        // The name is free again, and a second delete of the same name is the
        // missing resource this operation models.
        let _ = given_archive(&router, deleted, &bus, &json!({})).await;
        let (status, body) = events(&router, "DeleteArchive", &json!({ "ArchiveName": deleted })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-created archive deletes too, got {status} {body}"
        );
        let (status, body) = events(&router, "DeleteArchive", &json!({ "ArchiveName": deleted })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second delete is refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// An archive deleted while a replay of it is in flight is still deleted — the
/// replay re-publishes what it selected when it started — and a replay STARTED
/// afterwards is refused, because its event source resolves to nothing.
#[tokio::test]
async fn delete_archive_is_not_blocked_by_a_replay_and_frees_its_bus() {
    let router = emulator();
    let bus = given_bus(&router, "replayed-bus").await;
    let archive = given_archive(&router, "replayed-archive", &bus, &json!({})).await;
    tokio::time::sleep(past_the_transition(state::ARCHIVE_TRANSITION)).await;
    publish(&router, "the archived event", "replayed-bus", "com.demo.orders").await;

    let now = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now()).secs();
    let (status, body) = events(
        &router,
        "StartReplay",
        &json!({
            "ReplayName": "in-flight",
            "EventSourceArn": archive,
            "EventStartTime": now - 600,
            "EventEndTime": now + 600,
            "Destination": { "Arn": bus },
        }),
    )
    .await;
    assert_eq!(status, 200, "StartReplay answered {status} {body}");

    let (status, body) = events(
        &router,
        "DeleteArchive",
        &json!({ "ArchiveName": "replayed-archive" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a replay in flight does not block the delete, got {status} {body}"
    );
    // The replay itself is still readable: it is a resource of its own.
    let (status, replay) = events(&router, "DescribeReplay", &json!({ "ReplayName": "in-flight" })).await;
    assert_eq!(status, 200, "DescribeReplay answered {status} {replay}");

    // A replay started on the archive now resolves to no archive at all.
    let (status, body) = events(
        &router,
        "StartReplay",
        &json!({
            "ReplayName": "too-late",
            "EventSourceArn": archive,
            "EventStartTime": now - 600,
            "EventEndTime": now + 600,
            "Destination": { "Arn": bus },
        }),
    )
    .await;
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "a replay of a deleted archive is refused, got {status} {body}"
    );

    // And with no archive sending its events to the bus, the bus itself can go.
    let (status, body) = events(
        &router,
        "DeleteEventBus",
        &json!({ "Name": "replayed-bus" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the bus the deleted archive fed can be deleted, got {status} {body}"
    );
}

/// A replay never leaves the event bus its events were archived from: a destination
/// that names another bus, and a filter that names a rule of another bus, are each
/// refused with AWS's sentence for that member — the WORDING is the requirement, not
/// only the code.
#[tokio::test]
async fn start_replay_refuses_leaving_the_event_bus_the_events_were_archived_from() {
    let router = emulator();
    let source = given_bus(&router, "source-bus").await;
    let other = given_bus(&router, "other-bus").await;
    let archive = given_archive(&router, "crossing-archive", &source, &json!({})).await;
    tokio::time::sleep(past_the_transition(state::ARCHIVE_TRANSITION)).await;

    // A rule on the OTHER bus, so a filter has a real rule of the wrong bus to name.
    let (status, body) = events(
        &router,
        "PutRule",
        &json!({
            "Name": "other-rule",
            "EventBusName": "other-bus",
            "EventPattern": json!({ "source": ["com.demo.orders"] }).to_string(),
        }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");
    let other_rule = body["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no RuleArn in {body}"))
        .to_string();
    let absent_rule = format!(
        "arn:aws:events:{REGION}:{}:rule/source-bus/nobody-put-this",
        engine::emulator_account_id()
    );

    let now = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now()).secs();
    // (case, the Destination, the sentence it is refused with)
    let rows: [(&str, Value, &str); 3] = [
        (
            "a destination that is not the bus the events were archived from",
            json!({ "Arn": other }),
            "Parameter Destination.Arn is not valid. Reason: Cross event bus replay is not \
             permitted.",
        ),
        (
            "a filter naming a rule of another bus",
            json!({ "Arn": source, "FilterArns": [other_rule] }),
            "Parameter Destination.FilterArns is not valid. Reason: Rule must be on the \
             destination event bus.",
        ),
        (
            "a filter naming no rule at all",
            json!({ "Arn": source, "FilterArns": [absent_rule] }),
            "Parameter Destination.FilterArns is not valid. Reason: Rule must be on the \
             destination event bus.",
        ),
    ];

    for (index, (case, destination, expected)) in rows.into_iter().enumerate() {
        let (status, body) = events(
            &router,
            "StartReplay",
            &json!({
                "ReplayName": format!("crossing-{index}"),
                "EventSourceArn": archive,
                "EventStartTime": now - 600,
                "EventEndTime": now + 600,
                "Destination": destination,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected the message {expected:?}, got {body}"
        );
    }
}

/// `ListApiDestinations` reports the stored destinations with the state their
/// connection gives them, and both of its members narrow the listing.
#[tokio::test]
async fn list_api_destinations_reports_the_stored_destinations_and_both_members_narrow() {
    let router = emulator();
    // Two connections, so a `ConnectionArn` filter has something to discriminate.
    let mut connections = Vec::new();
    for name in ["first-connection", "second-connection"] {
        let (status, body) = events(
            &router,
            "CreateConnection",
            &json!({
                "Name": name,
                "AuthorizationType": "API_KEY",
                "AuthParameters": {
                    "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit" },
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnection answered {status} {body}"
        );
        connections.push(
            body["ConnectionArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {name:?}: no ConnectionArn in {body}"))
                .to_string(),
        );
    }
    tokio::time::sleep(past_the_transition(state::CONNECTION_TRANSITION)).await;

    // (the destination's name, which connection it invokes through, its method)
    let destinations: [(&str, usize, &str); 3] = [
        ("ord-create", 0, "POST"),
        ("ord-cancel", 0, "DELETE"),
        ("inv-fetch", 1, "GET"),
    ];
    for (name, connection, method) in destinations {
        let (status, body) = events(
            &router,
            "CreateApiDestination",
            &json!({
                "Name": name,
                "ConnectionArn": connections[connection],
                "InvocationEndpoint": format!("https://api.example.com/{name}"),
                "HttpMethod": method,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateApiDestination answered {status} {body}"
        );
    }

    // (case, the request, the names it has to report in order)
    let rows: [(&str, Value, Vec<&str>); 6] = [
        (
            "no filter at all",
            json!({}),
            vec!["inv-fetch", "ord-cancel", "ord-create"],
        ),
        (
            "a NamePrefix two of the three names start with",
            json!({ "NamePrefix": "ord" }),
            vec!["ord-cancel", "ord-create"],
        ),
        (
            "a NamePrefix no name starts with",
            json!({ "NamePrefix": "zzz" }),
            vec![],
        ),
        (
            "the ConnectionArn two destinations invoke through",
            json!({ "ConnectionArn": connections[0] }),
            vec!["ord-cancel", "ord-create"],
        ),
        (
            "a NamePrefix and a ConnectionArn that narrow together",
            json!({ "NamePrefix": "ord-c", "ConnectionArn": connections[0] }),
            vec!["ord-cancel", "ord-create"],
        ),
        (
            "a NamePrefix and a ConnectionArn no destination satisfies both of",
            json!({ "NamePrefix": "ord", "ConnectionArn": connections[1] }),
            vec![],
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = events(&router, "ListApiDestinations", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListApiDestinations answered {status} {body}"
        );
        assert_eq!(
            destination_names(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // The summary is projected from the same record the read reports, state
    // included.
    let (status, body) = events(
        &router,
        "ListApiDestinations",
        &json!({ "NamePrefix": "inv-fetch" }),
    )
    .await;
    assert_eq!(status, 200, "the summary listing answered {status} {body}");
    let summary = body["ApiDestinations"][0].clone();
    let (status, described) = events(
        &router,
        "DescribeApiDestination",
        &json!({ "Name": "inv-fetch" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeApiDestination answered {status} {described}"
    );
    for member in [
        "ApiDestinationArn",
        "Name",
        "ApiDestinationState",
        "ConnectionArn",
        "InvocationEndpoint",
        "HttpMethod",
        "InvocationRateLimitPerSecond",
        "CreationTime",
        "LastModifiedTime",
    ] {
        assert_eq!(
            summary[member], described[member],
            "case {member:?}: the listing and the read have to agree, got {summary} and {described}"
        );
    }
    assert_eq!(
        summary["ApiDestinationState"],
        json!(state::API_DESTINATION_STATE_ACTIVE),
        "a destination on an authorized connection is {}, got {summary}",
        state::API_DESTINATION_STATE_ACTIVE
    );

    // `Limit` bounds the page and the token it answers resumes it.
    let (status, first) = events(&router, "ListApiDestinations", &json!({ "Limit": 2 })).await;
    assert_eq!(
        status, 200,
        "the first page answered {status} {first}"
    );
    assert_eq!(
        destination_names(&first),
        vec!["inv-fetch", "ord-cancel"],
        "a Limit of 2 is the first two destinations, got {first}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page with more to come carries a token, got {first}"))
        .to_string();
    let (status, second) = events(
        &router,
        "ListApiDestinations",
        &json!({ "Limit": 2, "NextToken": token }),
    )
    .await;
    assert_eq!(status, 200, "the second page answered {status} {second}");
    assert_eq!(
        destination_names(&second),
        vec!["ord-create"],
        "the second page is the rest of the listing, got {second}"
    );
    assert!(
        second["NextToken"].is_null(),
        "the last page carries no token, got {second}"
    );
}
