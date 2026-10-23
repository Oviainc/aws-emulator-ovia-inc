// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the BODY of CloudWatch RUM's two app-monitor existence
//! refusals: `ResourceNotFoundException` (404) and `ConflictException` (409).
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, the `@httpLabel` the monitor is named by, the modelled
//! constraints, the handler and the error envelope are all exercised, not just a handler
//! body.
//!
//! The assertions pin two RULES over a SET of inputs rather than one request:
//!
//! * a not-found message is the literal `Resource not found ` followed by the requested
//!   name verbatim, with no trailing period, and the body carries NOTHING else — neither
//!   `resourceName` nor `resourceType`, both of which the model declares and RUM leaves
//!   off the wire;
//! * a duplicate-name conflict message is the literal `Resource `, the requested name
//!   verbatim and the literal ` already exists`, again with no trailing period, and that
//!   body DOES report `resourceName` (the requested name) and `resourceType`
//!   (`RUM application`).
//!
//! The not-found table is the cross product of every operation that resolves an app
//! monitor by name with a name catalog that walks `AppMonitorName`'s own constraints, so
//! neither rule is tied to one operation or to one spelling.

use serde_json::{json, Value};

use crate::support::{call, create, emulator, get_app_monitor, message, rum_events_body};

/// The literal a not-found message starts with, ahead of the requested name.
/// Source: `crate::state`'s `NOT_FOUND_MESSAGE_PREFIX`, which the recorded corpus pins
/// across nine clusters and many operations.
const NOT_FOUND_PREFIX: &str = "Resource not found ";

/// The literals a duplicate-name conflict message brackets the requested name with.
const CONFLICT_PREFIX: &str = "Resource ";
const CONFLICT_SUFFIX: &str = " already exists";

/// The `resourceType` a conflict reports: RUM's own product noun, not the model's shape
/// or resource name.
const CONFLICT_RESOURCE_TYPE: &str = "RUM application";

/// A well-formed IAM JSON policy, so `PutResourcePolicy` reaches its app-monitor
/// resolution rather than being refused on the document.
fn policy_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
            "Action": "rum:PutRumEvents",
            "Resource": "*"
        }]
    })
    .to_string()
}

/// Names that are all valid `AppMonitorName`s and all name nothing.
///
/// Source: `smithy_query(query="shape", shape="AppMonitorName")` — `@length(min: 1, max:
/// 255)` and `@pattern ^(?!\.)[\.\-_#A-Za-z0-9]+$`. The rows walk both published length
/// bounds and every class of character the pattern admits: letters in both cases, digits,
/// `.`, `-`, `_` and `#`. `#` cannot travel unencoded in a URI path, so that row sends it
/// percent-encoded and expects the DECODED name back in the message — which is what makes
/// the row worth having: the message is built from the deserialized member, not from the
/// raw path.
fn absent_names() -> Vec<(&'static str, String, String)> {
    vec![
        // (case, the name as the message must report it, the name as the path carries it)
        ("shortest-admissible", "a".to_string(), "a".to_string()),
        ("longest-admissible", "x".repeat(255), "x".repeat(255)),
        (
            "dots-inside",
            "absent.monitor.name".to_string(),
            "absent.monitor.name".to_string(),
        ),
        (
            "dash-underscore-digits",
            "Absent-Monitor_01234567890".to_string(),
            "Absent-Monitor_01234567890".to_string(),
        ),
        (
            "mixed-case",
            "MiXeDcAsE".to_string(),
            "MiXeDcAsE".to_string(),
        ),
        (
            "digits-only",
            "9876543210".to_string(),
            "9876543210".to_string(),
        ),
        (
            "hash-percent-encoded",
            "absent#monitor".to_string(),
            "absent%23monitor".to_string(),
        ),
    ]
}

/// Every operation that resolves an app monitor by its name, as
/// (case, method, path built from the encoded name, request body).
type Row = (&'static str, &'static str, String, Option<Value>);

