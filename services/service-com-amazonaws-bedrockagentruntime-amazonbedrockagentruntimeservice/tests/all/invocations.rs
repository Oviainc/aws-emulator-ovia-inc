// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListInvocations`: the listing of the invocations stored
//! under one session (`POST /sessions/{sessionIdentifier}/invocations/`).
//!
//! Every test drives real HTTP requests through the framework router over the shared
//! engine, reusing the request harness of `tests/all/sessions.rs` rather than a
//! second copy of it, so the wire protocol (restJson1), the URI bindings, the
//! boundary validator and the handler are all exercised and every assertion reads a
//! response BODY.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::sessions::{
    call, create_invocation, create_session, error_code, is_model_uuid, label, put_step, router,
    ACCOUNT, REGION,
};

/// One page of `ListInvocations` for `identifier`, with the query the caller asks
/// for. `maxResults` is omitted from the URI when `max_results` is `None`, which is
/// how an SDK sends an absent member.
async fn list_invocations(
    router: &MultiServiceRouter,
    identifier: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut path = format!("/sessions/{}/invocations/", label(identifier));
    let mut separator = '?';
    if let Some(value) = max_results {
        path.push_str(&format!("{separator}maxResults={value}"));
        separator = '&';
    }
    if let Some(token) = next_token {
        path.push_str(&format!("{separator}nextToken={token}"));
    }
    call(router, "POST", &path, json!({})).await
}

/// The `invocationSummaries` of a successful page.
fn summaries(body: &Value) -> &Vec<Value> {
    body["invocationSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("ListInvocations: no invocationSummaries in {body:?}"))
}

/// Walk every page of `ListInvocations`, returning the invocation ids seen in order
/// and the size of each page.
async fn drain_invocations(
    router: &MultiServiceRouter,
    identifier: &str,
    max_results: i64,
) -> (Vec<String>, Vec<usize>) {
    let mut ids: Vec<String> = Vec::new();
    let mut page_sizes: Vec<usize> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (status, listed) =
            list_invocations(router, identifier, Some(max_results), token.as_deref()).await;
        assert_eq!(
            status, 200,
            "ListInvocations(maxResults={max_results}): expected 200, got {status} {listed:?}"
        );
        let page = summaries(&listed);
        page_sizes.push(page.len());
        for summary in page {
            ids.push(
                summary["invocationId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("ListInvocations: no invocationId in {summary:?}"))
                    .to_string(),
            );
        }
        token = listed
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return (ids, page_sizes);
        }
        assert!(
            page_sizes.len() <= 64,
            "ListInvocations(maxResults={max_results}): pagination did not terminate after {} \
             pages",
            page_sizes.len()
        );
    }
}

/// RULE: a paginated list returns at most `maxResults` items per page from stored
/// state, in a stable order, emits a `nextToken` only while items remain, and the
/// pages concatenate to every stored item exactly once. The rows vary the stored
/// invocation count and the page size, including the published bounds of `maxResults`
/// (1..1000, `smithy_query(query="operation",operation="ListInvocations")`).
#[tokio::test]
async fn list_invocations_pages_stored_state_without_loss_or_repetition() {
    // (stored invocation count, maxResults, expected page sizes)
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
        let (session_id, _) = create_session(&router, Value::Null).await;

        let mut written: Vec<String> = Vec::new();
        for n in 0..total {
            written.push(
                create_invocation(
                    &router,
                    &session_id,
                    json!({ "description": format!("invocation-{n}") }),
                )
                .await,
            );
        }

        let (seen, page_sizes) = drain_invocations(&router, &session_id, max_results).await;
        assert_eq!(
            page_sizes, expected_pages,
            "case (total={total}, maxResults={max_results}): expected pages {expected_pages:?}, \
             got {page_sizes:?}"
        );
        assert_eq!(
            seen, written,
            "case (total={total}, maxResults={max_results}): the pages must concatenate to every \
             stored invocation once, in store order; got {seen:?}"
        );
    }
}

