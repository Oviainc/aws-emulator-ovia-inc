// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListSessions`: the account-wide listing of every session this
//! service's store holds (`POST /sessions/`).
//!
//! Every test drives real HTTP requests through the framework router over the shared
//! engine, reusing the request harness of `tests/all/sessions.rs` rather than a second
//! copy of it, so the wire protocol (restJson1), the URI bindings, the boundary validator
//! and the handler are all exercised and every assertion reads a response BODY.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::sessions::{
    call, create_invocation, create_session, error_code, is_model_uuid, router, ACCOUNT, REGION,
};

/// One page of `ListSessions`, with the query the caller asks for. `maxResults` is
/// omitted from the URI when `max_results` is `None`, which is how an SDK sends an absent
/// member; the request carries no body, because every member of this operation's input is
/// an `@httpQuery` one.
async fn list_sessions(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut path = String::from("/sessions/");
    let mut separator = '?';
    if let Some(value) = max_results {
        path.push_str(&format!("{separator}maxResults={value}"));
        separator = '&';
    }
    if let Some(token) = next_token {
        path.push_str(&format!("{separator}nextToken={token}"));
    }
    call(router, "POST", &path, Value::Null).await
}

/// The `sessionSummaries` of a successful page.
fn summaries(body: &Value) -> &Vec<Value> {
    body["sessionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("ListSessions: no sessionSummaries in {body:?}"))
}

/// The `nextToken` of a page, or `None` when the page carries none.
fn next_token(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Walk every page of `ListSessions`, returning the session ids seen in order and the
/// size of each page.
async fn drain_sessions(
    router: &MultiServiceRouter,
    max_results: i64,
) -> (Vec<String>, Vec<usize>) {
    let mut ids: Vec<String> = Vec::new();
    let mut page_sizes: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (status, listed) = list_sessions(router, Some(max_results), token.as_deref()).await;
        assert_eq!(
            status, 200,
            "ListSessions(maxResults={max_results}): expected 200, got {status} {listed:?}"
        );
        let page = summaries(&listed);
        page_sizes.push(page.len());
        for summary in page {
            ids.push(
                summary["sessionId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("ListSessions: no sessionId in {summary:?}"))
                    .to_string(),
            );
        }
        token = next_token(&listed);
        if token.is_none() {
            return (ids, page_sizes);
        }
        assert!(
            page_sizes.len() <= 64,
            "ListSessions(maxResults={max_results}): pagination did not terminate after {} pages",
            page_sizes.len()
        );
    }
}

/// The instant a `date-time` member carries. Every `DateTimestamp` member of
/// `SessionSummary` declares `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="SessionSummary")`), so its wire form is an RFC
/// 3339 string.
fn instant(case: &str, member: &str, value: &Value) -> DateTime {
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 string, got {value:?}")
    });
    DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} {rendered:?} must parse as RFC 3339: {error}")
    })
}

/// RULE: a paginated list returns at most `maxResults` items per page from stored state,
/// in a stable order, emits a `nextToken` only while items remain, and the pages
/// concatenate to every item exactly once. The rows vary the stored session count and the
/// page size, including the published bounds of `maxResults` (1..1000,
/// `smithy_query(query="operation",operation="ListSessions")`).
#[tokio::test]
async fn list_sessions_pages_stored_state_without_loss_or_repetition() {
    // (stored session count, maxResults, expected page sizes)
    let cases: [(usize, i64, &[usize]); 7] = [
        (0, 1, &[0]),
        (1, 1, &[1]),
        (3, 1, &[1, 1, 1]),
        (5, 2, &[2, 2, 1]),
        (4, 4, &[4]),
        (4, 1000, &[4]),
        (7, 3, &[3, 3, 1]),
    ];

    for (total, max_results, expected_pages) in cases {
        let router = router();
        let mut created: Vec<String> = Vec::new();
        for n in 0..total {
            let (session_id, _) = create_session(&router, json!({ "n": n.to_string() })).await;
            created.push(session_id);
        }

        let (seen, page_sizes) = drain_sessions(&router, max_results).await;
        assert_eq!(
            page_sizes, expected_pages,
            "case (total={total}, maxResults={max_results}): expected pages \
             {expected_pages:?}, got {page_sizes:?}"
        );
        assert_eq!(
            seen, created,
            "case (total={total}, maxResults={max_results}): the pages must concatenate to \
             every stored session once, in store order; got {seen:?}"
        );
    }
}

