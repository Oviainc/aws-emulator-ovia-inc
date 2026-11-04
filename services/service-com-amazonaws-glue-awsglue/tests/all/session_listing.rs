// SPDX-License-Identifier: CC0-1.0

//! `ListSessions`: the interactive sessions an endpoint holds.
//!
//! RULE: `ListSessions` "retrieve[s] a list of sessions" — it reports every session the
//! Region it is served in holds, from the same records `CreateSession` wrote and `GetSession`
//! reads back, narrowed to those carrying every pair a `Tags` filter names, in `Id` order and
//! one `MaxResults`-sized page at a time with a `NextToken` that resumes the walk; an
//! endpoint holding no session has an EMPTY listing, not a refusal.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many sessions are stored, the
//! page size asked for (including the published bounds 1 and 1000, a size past the stored
//! count, and one past each bound), which tags the filter names (none, one, a pair, a value
//! and a key nothing carries), the Region the call is served in, and the status the clock has
//! owed the stored sessions.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::SESSION_SETTLES_AFTER;
use smithy_server_runtime::MultiServiceRouter;

use crate::statements::rewind_session;
use crate::wire::{call, call_in_region, emulator, emulator_on, REGION};

/// An IAM role ARN of the form the modelled `OrchestrationRoleArn` pattern requires
/// (`^arn:aws[^:]*:iam::[0-9]*:role/.+$`).
const ROLE: &str = "arn:aws:iam::000000000000:role/GlueSessionRole";

/// Two of the modelled `SessionStatus` values (`smithy_query shape=SessionStatus`): the one a
/// create leaves a session in, and the one the clock owes it.
const STATUS_PROVISIONING: &str = "PROVISIONING";
const STATUS_READY: &str = "READY";

/// Create one session in `region`, with the tags the row asks for.
async fn given_session(router: &MultiServiceRouter, region: &str, id: &str, tags: Value) {
    let mut body = json!({
        "Id": id,
        "Role": ROLE,
        "Command": { "Name": "glueetl" },
    });
    if !tags.is_null() {
        body["Tags"] = tags;
    }
    let (status, parsed) = call_in_region(router, region, "CreateSession", &body).await;
    assert_eq!(
        status, 200,
        "CreateSession({id}) in {region} must answer 200, got {status} with body {parsed}"
    );
}

/// One `ListSessions` answer, served in `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, request: &Value) -> Value {
    let (status, parsed) = call_in_region(router, region, "ListSessions", request).await;
    assert_eq!(
        status, 200,
        "ListSessions must answer 200, got {status} with body {parsed}"
    );
    parsed
}

/// The `Ids` of one answer, in the order the service returned them.
fn ids(page: &Value) -> Vec<String> {
    page.get("Ids")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must report an Ids list, got {page}"))
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("a session id is a string, got {id}"))
                .to_string()
        })
        .collect()
}

/// The `Sessions` of one answer, in the order the service returned them.
fn sessions(page: &Value) -> Vec<Value> {
    page.get("Sessions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the answer must report a Sessions list, got {page}"))
        .clone()
}