fn operations_resolving_by_name(encoded: &str) -> Vec<Row> {
    vec![
        (
            "GetAppMonitor",
            "GET",
            format!("/appmonitor/{encoded}"),
            None,
        ),
        (
            "UpdateAppMonitor",
            "PATCH",
            format!("/appmonitor/{encoded}"),
            Some(json!({"Domain": "other.example.com"})),
        ),
        (
            "DeleteAppMonitor",
            "DELETE",
            format!("/appmonitor/{encoded}"),
            None,
        ),
        (
            "GetResourcePolicy",
            "GET",
            format!("/appmonitor/{encoded}/policy"),
            None,
        ),
        (
            "PutResourcePolicy",
            "PUT",
            format!("/appmonitor/{encoded}/policy"),
            Some(json!({"PolicyDocument": policy_document()})),
        ),
        (
            "DeleteResourcePolicy",
            "DELETE",
            format!("/appmonitor/{encoded}/policy"),
            None,
        ),
        (
            "PutRumMetricsDestination",
            "POST",
            format!("/rummetrics/{encoded}/metricsdestination"),
            Some(json!({"Destination": "CloudWatch"})),
        ),
        (
            "ListRumMetricsDestinations",
            "GET",
            format!("/rummetrics/{encoded}/metricsdestination"),
            None,
        ),
    ]
}

#[tokio::test]
async fn every_not_found_body_is_the_request_derived_message_and_nothing_else() {
    let (router, _engine) = emulator();
    // One monitor exists throughout, so a 404 means "that name is not stored" rather
    // than "this emulator holds nothing".
    create(
        &router,
        "seed",
        json!({"Name": "present", "Domain": "example.com"}),
    )
    .await;

    for (name_case, name, encoded) in absent_names() {
        for (op, method, path, body) in operations_resolving_by_name(&encoded) {
            let case = format!("{op} / {name_case}");
            let reply = call(&router, method, &path, body).await;
            assert_eq!(
                reply.status,
                404,
                "case {case:?}: expected HTTP 404, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let code = reply.error_code(&case);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {code:?}"
            );
            let document = reply.json(&case);
            let expected = format!("{NOT_FOUND_PREFIX}{name}");
            assert_eq!(
                message(&document),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {:?}",
                message(&document)
            );
            assert_eq!(
                document["resourceName"],
                Value::Null,
                "case {case:?}: a RUM not-found reports no resourceName, got {:?}",
                document["resourceName"]
            );
            assert_eq!(
                document["resourceType"],
                Value::Null,
                "case {case:?}: a RUM not-found reports no resourceType, got {:?}",
                document["resourceType"]
            );
        }
    }

    // None of those refused calls created, changed or removed anything: the one stored
    // monitor is still readable and still the only one.
    let monitor = get_app_monitor(&router, "seed survives", "present").await;
    assert_eq!(
        monitor["Domain"].as_str(),
        Some("example.com"),
        "case \"seed survives\": the stored monitor must be untouched, got {:?}",
        monitor["Domain"]
    );
}