/// RULE: every summary reports the invocation as it was PERSISTED — the session it
/// belongs to, its `invocationId` and the `createdAt` the creating call reported — so
/// a listing never invents or re-derives either value. The rows vary the request the
/// invocation was created by: with and without a client-chosen `invocationId`, and
/// with and without a `description`.
#[tokio::test]
async fn each_summary_reports_the_invocation_create_persisted() {
    // (case, the CreateInvocation body, the invocationId it must report)
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        ("no members at all", json!({}), None),
        (
            "a description only",
            json!({ "description": "User asking about weather in Seattle" }),
            None,
        ),
        (
            "a client-chosen invocationId",
            json!({ "invocationId": "0123abcd-4567-89ef-0123-456789abcdef" }),
            Some("0123abcd-4567-89ef-0123-456789abcdef"),
        ),
        (
            "a client-chosen invocationId and a description",
            json!({
                "invocationId": "fedcba98-7654-3210-fedc-ba9876543210",
                "description": "second turn",
            }),
            Some("fedcba98-7654-3210-fedc-ba9876543210"),
        ),
    ];

    for (case, body, chosen) in cases {
        let router = router();
        let (session_id, session_arn) = create_session(&router, Value::Null).await;

        let path = format!("/sessions/{}/invocations/", label(&session_id));
        let (status, created) = call(&router, "PUT", &path, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateInvocation expected 201, got {status} {created:?}"
        );
        let created_id = created["invocationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no invocationId in {created:?}"))
            .to_string();
        if let Some(expected) = chosen {
            assert_eq!(
                created_id, expected,
                "case {case:?}: a client-chosen invocationId must be kept, got {created:?}"
            );
        } else {
            assert!(
                is_model_uuid(&created_id),
                "case {case:?}: an absent invocationId must be minted as a model UUID, got \
                 {created_id:?}"
            );
        }

        // Listed by ARN as well as by bare id: both forms name the same session.
        for identifier in [session_id.clone(), session_arn.clone()] {
            let (status, listed) = list_invocations(&router, &identifier, None, None).await;
            assert_eq!(
                status, 200,
                "case {case:?} via {identifier}: expected 200, got {status} {listed:?}"
            );
            let page = summaries(&listed);
            assert_eq!(
                page.len(),
                1,
                "case {case:?} via {identifier}: exactly the one stored invocation is due, got \
                 {listed:?}"
            );
            assert_eq!(
                (
                    page[0]["sessionId"].as_str(),
                    page[0]["invocationId"].as_str(),
                    &page[0]["createdAt"],
                ),
                (
                    Some(session_id.as_str()),
                    Some(created_id.as_str()),
                    &created["createdAt"],
                ),
                "case {case:?} via {identifier}: the summary must report what CreateInvocation \
                 persisted ({created:?}), got {:?}",
                page[0]
            );
            assert!(
                listed.get("nextToken").is_none_or(Value::is_null),
                "case {case:?} via {identifier}: no nextToken is due when nothing remains, got \
                 {listed:?}"
            );
        }
    }
}