/// Every stored session is reported, in `Id` order, one page at a time, and each page's
/// `Sessions` agree element for element with its `Ids`.
#[tokio::test]
async fn every_stored_session_is_listed_and_paged() {
    // (case, how many sessions are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no sessions at all", 0, None),
        ("one session, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize` range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(1_000)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        // Created out of id order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("session-{index:02}"))
            .collect();
        for id in expected.iter().rev() {
            given_session(&router, REGION, id, Value::Null).await;
        }

        // Walk every page the service hands out, following its own token.
        let mut listed: Vec<String> = Vec::new();
        let mut projected: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({});
            if let Some(size) = page_size {
                request["MaxResults"] = json!(size);
            }
            if let Some(cursor) = &token {
                request["NextToken"] = json!(cursor);
            }
            let page = list_in(&router, REGION, &request).await;
            let page_ids = ids(&page);
            let page_sessions = sessions(&page);
            assert_eq!(
                page_sessions.len(),
                page_ids.len(),
                "case {case:?}: Ids and Sessions must report one page; got {page}"
            );
            for (id, session) in page_ids.iter().zip(page_sessions.iter()) {
                assert_eq!(
                    session.get("Id").and_then(Value::as_str),
                    Some(id.as_str()),
                    "case {case:?}: Sessions must agree with Ids element for element, got \
                     {session} against {id}"
                );
            }
            if let Some(size) = page_size {
                assert!(
                    page_ids.len() <= usize::try_from(*size).unwrap_or(usize::MAX),
                    "case {case:?}: a page must not exceed MaxResults {size}, got {page_ids:?}"
                );
            }
            listed.extend(page_ids);
            projected.extend(page_sessions);
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                listed.len() <= *stored,
                "case {case:?}: the walk must terminate, got {listed:?} for {stored} sessions"
            );
        }

        assert_eq!(
            listed, expected,
            "case {case:?}: expected every stored session id in id order, got {listed:?}"
        );
        // An id the listing reports is a session `GetSession` resolves, and the projection
        // the listing served is the one that read serves: one record, two readers.
        for (id, session) in listed.iter().zip(projected.iter()) {
            let read = list_one(&router, id).await;
            assert_eq!(
                &read, session,
                "case {case:?}: GetSession({id}) must report the session the listing reported, \
                 got {read} against {session}"
            );
        }
    }
}

/// The `Session` a `GetSession` answer reports for `id`.
async fn list_one(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, parsed) = call(router, "GetSession", &json!({ "Id": id })).await;
    assert_eq!(
        status, 200,
        "GetSession({id}) must answer 200, got {status} with body {parsed}"
    );
    parsed
        .get("Session")
        .cloned()
        .unwrap_or_else(|| panic!("GetSession must report a Session, got {parsed}"))
}

/// A `Tags` filter narrows the listing to the sessions carrying EVERY pair it names.
#[tokio::test]
async fn a_tag_filter_narrows_the_listing_to_the_sessions_carrying_every_pair() {
    let router = emulator();
    given_session(
        &router,
        REGION,
        "tagged-both",
        json!({"team": "data", "env": "prod"}),
    )
    .await;
    given_session(&router, REGION, "tagged-team", json!({"team": "data"})).await;
    given_session(&router, REGION, "tagged-other", json!({"team": "ops"})).await;
    given_session(&router, REGION, "untagged", Value::Null).await;

    // (case, the Tags filter, the ids it must report)
    let rows: &[(&str, Value, &[&str])] = &[
        (
            "no filter: every session",
            Value::Null,
            &["tagged-both", "tagged-other", "tagged-team", "untagged"],
        ),
        (
            "an empty filter narrows nothing",
            json!({}),
            &["tagged-both", "tagged-other", "tagged-team", "untagged"],
        ),
        (
            "one pair",
            json!({"team": "data"}),
            &["tagged-both", "tagged-team"],
        ),
        (
            "a pair that must BOTH match",
            json!({"team": "data", "env": "prod"}),
            &["tagged-both"],
        ),
        ("a value nothing carries", json!({"team": "finance"}), &[]),
        ("a key nothing carries", json!({"owner": "data"}), &[]),
    ];

    for (case, filter, expected) in rows {
        let mut request = json!({});
        if !filter.is_null() {
            request["Tags"] = filter.clone();
        }
        let page = list_in(&router, REGION, &request).await;
        assert_eq!(
            ids(&page),
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the sessions carrying every pair of {filter}, got {page}"
        );
        assert_eq!(
            sessions(&page).len(),
            expected.len(),
            "case {case:?}: Sessions must carry the same page as Ids, got {page}"
        );
    }
}

/// A page size one past either published bound is refused by the modelled constraint rather
/// than served as some other page size.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    given_session(&router, REGION, "bounded-session", Value::Null).await;

    // `MaxResults` is `PageSize`, range 1..1000.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1_001)] {
        let (status, body) = call(&router, "ListSessions", &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }
}

