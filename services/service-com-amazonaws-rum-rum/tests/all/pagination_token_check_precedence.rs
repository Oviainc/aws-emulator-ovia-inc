// SPDX-License-Identifier: CC0-1.0

//! Where a paginated reader of this service judges its `NextToken`: before the resource
//! the request addresses is resolved, and before the caller's permission is decided.
//!
//! The rule under test is the crate's one precedence rule, applied to the member check
//! every paginated reader makes: a refusal that is about the request's own members — a
//! check that stands in for a constraint the framework's boundary walk cannot apply, and
//! that reads neither the store nor the caller's identity — is answered BEFORE the
//! not-found for the resource the request addresses and before the authorization decision,
//! because that walk runs in front of everything a handler judges. A continuation token
//! this service never issued is exactly such a check, so its modelled `ValidationException`
//! (HTTP 400) is reachable for a caller holding no grant and for an app monitor that does
//! not exist.
//!
//! The table spans what the rule generalises over: every paginated reader of this service,
//! the HTTP method, where the operation binds `NextToken` (the query string or the request
//! body), whether the request addresses one app monitor or the account's collection, and
//! the form of the value a client sends (not hex at all, hex of a string carrying no
//! marker, hex of a truncated marker, and a marker another listing issues). Each row also
//! carries the SAME request carrying no token, which pins the two refusals the token check
//! must not swallow: the not-found (404) and the access denial (403).
//!
//! Everything is driven through the real wire path and through each service's own API: the
//! app monitor is created with `CreateAppMonitor`, the unauthorized principal is an IAM
//! role created with IAM's `CreateRole` and assumed through STS's `AssumeRole`, and every
//! RUM call carries that credential's access key id in its SigV4 credential scope.

use serde_json::{json, Value};

use super::authorization::{Emulator, Reply, ACCOUNT_KEY};

/// The app monitor every monitor-addressed row addresses, which exists for all of them.
const EXISTING_MONITOR: &str = "token-check-precedence";

/// An app monitor name no case creates, so a request naming it addresses a resource that
/// exists in no region.
const ABSENT_MONITOR: &str = "no-such-app-monitor";

/// The destination kind `BatchGetRumMetricDefinitions` requires, which carries no
/// `DestinationArn`.
/// Source: `smithy_query(query="shape", shape="MetricDestination")` — an enum of
/// `CloudWatch` and `Evidently`.
const CLOUDWATCH: &str = "CloudWatch";

/// Values no listing of this service ever issued, in the forms a client might send one.
/// Each is a value the shared token codec rejects, so what refuses it is the handler's own
/// member check and nothing else:
/// * a value that is not hex at all;
/// * hex of a plain string, carrying no listing's marker ("not-a-token");
/// * hex of a truncated marker ("ListApp");
/// * hex of one listing's whole marker with no resume point behind it
///   ("ListAppMonitors/1/"), which for every OTHER listing is a marker belonging to a
///   different one.
const TOKENS_NO_LISTING_ISSUED: [&str; 4] = [
    "not-hex-at-all",
    "6e6f742d612d746f6b656e",
    "4c697374417070",
    "4c6973744170704d6f6e69746f72732f312f",
];

/// One paginated reader of this service, as a request carrying a `NextToken` and the same
/// request carrying none.
struct Listing {
    /// What this row is, named in every assertion.
    case: &'static str,
    /// The operation's HTTP method.
    method: &'static str,
    /// Whether the request addresses one app monitor, so a name no monitor holds is
    /// answered with the not-found; `false` for the listing over the account's collection,
    /// which addresses no single monitor.
    addresses_monitor: bool,
    /// This row's URI for an app monitor and, where the operation binds `NextToken` to the
    /// query string, a token.
    uri: fn(&str, Option<&str>) -> String,
    /// This row's body, carrying the token where the operation binds `NextToken` to the
    /// body.
    body: fn(Option<&str>) -> Option<Value>,
}

/// The body a reader binding `NextToken` to the query string sends: none.
fn no_body(_token: Option<&str>) -> Option<Value> {
    None
}

/// `nextToken=<token>` for a reader binding `NextToken` to the query string.
fn token_query(token: Option<&str>) -> String {
    match token {
        Some(token) => format!("&nextToken={token}"),
        None => String::new(),
    }
}

/// Every row of the table: one request per paginated reader of this service.
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            case: "ListAppMonitors, the account's collection, token in the query string",
            method: "POST",
            addresses_monitor: false,
            uri: |_monitor, token| format!("/appmonitors?maxResults=1{}", token_query(token)),
            body: no_body,
        },
        Listing {
            case: "ListRumMetricsDestinations, one app monitor, token in the query string",
            method: "GET",
            addresses_monitor: true,
            uri: |monitor, token| {
                format!(
                    "/rummetrics/{monitor}/metricsdestination?maxResults=1{}",
                    token_query(token)
                )
            },
            body: no_body,
        },
        Listing {
            case: "BatchGetRumMetricDefinitions, one app monitor, token in the query string",
            method: "GET",
            addresses_monitor: true,
            uri: |monitor, token| {
                format!(
                    "/rummetrics/{monitor}/metrics?destination={CLOUDWATCH}&maxResults=1{}",
                    token_query(token)
                )
            },
            body: no_body,
        },
        Listing {
            case: "GetAppMonitorData, one app monitor, token in the request body",
            method: "POST",
            addresses_monitor: true,
            uri: |monitor, _token| format!("/appmonitor/{monitor}/data"),
            body: |token| {
                Some(match token {
                    Some(token) => json!({"TimeRange": {"After": 0}, "NextToken": token}),
                    None => json!({"TimeRange": {"After": 0}}),
                })
            },
        },
    ]
}