/// RULE: the listing is scoped to the session it names — it reports that session's
/// invocations and no others, so two sessions never see each other's content. The
/// rows are the two identifier forms the model admits for the scope.
#[tokio::test]
async fn list_invocations_is_scoped_to_the_session_it_names() {
    let router = router();
    let (first_id, first_arn) = create_session(&router, json!({ "n": "1" })).await;
    let (second_id, second_arn) = create_session(&router, json!({ "n": "2" })).await;

    // Interleaved creates, so a listing that ignored its scope would show the other
    // session's invocations in between its own.
    let mut first_ids = vec![create_invocation(&router, &first_id, json!({})).await];
    let mut second_ids = vec![create_invocation(&router, &second_id, json!({})).await];
    first_ids.push(create_invocation(&router, &first_arn, json!({})).await);
    second_ids.push(create_invocation(&router, &second_arn, json!({})).await);
    first_ids.push(create_invocation(&router, &first_id, json!({})).await);

    // (case, the identifier to list, the ids it must report)
    let cases: Vec<(&str, &str, &Vec<String>)> = vec![
        ("first session by bare id", first_id.as_str(), &first_ids),
        ("first session by ARN", first_arn.as_str(), &first_ids),
        ("second session by bare id", second_id.as_str(), &second_ids),
        ("second session by ARN", second_arn.as_str(), &second_ids),
    ];
    for (case, identifier, expected) in cases {
        let (seen, _) = drain_invocations(&router, identifier, 2).await;
        assert_eq!(
            &seen, expected,
            "case {case:?}: the listing must report exactly this session's invocations, got \
             {seen:?}"
        );
    }
}

/// RULE: `maxResults` pages at the value the model DEFAULTS it to when the member is
/// absent, not at the type's zero. The rows are the member's presence: omitted, and
/// set explicitly above and below that default.
#[tokio::test]
async fn an_omitted_max_results_pages_at_the_modelled_default() {
    use service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::state::DEFAULT_LIST_PAGE;

    // The literal the model declares for `maxResults`
    // (`smithy_query(query="operation",operation="ListInvocations")` -> `default: 10`),
    // asserted against the constant the handler actually pages at.
    const MODELLED_DEFAULT: i32 = 10;
    assert_eq!(
        DEFAULT_LIST_PAGE, MODELLED_DEFAULT,
        "the handler's default page size must be the one the model declares"
    );

    let stored = (MODELLED_DEFAULT as usize) + 2;
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;
    for n in 0..stored {
        create_invocation(
            &router,
            &session_id,
            json!({ "description": format!("invocation-{n}") }),
        )
        .await;
    }

    // (case, the maxResults to send, the page size due)
    let cases: [(&str, Option<i64>, usize); 3] = [
        ("omitted", None, MODELLED_DEFAULT as usize),
        (
            "below the default",
            Some(i64::from(MODELLED_DEFAULT) - 1),
            (MODELLED_DEFAULT as usize) - 1,
        ),
        (
            "above the default",
            Some(i64::from(MODELLED_DEFAULT) + 1),
            (MODELLED_DEFAULT as usize) + 1,
        ),
    ];
    for (case, max_results, expected) in cases {
        let (status, page) = list_invocations(&router, &session_id, max_results, None).await;
        assert_eq!(
            (status, summaries(&page).len()),
            (200, expected),
            "case {case:?}: expected a page of {expected}, got {status} {page:?}"
        );
        let expects_token = expected < stored;
        assert_eq!(
            page.get("nextToken").is_some_and(|t| !t.is_null()),
            expects_token,
            "case {case:?}: nextToken presence must be {expects_token}, got {page:?}"
        );
    }
}

/// RULE: `maxResults` is honoured across the whole range the model publishes for it
/// and refused outside it, as the modelled `ValidationException` (400). The rows are
/// each published bound and one step past each of them
/// (`smithy_query(query="operation",operation="ListInvocations")` -> `maxResults` has
/// `range {min: 1, max: 1000}`).
#[tokio::test]
async fn max_results_is_honoured_inside_the_published_range_and_refused_outside_it() {
    /// The model's published bounds for `maxResults`.
    const MIN: i64 = 1;
    /// See [`MIN`].
    const MAX: i64 = 1000;

    let stored = 3;
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;
    for _ in 0..stored {
        create_invocation(&router, &session_id, json!({})).await;
    }

    // (case, the maxResults query value, the accepted page size or None when the
    // value is outside the published range and must be refused)
    let cases: [(&str, i64, Option<usize>); 5] = [
        ("the published minimum", MIN, Some(1)),
        ("one below the published minimum", MIN - 1, None),
        // Below the minimum and distinct from the integer zero an absent member
        // arrives as, so it exercises the handler's own range check rather than only
        // the boundary validator's.
        ("negative, well below the published minimum", -1, None),
        ("the published maximum", MAX, Some(stored)),
        ("one above the published maximum", MAX + 1, None),
    ];
    for (case, value, accepted) in cases {
        let (status, body) = list_invocations(&router, &session_id, Some(value), None).await;
        match accepted {
            Some(size) => assert_eq!(
                (status, body["invocationSummaries"].as_array().map(Vec::len)),
                (200, Some(size)),
                "case {case:?}: maxResults={value} must page at {size}, got {status} {body:?}"
            ),
            None => assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case {case:?}: maxResults={value} is outside the published range and must be \
                 refused, got {status} {body:?}"
            ),
        }
    }
}

