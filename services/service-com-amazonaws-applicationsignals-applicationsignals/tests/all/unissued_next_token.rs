// SPDX-License-Identifier: CC0-1.0

//! The refusal every read of this service reports for a `NextToken` it did not issue.
//!
//! The rule under test: a `NextToken` is OPAQUE, so a token this service did not issue is
//! refused with the modelled `ValidationException` (HTTP 400) whose message names the member
//! and the requirement and carries NOTHING of the rejected token — one wording for one
//! failure, whichever read the caller hit. The rejected token is caller input, and a message
//! that rendered it would copy it into every client log recording the failure, which is the
//! same rule this service's constraint-violation clauses hold to (`max_results_range` pins it
//! for a rejected page bound).
//!
//! The rule is a property of the FAILURE, not of one operation, so every read whose cursor
//! this service resolves is a row below: the reads whose cursor is an offset into a derived
//! projection (`ListAuditFindings`, `ListEntityEvents`, `ListServices`,
//! `ListServiceOperations`, `ListServiceDependencies`, `ListServiceDependents`,
//! `ListServiceStates`, `ListServiceLevelObjectiveExclusionWindows`) beside the reads that
//! resolve a cursor of their own (`ListServiceLevelObjectives`, whose cursor is a store key,
//! and `ListGroupingAttributeDefinitions`, which issues no cursor at all). They differ along
//! where the model binds `NextToken` (the query string or the JSON body), along what the
//! cursor means, and along the HTTP method and route.
//!
//! The columns differ along WHY the token is not one this service issued: it is empty, it is
//! not hex, it is hex of bytes that are not text, it is well-formed but carries no marker of
//! this service, it is long, it looks like a credential a log must not keep — and it is a
//! token this service really DID issue, for another read.
//!
//! The two reads whose cursor is only a cursor within the scope that issued it
//! (`ListInstrumentationConfigurations`, `GetInstrumentationConfigurationStatus`) state that
//! scope after the same sentence; their own suites assert them.
//!
//! The harness is the one `slo_lifecycle` drives this service's `restJson1` surface with, so
//! every suite here speaks to one mounted router.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, create, name_path_segment, router, sli_config};

/// The ONE message a read of this service answers a `NextToken` it did not issue with: the
/// member and the requirement, and no rendering of the token.
const EXPECTED: &str = "NextToken is not a token this service issued";

/// The services the seeded SLOs monitor: more than one, so the paged reads below hold a set a
/// cursor can divide and a page of one issues a token this service really did issue.
const SEEDED_SERVICES: [&str; 3] = ["cursor-alpha", "cursor-beta", "cursor-gamma"];

/// The environment every seeded entity is hosted in, as `slo_lifecycle::sli_config` names it.
const ENVIRONMENT: &str = "prod";

/// One HTTP request as this suite's harness takes it: method, URI and optional JSON body.
type Request = (&'static str, String, Option<Value>);

/// Builds the request that lists with the given cursor, for one read.
type RequestBuilder = Box<dyn Fn(Option<&str>) -> Request>;

/// One read of this service whose `NextToken` this service resolves.
struct Read {
    /// The operation under test, named in every assertion.
    operation: &'static str,
    /// The output member the page would be reported in.
    items: &'static str,
    /// The request listing with `token` as its cursor, absent when `None`, with every member
    /// in the place this operation's model binds it to.
    request: RequestBuilder,
}

/// Seconds since the epoch, for a window around the instant the seeded SLOs are written at.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The key attributes of one Application Signals service entity.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": ENVIRONMENT })
}

/// The SLO name the SLO monitoring `service` is stored under.
fn slo_name(service: &str) -> String {
    format!("{service}-latency")
}

