// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the ONE instant rendering every time-ranged read states a refused
//! period with: `EEE MMM dd HH:mm:ss UTC yyyy`, in UTC.
//!
//! The rule under test: a read whose requested period does not run forwards is refused with
//! `ValidationException` (HTTP 400) naming both instants in that single form, and the form is
//! the SAME for every such read and for whichever documented wire form the instant arrived
//! in — the epoch-seconds number a JSON body carries or the RFC-3339 `date-time` a query
//! string carries.
//!
//! The rows span what the rendering has to get right: every month abbreviation (twelve of
//! them, paired two per row), every weekday name, a leap day, midnight and a non-midnight
//! time of day, instants before the epoch, and an instant carrying a sub-second part the
//! whole-second form drops. The month names, their order and the weekday of a given date are
//! the calendar's, not this crate's: each row's expected string is the civil date of the
//! epoch second it names.
//!
//! The operations are driven in pairs so BOTH bindings of the period are covered: `GetService`
//! and `ListServices` take it in the query string, `ListServiceStates` and `ListEntityEvents`
//! in the JSON body.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification
/// is off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("serialize the request body")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One instant: the epoch seconds a JSON body carries it as, the RFC-3339 `date-time` a
/// query string carries it as, and the form this service states it in.
struct Instant {
    seconds: f64,
    date_time: &'static str,
    stated: &'static str,
}

/// One refused period: a `start` that is not before its `end`.
struct Case {
    case: &'static str,
    start: Instant,
    end: Instant,
}

/// Every refused period this suite drives, each pinning two renderings.
///
/// The paired months walk the whole twelve-entry abbreviation list in order, on the first of
/// each month of 2026 (`1788660000` is `2026-09-06T02:00:00Z`, the instant the service's own
/// refusal names, so the row stepping is anchored on a known second); the remaining rows add
/// a leap day, a non-midnight time of day, two instants before the epoch and a sub-second
/// part.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "February back to January",
            start: Instant {
                seconds: 1_769_904_000.0,
                date_time: "2026-02-01T00:00:00Z",
                stated: "Sun Feb 01 00:00:00 UTC 2026",
            },
            end: Instant {
                seconds: 1_767_225_600.0,
                date_time: "2026-01-01T00:00:00Z",
                stated: "Thu Jan 01 00:00:00 UTC 2026",
            },
        },
        Case {
            case: "April back to March",
            start: Instant {
                seconds: 1_775_001_600.0,
                date_time: "2026-04-01T00:00:00Z",
                stated: "Wed Apr 01 00:00:00 UTC 2026",
            },
            end: Instant {
                seconds: 1_772_323_200.0,
                date_time: "2026-03-01T00:00:00Z",
                stated: "Sun Mar 01 00:00:00 UTC 2026",
            },
        },
        Case {
            case: "June back to May",
            start: Instant {
                seconds: 1_780_272_000.0,
                date_time: "2026-06-01T00:00:00Z",
                stated: "Mon Jun 01 00:00:00 UTC 2026",
            },
            end: Instant {
                seconds: 1_777_593_600.0,
                date_time: "2026-05-01T00:00:00Z",
                stated: "Fri May 01 00:00:00 UTC 2026",
            },
        },
        Case {
            case: "August back to July",
            start: Instant {
                seconds: 1_785_542_400.0,
                date_time: "2026-08-01T00:00:00Z",
                stated: "Sat Aug 01 00:00:00 UTC 2026",
            },
            end: Instant {
                seconds: 1_782_864_000.0,
                date_time: "2026-07-01T00:00:00Z",
                stated: "Wed Jul 01 00:00:00 UTC 2026",
            },
        },
        Case {
            case: "October back to September",
            start: Instant {
                seconds: 1_790_812_800.0,
                date_time: "2026-10-01T00:00:00Z",
                stated: "Thu Oct 01 00:00:00 UTC 2026",
            },
            end: Instant {
                seconds: 1_788_220_800.0,
                date_time: "2026-09-01T00:00:00Z",
                stated: "Tue Sep 01 00:00:00 UTC 2026",
            },
        },
        Case {
            case: "December back to November",
            start: Instant {
                seconds: 1_796_083_200.0,
                date_time: "2026-12-01T00:00:00Z",
                stated: "Tue Dec 01 00:00:00 UTC 2026",
            },
            end: Instant {
                seconds: 1_793_491_200.0,
                date_time: "2026-11-01T00:00:00Z",
                stated: "Sun Nov 01 00:00:00 UTC 2026",
            },
        },
        Case {
            case: "a leap day at a stated time of day",
            start: Instant {
                seconds: 1_709_210_096.0,
                date_time: "2024-02-29T12:34:56Z",
                stated: "Thu Feb 29 12:34:56 UTC 2024",
            },
            end: Instant {
                seconds: 1_709_111_287.0,
                date_time: "2024-02-28T09:08:07Z",
                stated: "Wed Feb 28 09:08:07 UTC 2024",
            },
        },
        Case {
            case: "the epoch second back to the second before it",
            start: Instant {
                seconds: 0.0,
                date_time: "1970-01-01T00:00:00Z",
                stated: "Thu Jan 01 00:00:00 UTC 1970",
            },
            end: Instant {
                seconds: -1.0,
                date_time: "1969-12-31T23:59:59Z",
                stated: "Wed Dec 31 23:59:59 UTC 1969",
            },
        },
        Case {
            case: "both bounds before the epoch",
            start: Instant {
                seconds: -14_182_940.0,
                date_time: "1969-07-20T20:17:40Z",
                stated: "Sun Jul 20 20:17:40 UTC 1969",
            },
            end: Instant {
                seconds: -14_428_800.0,
                date_time: "1969-07-18T00:00:00Z",
                stated: "Fri Jul 18 00:00:00 UTC 1969",
            },
        },
        Case {
            case: "a sub-second part the stated form drops",
            start: Instant {
                seconds: 1_788_660_000.75,
                date_time: "2026-09-06T02:00:00.75Z",
                stated: "Sun Sep 06 02:00:00 UTC 2026",
            },
            end: Instant {
                seconds: 1_788_660_000.25,
                date_time: "2026-09-06T02:00:00.25Z",
                stated: "Sun Sep 06 02:00:00 UTC 2026",
            },
        },
    ]
}