/// A cursor names a POSITION in the `Id` order: the page resumes at the first stored id
/// strictly after it, whether or not this service issued that token.
#[tokio::test]
async fn a_cursor_names_a_position_in_the_id_order() {
    let router = emulator();
    for id in ["session-a", "session-b", "session-c"] {
        given_session(&router, REGION, id, Value::Null).await;
    }

    // (case, the NextToken sent, the ids the page must report)
    let rows: &[(&str, &str, &[&str])] = &[
        (
            "a cursor before every id",
            "session-",
            &["session-a", "session-b", "session-c"],
        ),
        (
            "the id the first page ended on",
            "session-a",
            &["session-b", "session-c"],
        ),
        (
            "a token nothing minted, between two ids",
            "session-b0",
            &["session-c"],
        ),
        ("a token past every id", "session-z", &[]),
    ];

    for (case, token, expected) in rows {
        let page = list_in(&router, REGION, &json!({ "NextToken": token })).await;
        assert_eq!(
            ids(&page),
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the sessions after {token}, got {page}"
        );
    }
}

/// A session is served only through the endpoint that created it, so a listing reports the
/// sessions of the Region it is served in and nothing else — including an endpoint nothing
/// has created a session in, whose listing is empty rather than a refusal.
#[tokio::test]
async fn the_listing_reports_only_the_sessions_of_the_region_it_is_served_in() {
    let router = emulator();
    given_session(&router, "us-east-1", "east-one", Value::Null).await;
    given_session(&router, "us-east-1", "east-two", Value::Null).await;
    given_session(&router, "eu-west-1", "west-one", Value::Null).await;

    // (case, the Region the listing is served in, the ids it must report)
    let rows: &[(&str, &str, &[&str])] = &[
        ("the Region two sessions were created in", "us-east-1", &["east-one", "east-two"]),
        ("the Region one session was created in", "eu-west-1", &["west-one"]),
        ("a Region nothing was created in", "ap-south-1", &[]),
    ];

    for (case, region, expected) in rows {
        let page = list_in(&router, region, &json!({})).await;
        assert_eq!(
            ids(&page),
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the sessions of {region}, got {page}"
        );
        assert_eq!(
            page.get("NextToken"),
            None,
            "case {case:?}: a listing that reached its end carries no token, got {page}"
        );
    }
}

/// The listing reports the status the clock had already owed each session, exactly as a
/// single-session read does.
#[tokio::test]
async fn the_listing_reports_the_status_the_clock_owes_each_session() {
    // (case, how long ago the session was created, the status the listing must report)
    let rows: &[(&str, Duration, &str)] = &[
        ("the instant it was created", Duration::ZERO, STATUS_PROVISIONING),
        ("exactly at the due instant", SESSION_SETTLES_AFTER, STATUS_READY),
        ("well past the due instant", SESSION_SETTLES_AFTER * 4, STATUS_READY),
    ];

    for (case, elapsed, expected) in rows {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(engine.clone());
        given_session(&router, REGION, "settling", Value::Null).await;
        // The session suites' own helper, which rewinds a stored session's instants so the
        // status the clock owes it is observable without sleeping for the span.
        rewind_session(&engine, "settling", *elapsed);

        let page = list_in(&router, REGION, &json!({})).await;
        let listed = sessions(&page);
        let [session] = listed.as_slice() else {
            panic!("case {case:?}: one stored session must be listed, got {page}");
        };
        assert_eq!(
            session.get("Status").and_then(Value::as_str),
            Some(*expected),
            "case {case:?}: expected {expected} {elapsed:?} after the create, got {session}"
        );
        // The single-session read agrees, and a second listing reports the same record: the
        // status comes from the DUE instant the create recorded, not from who read it.
        let read = list_one(&router, "settling").await;
        assert_eq!(
            &read, session,
            "case {case:?}: GetSession must agree with the listing, got {read} against {session}"
        );
        let again = list_in(&router, REGION, &json!({})).await;
        assert_eq!(
            again, page,
            "case {case:?}: two listings must agree, got {again} then {page}"
        );
    }
}