/// RULE: every pass over the same stored state lists the same sessions in the same order,
/// so a page boundary is a property of the store and not of the page size the caller
/// chose. The rows are the page sizes a caller may walk the same 6 sessions with.
#[tokio::test]
async fn pagination_order_is_the_same_whatever_the_page_size() {
    let router = router();
    let mut created: Vec<String> = Vec::new();
    for n in 0..6 {
        let (session_id, _) = create_session(&router, json!({ "n": n.to_string() })).await;
        created.push(session_id);
    }

    for max_results in [1_i64, 2, 3, 4, 5, 6, 7, 1000] {
        let (seen, _) = drain_sessions(&router, max_results).await;
        assert_eq!(
            seen, created,
            "case maxResults={max_results}: every walk must list the stored sessions in one \
             stable order, got {seen:?}"
        );
    }
}

/// RULE: a listing reports each item from the stored resource, so every summary member
/// equals what the resource's own reader (`GetSession`) reports for that resource, at
/// whatever lifecycle point the resource is in. The rows span the lifecycle states a
/// stored session can be listed in — freshly created, updated, and ended — reached
/// through the operations that write them.
#[tokio::test]
async fn a_listed_summary_reports_what_get_session_reports() {
    // The `SessionStatus` catalog the model declares
    // (`smithy_query(query="shape",shape="SessionStatus")`).
    const SESSION_STATUS: [&str; 3] = ["ACTIVE", "EXPIRED", "ENDED"];

    let router = router();
    // (case, the status the session must be listed in)
    let cases: [(&str, &str); 3] = [
        ("freshly created", "ACTIVE"),
        ("updated after creation", "ACTIVE"),
        // ENDED is reached through EndSession, the one writer of that status.
        ("ended", "ENDED"),
    ];

    let mut expected: Vec<(&str, String, &str)> = Vec::new();
    for (case, status) in cases {
        let (session_id, _) = create_session(&router, json!({ "case": case })).await;
        match case {
            "updated after creation" => {
                let (code, body) = call(
                    &router,
                    "PUT",
                    &format!("/sessions/{session_id}/"),
                    json!({ "sessionMetadata": { "topic": "revised" } }),
                )
                .await;
                assert_eq!(code, 200, "case {case:?}: UpdateSession got {code} {body:?}");
            }
            "ended" => {
                let (code, body) =
                    call(&router, "PATCH", &format!("/sessions/{session_id}"), Value::Null).await;
                assert_eq!(code, 200, "case {case:?}: EndSession got {code} {body:?}");
            }
            _ => {}
        }
        expected.push((case, session_id, status));
    }

    let (status, listed) = list_sessions(&router, Some(1000), None).await;
    assert_eq!(status, 200, "ListSessions got {status} {listed:?}");
    let page = summaries(&listed);
    assert_eq!(
        page.len(),
        expected.len(),
        "every stored session must be listed, got {listed:?}"
    );

    for (case, session_id, status_wanted) in &expected {
        let summary = page
            .iter()
            .find(|summary| summary["sessionId"].as_str() == Some(session_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {session_id} is missing from {listed:?}"));

        let (code, read) = call(&router, "GET", &format!("/sessions/{session_id}/"), Value::Null)
            .await;
        assert_eq!(code, 200, "case {case:?}: GetSession got {code} {read:?}");

        for member in ["sessionId", "sessionArn", "sessionStatus"] {
            assert_eq!(
                summary[member], read[member],
                "case {case:?}: the summary's {member} must equal GetSession's, got \
                 {summary:?} vs {read:?}"
            );
        }
        for member in ["createdAt", "lastUpdatedAt"] {
            assert_eq!(
                instant(case, member, &summary[member]),
                instant(case, member, &read[member]),
                "case {case:?}: the summary's {member} must equal GetSession's, got \
                 {summary:?} vs {read:?}"
            );
        }
        assert_eq!(
            summary["sessionStatus"].as_str(),
            Some(*status_wanted),
            "case {case:?}: expected status {status_wanted}, got {summary:?}"
        );
        assert!(
            SESSION_STATUS.contains(&summary["sessionStatus"].as_str().unwrap_or_default()),
            "case {case:?}: sessionStatus must be one of {SESSION_STATUS:?}, got {summary:?}"
        );
        assert!(
            is_model_uuid(summary["sessionId"].as_str().unwrap_or_default()),
            "case {case:?}: sessionId must satisfy the modelled Uuid pattern, got {summary:?}"
        );
        assert_eq!(
            summary["sessionArn"].as_str(),
            Some(format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:session/{session_id}").as_str()),
            "case {case:?}: the summary must report the session's minted ARN, got {summary:?}"
        );
        assert!(
            instant(case, "lastUpdatedAt", &summary["lastUpdatedAt"])
                >= instant(case, "createdAt", &summary["createdAt"]),
            "case {case:?}: lastUpdatedAt cannot precede createdAt, got {summary:?}"
        );
    }
}

/// RULE: a session leaves this listing only when `DeleteSession` releases it, and the
/// sessions beside it are untouched. The rows are the two outcomes a delete has on the
/// listing: the released session is gone, every other one stays.
#[tokio::test]
async fn a_deleted_session_leaves_the_listing_and_the_others_stay() {
    let router = router();
    let mut created: Vec<String> = Vec::new();
    for n in 0..4 {
        let (session_id, _) = create_session(&router, json!({ "n": n.to_string() })).await;
        created.push(session_id);
    }

    // DeleteSession refuses an ACTIVE session, so the session is ended first.
    let released = created[1].clone();
    let (code, body) = call(&router, "PATCH", &format!("/sessions/{released}"), Value::Null).await;
    assert_eq!(code, 200, "EndSession got {code} {body:?}");
    let (code, body) = call(
        &router,
        "DELETE",
        &format!("/sessions/{released}/"),
        Value::Null,
    )
    .await;
    assert_eq!(code, 200, "DeleteSession got {code} {body:?}");

    let remaining: Vec<String> = created
        .iter()
        .filter(|id| **id != released)
        .cloned()
        .collect();
    let (seen, _) = drain_sessions(&router, 2).await;
    assert_eq!(
        seen, remaining,
        "the released session must be gone and the others must stay, got {seen:?}"
    );
}

/// RULE: an omitted `maxResults` pages at the default the model declares (10), a value
/// inside the published range is honoured, and one outside it is refused with the
/// modelled `ValidationException` (400) rather than silently reinterpreted. The rows are
/// the member absent, each published bound, and one step past each of them
/// (`smithy_query(query="operation",operation="ListSessions")` -> `maxResults` has
/// `range {min: 1, max: 1000}` and `default: 10`).
#[tokio::test]
async fn max_results_defaults_and_is_refused_outside_the_published_range() {
    use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::state::DEFAULT_LIST_PAGE;

    /// The model's published bounds for `maxResults`.
    const MIN: i64 = 1;
    /// See [`MIN`].
    const MAX: i64 = 1000;
    /// The default the model declares for `maxResults`, asserted independently of the
    /// implementation's constant so a constant that drifted from the model fails here.
    const MODELLED_DEFAULT: usize = 10;

    assert_eq!(
        usize::try_from(DEFAULT_LIST_PAGE).ok(),
        Some(MODELLED_DEFAULT),
        "case \"the service's default page size\": expected the modelled {MODELLED_DEFAULT}, \
         got {DEFAULT_LIST_PAGE:?}"
    );

    let stored = MODELLED_DEFAULT + 2;
    let router = router();
    for n in 0..stored {
        create_session(&router, json!({ "n": n.to_string() })).await;
    }

    // (case, the maxResults query value, the accepted first-page size or None when the
    // value is outside the published range and must be refused)
    let cases: [(&str, Option<i64>, Option<usize>); 7] = [
        ("maxResults omitted", None, Some(MODELLED_DEFAULT)),
        ("the published minimum", Some(MIN), Some(1)),
        ("one below the published minimum", Some(MIN - 1), None),
        // Below the minimum and distinct from the integer zero an absent member arrives
        // as, so it exercises the handler's own range check rather than only the
        // boundary validator's.
        ("negative, well below the published minimum", Some(-1), None),
        ("inside the published range", Some(5), Some(5)),
        ("the published maximum", Some(MAX), Some(stored)),
        ("one above the published maximum", Some(MAX + 1), None),
    ];

    for (case, value, accepted) in cases {
        let (status, body) = list_sessions(&router, value, None).await;
        match accepted {
            Some(size) => {
                assert_eq!(
                    (status, body["sessionSummaries"].as_array().map(Vec::len)),
                    (200, Some(size)),
                    "case {case:?}: maxResults={value:?} must page at {size}, got {status} \
                     {body:?}"
                );
                let expects_token = size < stored;
                assert_eq!(
                    next_token(&body).is_some(),
                    expects_token,
                    "case {case:?}: nextToken presence must be {expects_token}, got {body:?}"
                );
            }
            None => assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case {case:?}: maxResults={value:?} is outside the published range and must \
                 be refused, got {status} {body:?}"
            ),
        }
    }
}

/// RULE: a pagination cursor belongs to the listing that issued it, so a token this
/// listing did not mint is refused with the modelled `ValidationException` (400) rather
/// than silently resuming at an unrelated offset. The rows span the ways a token can fail
/// to be this listing's: it is not a token at all, or it is another listing's.
#[tokio::test]
async fn a_pagination_token_this_listing_did_not_issue_is_refused() {
    let router = router();
    // Two sessions, each with two invocations, so both listings page.
    let (first, _) = create_session(&router, json!({ "n": "1" })).await;
    let (second, _) = create_session(&router, json!({ "n": "2" })).await;
    for session in [&first, &second] {
        for _ in 0..2 {
            create_invocation(&router, session, json!({})).await;
        }
    }

    // A cursor ListInvocations minted for one of those sessions.
    let (status, page) = call(
        &router,
        "POST",
        &format!("/sessions/{first}/invocations/?maxResults=1"),
        json!({}),
    )
    .await;
    assert_eq!(status, 200, "ListInvocations got {status} {page:?}");
    let invocations_token =
        next_token(&page).unwrap_or_else(|| panic!("ListInvocations must page, got {page:?}"));

    // (case, the token)
    let cases: Vec<(&str, String)> = vec![
        ("not a token at all", "not-a-token".to_string()),
        ("base64 of unrelated text", "6f726465723a".to_string()),
        ("a truncated token", "00".to_string()),
        ("another listing's cursor", invocations_token.clone()),
    ];

    for (case, token) in cases {
        let (status, refused) = list_sessions(&router, Some(1), Some(&token)).await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "ValidationException"),
            "case {case:?}: token {token:?} must be refused, got {status} {refused:?}"
        );
    }

    // And symmetrically: a ListSessions cursor is not a ListInvocations cursor.
    let (status, page) = list_sessions(&router, Some(1), None).await;
    assert_eq!(status, 200, "ListSessions got {status} {page:?}");
    let sessions_token =
        next_token(&page).unwrap_or_else(|| panic!("ListSessions must page, got {page:?}"));
    let (status, refused) = call(
        &router,
        "POST",
        &format!("/sessions/{first}/invocations/?maxResults=1&nextToken={sessions_token}"),
        json!({}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "ValidationException"),
        "case \"a ListSessions cursor on ListInvocations\": expected 400 ValidationException, \
         got {status} {refused:?}"
    );
}