/// The refusal a case earns, with both instants stated.
fn expected_message(case: &Case) -> String {
    format!(
        "Invalid start time {} and end time {} provided: start time must be before end time",
        case.start.stated, case.end.stated
    )
}

/// One instant as a query-string value: the RFC-3339 form with its colons percent-encoded.
fn query_value(instant: &Instant) -> String {
    instant.date_time.replace(':', "%3A")
}

/// The key attributes of one Application Signals service entity.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": "prod" })
}

/// Every refused period is stated in the one rendering, by the reads that bind the period to
/// the QUERY STRING — so the instants cross the wire in the RFC-3339 `date-time` form.
#[tokio::test]
async fn a_query_bound_period_that_does_not_run_forwards_is_stated_in_one_rendering() {
    let router = router();
    let entity = service_entity("checkout");
    for refused in cases() {
        let message = expected_message(&refused);
        let window = format!(
            "StartTime={}&EndTime={}",
            query_value(&refused.start),
            query_value(&refused.end)
        );
        let calls = [
            ("GetService", "POST", format!("/service?{window}"), true),
            ("ListServices", "GET", format!("/services?{window}"), false),
        ];
        for (operation, method, uri, needs_entity) in calls {
            let case = format!("{operation} with {}", refused.case);
            let body = needs_entity.then(|| json!({ "KeyAttributes": entity }));
            let (status, answer) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
            );
            assert_eq!(
                answer["message"].as_str(),
                Some(message.as_str()),
                "case {case:?}: expected {message:?}, got {:?}",
                answer["message"]
            );
        }
    }
}

/// The same rendering for the same instants, by the reads that bind the period to the JSON
/// BODY — so the instants cross the wire as epoch seconds instead.
#[tokio::test]
async fn a_body_bound_period_that_does_not_run_forwards_is_stated_in_one_rendering() {
    let router = router();
    let entity = service_entity("checkout");
    for refused in cases() {
        let message = expected_message(&refused);
        let window = json!({ "StartTime": refused.start.seconds, "EndTime": refused.end.seconds });
        let mut events_body = window.clone();
        events_body["Entity"] = entity.clone();
        let calls = [
            ("ListServiceStates", "/service/states", window.clone()),
            ("ListEntityEvents", "/events", events_body),
        ];
        for (operation, path, body) in calls {
            let case = format!("{operation} with {}", refused.case);
            let (status, answer) = call(&router, "POST", path, Some(body)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {answer:?}"
            );
            assert_eq!(
                answer["message"].as_str(),
                Some(message.as_str()),
                "case {case:?}: expected {message:?}, got {:?}",
                answer["message"]
            );
        }
    }
}

/// A period that DOES run forwards is served, whatever calendar date its bounds fall on: the
/// rendering is reached only by a refusal, so the same instants in the right order are
/// answered with the window they named rather than with a stated refusal.
#[tokio::test]
async fn a_period_that_runs_forwards_is_served_for_the_same_instants() {
    let router = router();
    let entity = service_entity("checkout");
    for served in cases() {
        // The bounds are swapped, so the period now runs forwards — except for the row whose
        // bounds share a second, where only the sub-second part orders them.
        let case = format!("GetService with {} reversed", served.case);
        let uri = format!(
            "/service?StartTime={}&EndTime={}",
            query_value(&served.end),
            query_value(&served.start)
        );
        let (status, answer) = call(
            &router,
            "POST",
            &uri,
            Some(json!({ "KeyAttributes": entity })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["StartTime"].as_f64(),
            Some(served.end.seconds),
            "case {case:?}: expected the requested StartTime back, got {:?}",
            answer["StartTime"]
        );
        assert_eq!(
            answer["EndTime"].as_f64(),
            Some(served.start.seconds),
            "case {case:?}: expected the requested EndTime back, got {:?}",
            answer["EndTime"]
        );
    }
}