/// Store one SLO per entry of [`SEEDED_SERVICES`], so every read below has a set to page.
async fn seed(router: &MultiServiceRouter) {
    for service in SEEDED_SERVICES {
        let (status, created) = create(
            router,
            json!({
                "Name": slo_name(service),
                "SliConfig": sli_config(service, "GET /cart", 100.0)
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {service:?}: expected the setup create to answer 200, got {status} with {created}"
        );
    }
}

/// The query-string tail carrying a cursor, to append after a window, empty when there is
/// no cursor.
fn cursor_query(token: Option<&str>) -> String {
    token.map_or_else(String::new, |token| format!("&NextToken={token}"))
}

/// The WHOLE query string of a read whose only query member is the cursor, empty when there
/// is no cursor.
fn cursor_only_query(token: Option<&str>) -> String {
    token.map_or_else(String::new, |token| format!("?NextToken={token}"))
}

/// Every read under test, each pointed at a request it serves when no cursor is supplied.
fn reads(now: i64) -> Vec<Read> {
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    // The reads that take the entity to report on as a key-attribute map in the body, with
    // their window and cursor in the query string.
    let entity_read = |operation: &'static str, items: &'static str, path: &'static str| {
        let window = window.clone();
        Read {
            operation,
            items,
            request: Box::new(move |token| {
                (
                    "POST",
                    format!("{path}?{window}{}", cursor_query(token)),
                    Some(json!({ "KeyAttributes": service_entity(SEEDED_SERVICES[0]) })),
                )
            }),
        }
    };

    let audit_window = window.clone();
    let services_window = window.clone();
    vec![
        Read {
            operation: "ListAuditFindings",
            items: "AuditFindings",
            // `MaxResults` and `NextToken` are BODY members of this operation
            // (`smithy_query(query="operation", operation="ListAuditFindings")`: neither
            // carries an `@httpQuery` binding, unlike its `StartTime`/`EndTime`).
            request: Box::new(move |token| {
                let mut body = json!({
                    "AuditTargets": [
                        { "Type": "service", "Data": { "Service": service_entity(SEEDED_SERVICES[0]) } }
                    ]
                });
                if let Some(token) = token {
                    body["NextToken"] = json!(token);
                }
                ("POST", format!("/auditFindings?{audit_window}"), Some(body))
            }),
        },
        Read {
            operation: "ListEntityEvents",
            items: "ChangeEvents",
            request: Box::new(move |token| {
                (
                    "POST",
                    format!("/events{}", cursor_only_query(token)),
                    Some(json!({
                        "Entity": service_entity(SEEDED_SERVICES[0]),
                        "StartTime": now - 3_600,
                        "EndTime": now + 3_600
                    })),
                )
            }),
        },
        Read {
            operation: "ListServices",
            items: "ServiceSummaries",
            request: Box::new(move |token| {
                (
                    "GET",
                    format!("/services?{services_window}{}", cursor_query(token)),
                    None,
                )
            }),
        },
        entity_read(
            "ListServiceOperations",
            "ServiceOperations",
            "/service-operations",
        ),
        entity_read(
            "ListServiceDependencies",
            "ServiceDependencies",
            "/service-dependencies",
        ),
        entity_read(
            "ListServiceDependents",
            "ServiceDependents",
            "/service-dependents",
        ),
        Read {
            operation: "ListServiceStates",
            items: "ServiceStates",
            // Every input of this operation, `NextToken` included, is a body member.
            request: Box::new(move |token| {
                let mut body = json!({ "StartTime": now - 3_600, "EndTime": now + 3_600 });
                if let Some(token) = token {
                    body["NextToken"] = json!(token);
                }
                ("POST", "/service/states".to_string(), Some(body))
            }),
        },
        Read {
            operation: "ListServiceLevelObjectiveExclusionWindows",
            items: "ExclusionWindows",
            request: Box::new(|token| {
                (
                    "GET",
                    format!(
                        "/slo/{}/exclusion-windows{}",
                        name_path_segment(&slo_name(SEEDED_SERVICES[0])),
                        cursor_only_query(token)
                    ),
                    None,
                )
            }),
        },
        Read {
            operation: "ListServiceLevelObjectives",
            items: "SloSummaries",
            request: Box::new(|token| {
                (
                    "POST",
                    format!("/slos{}", cursor_only_query(token)),
                    Some(json!({})),
                )
            }),
        },
        Read {
            operation: "ListGroupingAttributeDefinitions",
            items: "GroupingAttributeDefinitions",
            // Every input member of this read is an `@httpQuery` binding, so the request
            // carries no body at all.
            request: Box::new(|token| {
                (
                    "POST",
                    format!(
                        "/grouping-attribute-definitions{}",
                        cursor_only_query(token)
                    ),
                    None,
                )
            }),
        },
    ]
}

/// The tokens no read of this service issued, and why each is not one of its own.
///
/// `issued_elsewhere` is a token this service really DID hand out, for a read other than the
/// one it is replayed against; the rest are forged with `hex::encode`, the one hex encoder in
/// this tree (the same call `state::encode_cursor` makes).
fn forged_tokens(issued_elsewhere: &str) -> Vec<(&'static str, String)> {
    vec![
        ("empty", String::new()),
        ("not hex at all", "not-a-token".to_string()),
        ("an odd number of hex digits", "abc".to_string()),
        ("hex of bytes that are not text", "ff".to_string()),
        (
            "hex carrying no marker of this service",
            ::hex::encode("forged/1/0"),
        ),
        (
            "a marker of this service with no position",
            ::hex::encode("services/1/first"),
        ),
        (
            "a token far longer than any this service issues",
            ::hex::encode("x".repeat(512)),
        ),
        (
            "a token shaped like a credential",
            "AKIAIOSFODNN7EXAMPLE".to_string(),
        ),
        (
            "a token this service issued for another read",
            issued_elsewhere.to_string(),
        ),
    ]
}

/// Two tokens this service really issued, each for a different read: `ListServices`, whose
/// cursor is an offset into a derived projection, and `ListServiceLevelObjectives`, whose
/// cursor is a key of the store. The first page of a set of several, bounded to one entry,
/// carries one.
async fn issued_tokens(router: &MultiServiceRouter, now: i64) -> [(&'static str, String); 2] {
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);
    let pages = [
        (
            "ListServices",
            "GET",
            format!("/services?{window}&MaxResults=1"),
            None,
        ),
        (
            "ListServiceLevelObjectives",
            "POST",
            "/slos?MaxResults=1".to_string(),
            Some(json!({})),
        ),
    ];

    let mut issued = Vec::new();
    for (issuer, method, uri, body) in pages {
        let (status, answer) = call(router, method, &uri, body).await;
        assert_eq!(
            status, 200,
            "case {issuer:?}: expected the setup page to answer 200, got {status} with {answer}"
        );
        let token = answer["NextToken"].as_str().unwrap_or_else(|| {
            panic!("case {issuer:?}: expected a cursor on a page of one of several, got {answer}")
        });
        issued.push((issuer, token.to_string()));
    }
    [issued[0].clone(), issued[1].clone()]
}

/// Every read refuses a token this service did not issue with HTTP 400, the modelled
/// `ValidationException`, and the ONE message — which carries no part of the rejected token,
/// however that token was shaped.
#[tokio::test]
async fn every_read_refuses_an_unissued_next_token_with_one_message_and_no_echo() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let issued = issued_tokens(&router, now).await;

    for read in reads(now) {
        // A token this service issued for ANOTHER read: the one of the pair that this read
        // did not issue itself, since a read honours its own.
        let issued_elsewhere = issued
            .iter()
            .find(|(issuer, _)| *issuer != read.operation)
            .map(|(_, token)| token.clone())
            .unwrap_or_else(|| panic!("case {:?}: expected a token issued by another read", read.operation));

        // The same request WITHOUT a cursor is served, so the refusal below is earned by the
        // token and by nothing else in the request.
        let (method, uri, body) = (read.request)(None);
        let (status, served) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, 200,
            "case {:?} without a cursor: expected the read to be served, got {status} with {served}",
            read.operation
        );

        for (why, token) in forged_tokens(&issued_elsewhere) {
            let case = format!("{} with {why}", read.operation);
            let (method, uri, body) = (read.request)(Some(&token));
            let (status, answer) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a token this service did not issue, got {status} with {answer}"
            );
            assert_eq!(
                answer["__type"]
                    .as_str()
                    .map(|kind| kind.rsplit('#').next_back().unwrap_or(kind)),
                Some("ValidationException"),
                "case {case:?}: expected a ValidationException, got {answer}"
            );
            let message = answer["message"].as_str().unwrap_or_default();
            assert_eq!(
                message, EXPECTED,
                "case {case:?}: expected the message {EXPECTED:?}, got {message:?}"
            );
            assert!(
                token.is_empty() || !message.contains(&token),
                "case {case:?}: the message must not echo the rejected token, got {message:?}"
            );
            assert!(
                answer.get(read.items).is_none() && answer.get("NextToken").is_none(),
                "case {case:?}: a refused cursor carries no page and no cursor, got {answer}"
            );
        }

        // The refused reads left the set as it was: the same request is still served, and
        // reports what it reported before.
        let (method, uri, body) = (read.request)(None);
        let (status, after) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, 200,
            "case {:?} after the refusals: expected the read to be served, got {status} with {after}",
            read.operation
        );
        assert_eq!(
            after[read.items], served[read.items],
            "case {:?}: expected the refused tokens to leave the page as it was, got {after}",
            read.operation
        );
    }
}