/// RULE: a reporting operation must not change what a later caller sees. `ListSessions`
/// is `@readonly` (`smithy_query(query="operation",operation="ListSessions")`), so
/// repeating it — and resuming it with its own cursor — leaves every response identical.
#[tokio::test]
async fn listing_sessions_does_not_change_what_a_later_caller_sees() {
    let router = router();
    let mut created: Vec<String> = Vec::new();
    for n in 0..3 {
        let (session_id, _) = create_session(&router, json!({ "n": n.to_string() })).await;
        created.push(session_id);
    }

    let (status, first_pass) = list_sessions(&router, Some(2), None).await;
    assert_eq!(status, 200, "ListSessions got {status} {first_pass:?}");
    // Walk the rest of the pages, then list the first page again.
    let (_, _) = drain_sessions(&router, 2).await;
    let (status, second_pass) = list_sessions(&router, Some(2), None).await;
    assert_eq!(status, 200, "ListSessions got {status} {second_pass:?}");
    assert_eq!(
        first_pass, second_pass,
        "case \"repeated first page\": a read must answer identically, got {first_pass:?} \
         then {second_pass:?}"
    );

    // Resuming with the cursor the first page issued yields the remainder, and does so
    // every time it is used.
    let token = next_token(&first_pass).unwrap_or_else(|| panic!("got {first_pass:?}"));
    let (status, resumed) = list_sessions(&router, Some(2), Some(&token)).await;
    assert_eq!(status, 200, "ListSessions got {status} {resumed:?}");
    let (status, resumed_again) = list_sessions(&router, Some(2), Some(&token)).await;
    assert_eq!(status, 200, "ListSessions got {status} {resumed_again:?}");
    assert_eq!(
        resumed, resumed_again,
        "case \"a cursor replayed\": the same cursor must answer identically, got {resumed:?} \
         then {resumed_again:?}"
    );

    // And every session is still readable, with the status it had before the listings.
    for session_id in &created {
        let (status, read) = call(&router, "GET", &format!("/sessions/{session_id}/"), Value::Null)
            .await;
        assert_eq!(
            (status, read["sessionStatus"].as_str()),
            (200, Some("ACTIVE")),
            "case {session_id:?}: a listing must not move a session's status, got {status} \
             {read:?}"
        );
    }
}