#[tokio::test]
async fn a_duplicate_name_conflict_names_the_resource_it_collided_with() {
    let (router, _engine) = emulator();

    // The conflict rule generalises over the spelling of the name exactly as the
    // not-found rule does, so the same catalog drives it — each row created once and then
    // asked for a second time.
    for (name_case, name, _encoded) in absent_names() {
        let body = json!({"Name": name, "Domain": "example.com"});
        create(&router, name_case, body.clone()).await;

        let reply = call(&router, "POST", "/appmonitor", Some(body)).await;
        assert_eq!(
            reply.status,
            409,
            "case {name_case:?}: expected HTTP 409, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(name_case);
        assert_eq!(
            code, "ConflictException",
            "case {name_case:?}: expected ConflictException, got {code:?}"
        );
        let document = reply.json(name_case);
        let expected = format!("{CONFLICT_PREFIX}{name}{CONFLICT_SUFFIX}");
        assert_eq!(
            message(&document),
            Some(expected.as_str()),
            "case {name_case:?}: expected the message {expected:?}, got {:?}",
            message(&document)
        );
        assert_eq!(
            document["resourceName"].as_str(),
            Some(name.as_str()),
            "case {name_case:?}: the conflict must name the requested resource, got {:?}",
            document["resourceName"]
        );
        assert_eq!(
            document["resourceType"].as_str(),
            Some(CONFLICT_RESOURCE_TYPE),
            "case {name_case:?}: expected resourceType {CONFLICT_RESOURCE_TYPE:?}, got {:?}",
            document["resourceType"]
        );
    }
}

#[tokio::test]
async fn a_deleted_monitor_is_reported_the_same_way_as_one_never_created() {
    let (router, _engine) = emulator();
    let name = "deleted.monitor";
    create(
        &router,
        "create",
        json!({"Name": name, "Domain": "example.com"}),
    )
    .await;
    let deleted = call(&router, "DELETE", &format!("/appmonitor/{name}"), None).await;
    assert_eq!(
        deleted.status,
        200,
        "case \"delete\": expected DeleteAppMonitor to answer 200, got {} {:?}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    // The same rule, reached through the other way a name stops resolving: the message
    // names the requested name and the body carries nothing else.
    for (op, method, path, body) in operations_resolving_by_name(name) {
        let case = format!("{op} after delete");
        let reply = call(&router, method, &path, body).await;
        assert_eq!(
            reply.status,
            404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let document = reply.json(&case);
        let expected = format!("{NOT_FOUND_PREFIX}{name}");
        assert_eq!(
            message(&document),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {:?}",
            message(&document)
        );
        assert_eq!(
            document["resourceName"],
            Value::Null,
            "case {case:?}: a RUM not-found reports no resourceName, got {:?}",
            document["resourceName"]
        );
        assert_eq!(
            document["resourceType"],
            Value::Null,
            "case {case:?}: a RUM not-found reports no resourceType, got {:?}",
            document["resourceType"]
        );
    }
}

#[tokio::test]
async fn an_app_monitor_can_be_created_again_once_the_name_is_free() {
    // The conflict is about the name being TAKEN, not about it ever having been used, so
    // the same name is accepted again after a delete — which is what shows the 409 above
    // is driven by stored state rather than by the request's shape.
    let (router, _engine) = emulator();
    let name = "reused-name";
    let first = create(
        &router,
        "first",
        json!({"Name": name, "Domain": "first.example.com"}),
    )
    .await;
    let deleted = call(&router, "DELETE", &format!("/appmonitor/{name}"), None).await;
    assert_eq!(
        deleted.status,
        200,
        "case \"delete\": expected DeleteAppMonitor to answer 200, got {} {:?}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );
    let second = create(
        &router,
        "second",
        json!({"Name": name, "Domain": "second.example.com"}),
    )
    .await;
    assert_ne!(
        first, second,
        "case \"second\": the re-created monitor is a new resource, so it must carry a new \
         Id, got {second:?} twice"
    );
    let monitor = get_app_monitor(&router, "second", name).await;
    assert_eq!(
        monitor["Domain"].as_str(),
        Some("second.example.com"),
        "case \"second\": the re-created monitor reports the second request's Domain, got \
         {:?}",
        monitor["Domain"]
    );
}

#[tokio::test]
async fn put_rum_events_names_the_id_it_could_not_resolve() {
    // The other half of the rule: `PutRumEvents` is the one operation that names its
    // monitor by the minted `Id` rather than by `Name` (`POST /appmonitors/{Id}/`), so the
    // message must report THAT value — which is what shows the message is built from
    // whichever member identified the monitor, not from a `Name` member it does not have.
    let (router, _engine) = emulator();
    let live = create(
        &router,
        "seed",
        json!({"Name": "events-present", "Domain": "example.com"}),
    )
    .await;

    // `AppMonitorId`'s modelled pattern is a 36-character RFC 4122 v4 UUID (see
    // `support::is_modelled_app_monitor_id`), so every row is a well-formed id that was
    // never minted; the rows differ in which hex digits they carry and in letter case.
    let unresolvable = [
        ("all-zero", "00000000-0000-4000-8000-000000000000"),
        ("all-f-lowercase", "ffffffff-ffff-4fff-bfff-ffffffffffff"),
        ("mixed-case-hex", "DeAdBeEf-1234-4abc-89De-0123456789aB"),
        ("digits-only", "12345678-9012-4345-8678-901234567890"),
    ];
    for (case, id) in unresolvable {
        assert_ne!(
            id, live,
            "case {case:?}: the row must not be the id the seed minted"
        );
        let reply = call(
            &router,
            "POST",
            &format!("/appmonitors/{id}/"),
            Some(rum_events_body(id)),
        )
        .await;
        assert_eq!(
            reply.status,
            404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let code = reply.error_code(case);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?}"
        );
        let document = reply.json(case);
        let expected = format!("{NOT_FOUND_PREFIX}{id}");
        assert_eq!(
            message(&document),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {:?}",
            message(&document)
        );
        assert_eq!(
            document["resourceName"],
            Value::Null,
            "case {case:?}: a RUM not-found reports no resourceName, got {:?}",
            document["resourceName"]
        );
        assert_eq!(
            document["resourceType"],
            Value::Null,
            "case {case:?}: a RUM not-found reports no resourceType, got {:?}",
            document["resourceType"]
        );
    }
}