/// RULE: a pagination cursor belongs to the listing that issued it, so a token that
/// this listing did not mint is refused as the modelled `ValidationException` (400)
/// rather than silently resuming at an unrelated offset. The rows span every way a
/// token can fail to belong: not a token at all, one minted for another session, and
/// one minted by another listing of the SAME session.
#[tokio::test]
async fn a_pagination_token_is_only_honoured_by_the_listing_that_issued_it() {
    let router = router();
    let (session_a, _) = create_session(&router, json!({ "n": "a" })).await;
    let (session_b, _) = create_session(&router, json!({ "n": "b" })).await;

    let a_first = create_invocation(&router, &session_a, json!({})).await;
    let a_second = create_invocation(&router, &session_a, json!({})).await;
    create_invocation(&router, &session_b, json!({})).await;
    create_invocation(&router, &session_b, json!({})).await;
    put_step(
        &router,
        &session_a,
        &a_first,
        1_783_501_811,
        &json!({ "contentBlocks": [{ "text": "one" }] }),
    )
    .await;
    put_step(
        &router,
        &session_a,
        &a_first,
        1_783_501_812,
        &json!({ "contentBlocks": [{ "text": "two" }] }),
    )
    .await;

    // The genuine first page of session A's invocations, and of its steps.
    let (status, page) = list_invocations(&router, &session_a, Some(1), None).await;
    let a_token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a nextToken, got {status} {page:?}"))
        .to_string();
    let (status, b_page) = list_invocations(&router, &session_b, Some(1), None).await;
    let b_token = b_page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a nextToken, got {status} {b_page:?}"))
        .to_string();
    let (status, steps_page) = call(
        &router,
        "POST",
        &format!("/sessions/{session_a}/invocationSteps/?maxResults=1"),
        json!({}),
    )
    .await;
    let steps_token = steps_page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a nextToken, got {status} {steps_page:?}"))
        .to_string();

    // (case, the token to replay on session A's invocation listing)
    let cases: Vec<(&str, String)> = vec![
        ("not a token this service minted", "bm90LWEtdG9rZW4".to_string()),
        ("not even base64", "!!!!".to_string()),
        ("a token minted for another session", b_token),
        (
            "a token minted by the step listing of the same session",
            steps_token,
        ),
    ];
    for (case, token) in cases {
        let (status, refused) =
            list_invocations(&router, &session_a, Some(1), Some(&token)).await;
        assert_eq!(
            (status, error_code(&refused)),
            (400, "ValidationException"),
            "case {case:?}: a token this listing did not issue must be refused, got {status} \
             {refused:?}"
        );
    }

    // The genuine token still resumes exactly where its page stopped.
    let (status, resumed) = list_invocations(&router, &session_a, Some(1), Some(&a_token)).await;
    assert_eq!(
        (status, summaries(&resumed)[0]["invocationId"].as_str()),
        (200, Some(a_second.as_str())),
        "the token this listing issued must resume after {a_first}, got {status} {resumed:?}"
    );
}

