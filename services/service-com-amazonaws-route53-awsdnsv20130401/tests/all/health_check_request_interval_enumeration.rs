// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of ONE rule: a request element whose accepted values the
//! service's request schema ENUMERATES is refused for any other value, by the
//! schema check that runs while the request body is parsed.
//!
//! The rule under test: `HealthCheckConfig$RequestInterval` is modelled `@range
//! min 10 max 30`
//! (smithy_query(query="shape",shape="HealthCheckConfig")), but the service
//! accepts only the two values its request schema enumerates — the standard
//! cadence and the fast one — so an in-range value between them is not a
//! configuration the service will store. It is refused with the modelled
//! `InvalidInput` (HTTP 400) whose message is the schema violation itself: the
//! offending value, the enumeration it is not in, and the position in the
//! request body where that value ends.
//!
//! One rejected interval is ONE row of that class, so the tables below span it:
//! the accepted values (stored, and read back), values inside the modelled range
//! but outside the enumeration, values outside the range on both sides, and the
//! same element at different POSITIONS in the body — the position is part of the
//! refusal, so a message built from a fixed offset would fail those rows.
//!
//! Every test drives the service through the real router, using the same harness
//! the other health-check suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use crate::health_checks::{create_health_check, get_health_check, health_check_count, health_check_id};
use crate::hosted_zone_records::{router, Reply};

/// The two values the service's request schema enumerates for
/// `RequestInterval`, which are the two probe cadences a health check is
/// configured with: thirty seconds, which is also the value the service fills in
/// for a request that names none
/// (smithy_query(query="doc",shape="HealthCheckConfig",member="RequestInterval")),
/// and ten.
const ENUMERATED_INTERVALS: &[i32] = &[10, 30];

/// The enumeration as the schema violation names it.
const ENUMERATION: &str = "[10, 30]";

/// A health-check configuration carrying `interval`, with the members before and
/// after it spelled out so the element sits in the middle of the body.
fn config_with(interval: i32) -> String {
    format!(
        "<Type>HTTP</Type><IPAddress>192.0.2.70</IPAddress><Port>80</Port>\
         <ResourcePath>/health</ResourcePath><RequestInterval>{interval}</RequestInterval>\
         <FailureThreshold>3</FailureThreshold>"
    )
}

/// The `Error/Message` of a refusal.
fn message_of(reply: &Reply, case: &str) -> String {
    reply
        .first("Error/Message")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the refusal must carry a message; body {}",
                reply.body
            )
        })
        .to_string()
}

#[tokio::test]
async fn an_interval_outside_the_enumeration_is_refused() {
    // The rows differ along WHERE the value sits relative to the enumeration and
    // the modelled range: just inside the range above the low value, the middle,
    // just below the high value, one below the range, one above it, and zero.
    let rows = [9, 11, 15, 20, 29, 31, 0];

    let router = router();
    let population = health_check_count(&router, "before the refusals").await;

    for (index, interval) in rows.iter().enumerate() {
        let case = format!("RequestInterval {interval}");
        let reply = create_health_check(
            &router,
            &format!("interval-refused-{index}"),
            &config_with(*interval),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidInput",
            "case {case:?}: expected the InvalidInput code, got body {}",
            reply.body
        );
        let got = message_of(&reply, &case);
        assert!(
            got.contains(&format!("Value '{interval}' is not facet-valid")),
            "case {case:?}: the violation must name the value the request sent, got {got:?}"
        );
        assert!(
            got.contains(&format!("enumeration '{ENUMERATION}'")),
            "case {case:?}: the violation must name the enumeration, got {got:?}"
        );
        assert!(
            got.contains("cvc-enumeration-valid")
                && got.contains("It must be a value from the enumeration."),
            "case {case:?}: the violation must state the schema rule, got {got:?}"
        );
        assert!(
            reply.first("HealthCheck/Id").is_none(),
            "case {case:?}: a refused create answers no health check; body {}",
            reply.body
        );
    }

    // Nothing was created by any of them.
    let counted = health_check_count(&router, "after the refusals").await;
    assert_eq!(
        counted, population,
        "a refused create must leave the account's health checks as they were, \
         GetHealthCheckCount reports {counted}"
    );
}

#[tokio::test]
async fn the_violation_names_the_position_the_value_ends_at() {
    // The position is read off the REQUEST, so the rows move the element: the
    // same interval is sent with a short caller reference and with a long one,
    // which shifts the element later in the body, and the reported column has to
    // move with it.
    let router = router();
    let mut reported = Vec::new();
    for (index, reference) in ["r", &"r".repeat(40)].iter().enumerate() {
        let case = format!("caller reference of {} character(s)", reference.len());
        let reply = create_health_check(&router, &format!("{reference}{index}"), &config_with(15))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        let got = message_of(&reply, &case);
        assert!(
            got.contains("lineNumber: 1; columnNumber: "),
            "case {case:?}: the violation must name the position in the body, got {got:?}"
        );
        let column: i64 = got
            .split("columnNumber: ")
            .nth(1)
            .and_then(|tail| tail.split(';').next())
            .and_then(|digits| digits.trim().parse().ok())
            .unwrap_or_else(|| panic!("case {case:?}: the column must be a number, got {got:?}"));
        reported.push((case, column));
    }
    let (first_case, first) = &reported[0];
    let (second_case, second) = &reported[1];
    assert!(
        second > first,
        "case {second_case:?}: a body whose element sits later must report a later column than \
         case {first_case:?} ({first}), got {second}"
    );
}

#[tokio::test]
async fn an_enumerated_interval_is_stored_and_read_back() {
    // The positive control, over the whole enumeration: each accepted value is
    // created and read back through GetHealthCheck, so the refusal above is the
    // schema's and not a blanket one.
    let router = router();
    for (index, interval) in ENUMERATED_INTERVALS.iter().enumerate() {
        let case = format!("RequestInterval {interval}");
        let created = create_health_check(
            &router,
            &format!("interval-accepted-{index}"),
            &config_with(*interval),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let id = health_check_id(&created, &case);
        let read = get_health_check(&router, &id).await;
        assert_eq!(
            read.first("HealthCheckConfig/RequestInterval"),
            Some(interval.to_string().as_str()),
            "case {case:?}: the health check must keep the interval it was created with; body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_interval_is_served_the_default() {
    // The element the request leaves out is not an element the schema refuses:
    // the service fills in its documented default, which is one of the two
    // enumerated values.
    let router = router();
    let created = create_health_check(
        &router,
        "interval-absent",
        "<Type>HTTP</Type><IPAddress>192.0.2.71</IPAddress><Port>80</Port>",
    )
    .await;
    assert_eq!(
        created.status, 201,
        "expected HTTP 201 for a request naming no interval, got {} with body {}",
        created.status, created.body
    );
    let filled = created.first("HealthCheckConfig/RequestInterval").unwrap_or("");
    assert!(
        ENUMERATED_INTERVALS
            .iter()
            .any(|interval| interval.to_string() == filled),
        "the interval the service fills in must be one it accepts, got {filled:?}; body {}",
        created.body
    );
}