/// The refusal of a repeated `GroupingName` holds to the same rule: `PutGroupingConfiguration`
/// is refused with a message that names the member and the requirement and renders none of
/// the rejected names.
///
/// The rows differ along which name repeats, where the repeat sits, and how many other
/// definitions surround it. The grouping names are the ones the modelled
/// `GroupingAttributeDefinition.GroupingName` admits
/// (`smithy_query(query="shape", shape="GroupingAttributeDefinition")`).
#[tokio::test]
async fn a_repeated_grouping_name_is_refused_without_echoing_it() {
    let router = router();

    let rows = [
        (
            "repeated immediately",
            "BusinessUnit",
            json!([
                { "GroupingName": "BusinessUnit" },
                { "GroupingName": "BusinessUnit", "DefaultGroupingValue": "other" }
            ]),
        ),
        (
            "repeated after another definition",
            "Environment",
            json!([
                { "GroupingName": "Environment" },
                { "GroupingName": "BusinessUnit", "GroupingSourceKeys": ["bu"] },
                { "GroupingName": "Environment", "GroupingSourceKeys": ["env"] }
            ]),
        ),
        (
            "repeated three times",
            "Team",
            json!([
                { "GroupingName": "Team" },
                { "GroupingName": "Team", "GroupingSourceKeys": ["team"] },
                { "GroupingName": "Team", "DefaultGroupingValue": "unknown" }
            ]),
        ),
    ];

    for (label, repeated, definitions) in rows {
        let (status, error) = call(
            &router,
            "PUT",
            "/grouping-configuration",
            Some(json!({ "GroupingAttributeDefinitions": definitions })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a repeated GroupingName, got {status} with {error}"
        );
        let message = error["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("groupingAttributeDefinitions"),
            "case {label:?}: expected the refusal to name the member, got {message:?}"
        );
        assert!(
            !message.contains(repeated),
            "case {label:?}: the message must not echo the rejected name, got {message:?}"
        );
    }
}