/// RULE: a `sessionIdentifier` that names no stored session is the modelled
/// `ResourceNotFoundException` (404). The rows span every form an identifier can take
/// without naming a session this emulator holds.
#[tokio::test]
async fn listing_an_unknown_session_is_a_modelled_404() {
    let router = router();
    let (session_id, _) = create_session(&router, Value::Null).await;
    create_invocation(&router, &session_id, json!({})).await;

    let absent = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    // (case, the identifier to list)
    let cases: Vec<(&str, String)> = vec![
        ("a bare uuid no session carries", absent.to_string()),
        (
            "an ARN for a session that was never created",
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:session/{absent}"),
        ),
        (
            "a well-formed ARN for another account",
            format!("arn:aws:bedrock:{REGION}:123456789012:session/{session_id}"),
        ),
    ];
    for (case, identifier) in cases {
        let (status, refused) = list_invocations(&router, &identifier, None, None).await;
        assert_eq!(
            (status, error_code(&refused)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {refused:?}"
        );
    }

    // A deleted session stops being listable too.
    let (status, _) = call(
        &router,
        "PATCH",
        &format!("/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200, "EndSession: expected 200, got {status}");
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/sessions/{session_id}/"),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteSession: expected 200, got {status} {deleted:?}"
    );
    let (status, refused) = list_invocations(&router, &session_id, None, None).await;
    assert_eq!(
        (status, error_code(&refused)),
        (404, "ResourceNotFoundException"),
        "case \"a deleted session\": expected 404 ResourceNotFoundException, got {status} \
         {refused:?}"
    );
}

/// RULE: `ListInvocations` is `@readonly` — it reports stored state, so an ended
/// session still lists the invocations it holds, a create the service refused added
/// nothing to that listing, and repeating the call changes nothing a later caller
/// sees.
#[tokio::test]
async fn listing_reports_stored_state_and_changes_nothing() {
    let router = router();
    let (session_id, _) = create_session(&router, json!({ "topic": "demo" })).await;
    let before = [
        create_invocation(&router, &session_id, json!({})).await,
        create_invocation(&router, &session_id, json!({})).await,
    ];

    let (status, first_read) = list_invocations(&router, &session_id, None, None).await;
    assert_eq!(status, 200, "ListInvocations: expected 200, got {status}");
    for attempt in 1..=3 {
        let (status, again) = list_invocations(&router, &session_id, None, None).await;
        assert_eq!(
            (status, &again),
            (200, &first_read),
            "repeat {attempt}: a read must not change what a later caller sees, got {status} \
             {again:?}"
        );
    }

    // End the session: the content stays readable.
    let (status, ended) = call(
        &router,
        "PATCH",
        &format!("/sessions/{session_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, ended["sessionStatus"].as_str()),
        (200, Some("ENDED")),
        "EndSession: expected 200 ENDED, got {status} {ended:?}"
    );
    let (status, after_end) = list_invocations(&router, &session_id, None, None).await;
    assert_eq!(
        (status, &after_end),
        (200, &first_read),
        "an ENDED session must still list the invocations it holds, got {status} {after_end:?}"
    );

    // A create the ended session refuses adds nothing to the listing.
    let (status, refused) = call(
        &router,
        "PUT",
        &format!("/sessions/{session_id}/invocations/"),
        json!({}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (409, "ConflictException"),
        "CreateInvocation on an ENDED session must be refused, got {status} {refused:?}"
    );
    let (status, after_refusal) = list_invocations(&router, &session_id, None, None).await;
    let seen: Vec<&str> = summaries(&after_refusal)
        .iter()
        .filter_map(|summary| summary["invocationId"].as_str())
        .collect();
    assert_eq!(
        (status, seen),
        (200, before.iter().map(String::as_str).collect::<Vec<&str>>()),
        "a refused create must leave the listing exactly as it was, got {after_refusal:?}"
    );
}