/// Assert `reply` is the modelled refusal `code` with `status`.
fn assert_refused(reply: &Reply, case: &str, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let actual = reply.error_code(case);
    assert_eq!(
        actual, code,
        "case {case:?}: expected error code {code}, got {actual:?}"
    );
}

/// Assert `reply` refuses `token` as a token this service never issued, naming it.
fn assert_token_refused(reply: &Reply, case: &str, token: &str) {
    assert_refused(reply, case, 400, "ValidationException");
    let message = reply.message(case);
    assert!(
        message.contains(token),
        "case {case:?}: expected the refusal to name the token {token:?}, got {message:?}"
    );
}

/// A continuation token this service never issued is refused with the modelled
/// `ValidationException` even for a caller holding no grant for the operation, while the
/// same request carrying no token is answered with the access denial.
#[tokio::test]
async fn a_token_check_precedes_the_authorization_decision() {
    let emulator = Emulator::new();
    emulator.create_monitor(EXISTING_MONITOR).await;
    // A principal with no policy at all: every action of this service is refused for it,
    // so an answer that is not the token refusal is the denial.
    let unauthorized = emulator.assumed_role_key("holds-no-grant", None).await;

    for listing in listings() {
        for token in TOKENS_NO_LISTING_ISSUED {
            let case = format!("{} [{token}]", listing.case);
            let refused = emulator
                .rum(
                    &unauthorized,
                    listing.method,
                    &(listing.uri)(EXISTING_MONITOR, Some(token)),
                    (listing.body)(Some(token)),
                )
                .await;
            assert_token_refused(&refused, &case, token);
        }

        let control = format!("{} (carrying no token)", listing.case);
        let denied = emulator
            .rum(
                &unauthorized,
                listing.method,
                &(listing.uri)(EXISTING_MONITOR, None),
                (listing.body)(None),
            )
            .await;
        assert_refused(&denied, &control, 403, "AccessDeniedException");
    }
}

/// The same refusal precedes the resolution of the app monitor the request addresses: a
/// token this service never issued is refused even for a monitor that exists in no region,
/// while the same request carrying no token is answered with the not-found.
#[tokio::test]
async fn a_token_check_precedes_the_not_found_for_the_addressed_app_monitor() {
    let emulator = Emulator::new();
    // Nothing is created, and the caller is the account's own credential, so nothing but
    // the token check and the resolution can decide these answers.
    for listing in listings().into_iter().filter(|l| l.addresses_monitor) {
        for token in TOKENS_NO_LISTING_ISSUED {
            let case = format!("{} [{token}]", listing.case);
            let refused = emulator
                .rum(
                    ACCOUNT_KEY,
                    listing.method,
                    &(listing.uri)(ABSENT_MONITOR, Some(token)),
                    (listing.body)(Some(token)),
                )
                .await;
            assert_token_refused(&refused, &case, token);
        }

        let control = format!("{} (carrying no token)", listing.case);
        let not_found = emulator
            .rum(
                ACCOUNT_KEY,
                listing.method,
                &(listing.uri)(ABSENT_MONITOR, None),
                (listing.body)(None),
            )
            .await;
        assert_refused(&not_found, &control, 404, "ResourceNotFoundException");
    }
}

/// The refusal is about the token's provenance, not about tokens in general: a token the
/// listing itself issued is read as the resume point it is, so an unauthorized caller
/// sending it is answered with the access denial and an authorized one resumes the
/// listing.
#[tokio::test]
async fn a_token_the_listing_issued_is_not_what_the_member_check_refuses() {
    let emulator = Emulator::new();
    for index in 0..3 {
        emulator
            .create_monitor(&format!("token-precedence-{index}"))
            .await;
    }
    let unauthorized = emulator.assumed_role_key("holds-no-grant", None).await;

    // One real token, taken from the listing itself with a credential that may list.
    let first = emulator
        .rum(ACCOUNT_KEY, "POST", "/appmonitors?maxResults=1", None)
        .await;
    let case = "ListAppMonitors, a token the listing issued";
    assert_eq!(
        first.status, 200,
        "case {case:?}: expected the first page to answer 200, got {} {:?}",
        first.status,
        String::from_utf8_lossy(&first.body)
    );
    let issued = first.json(case)["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a NextToken while monitors remain, got {:?}",
                first.json(case)
            )
        })
        .to_string();

    let denied = emulator
        .rum(
            &unauthorized,
            "POST",
            &format!("/appmonitors?maxResults=1&nextToken={issued}"),
            None,
        )
        .await;
    assert_refused(&denied, case, 403, "AccessDeniedException");

    let resumed = emulator
        .rum(
            ACCOUNT_KEY,
            "POST",
            &format!("/appmonitors?maxResults=1&nextToken={issued}"),
            None,
        )
        .await;
    assert_eq!(
        resumed.status, 200,
        "case {case:?}: expected the issued token to resume the listing, got {} {:?}",
        resumed.status,
        String::from_utf8_lossy(&resumed.body)
    );
}
