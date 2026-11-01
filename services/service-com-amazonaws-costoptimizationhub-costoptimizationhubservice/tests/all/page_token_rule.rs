// SPDX-License-Identifier: CC0-1.0

//! The ONE pagination-token rule of this service, driven end-to-end over the
//! wire (`awsJson1_0`) through the same multi-service router the emulator boots,
//! and through EVERY paginated read it serves.
//!
//! The rule: a paginated read starts at the row its `nextToken` names — no token
//! at all asking for the first row — and a token that read's own list did not
//! issue names no row to resume at, so it is refused with HTTP 400
//! `ValidationException` attributed to `nextToken`, rather than silently served
//! the first page again.
//!
//! Every paginated operation of this service judges a token the same way, so the
//! table below spans the operations (each of which used to carry its own copy of
//! the branch and the refusal) against the forms a token can take: absent,
//! issued by this list, issued by ANOTHER list of the same service, and four
//! kinds of value this service never issued. The two answers a case asserts are
//! the whole refusal body (status, code, `reason`, `message` and the single
//! `fields` entry, which differ between operations only in the operation named)
//! and, for an accepted token, the rows the page resumes at.
//!
//! A token's ROW is part of what the list issued: a list hands a token out only
//! for a page that has rows before it and rows after it, so the rows it ever
//! resumes at are the ones inside the collection being read and never its first.
//! The suite therefore spans, for every paginated read, the rows a token can
//! name — the first row, one past the last, further past it, and a row inside
//! the collection — including a token this service really issued for a WIDER
//! scope of the same list and then presented to a narrower one.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{
    call, fields, management_account_router_with_store, ok, record_enrollment,
    FIELD_VALIDATION_FAILED, FIELD_VALIDATION_SUMMARY,
};

/// Member accounts of the organization the store records: twelve digits, which
/// is what the `AccountId` shape's `^[0-9]{12}$` pattern admits, and neither of
/// them the caller — recorded so the organization-wide enrollment read has more
/// than one row and so pages at all.
const MEMBER_ACTIVE: &str = "111122223333";
const MEMBER_INACTIVE: &str = "444455556666";

/// Every paginated read of this service whose rows are a list of records, with
/// the request that makes it answer more than one row (so a page size of one
/// issues a token) — the operations that each enforce the token rule.
///
/// `ListEfficiencyMetrics` is this service's fourth paginated read; it pages
/// over GROUPS of a time series rather than records, so the same rule is driven
/// through it by the `efficiency_metrics` suite instead of by this table.
fn paginated_reads() -> Vec<(&'static str, Value)> {
    vec![
        (
            "ListRecommendations",
            json!({"includeAllRecommendations": true}),
        ),
        (
            "ListRecommendationSummaries",
            json!({"groupBy": "ResourceType"}),
        ),
        (
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": true}),
        ),
    ]
}

/// A router whose every paginated read answers more than one row: the caller is
/// enrolled with its analysis already due (so the catalog is readable), and the
/// organization it manages has two member accounts beside it.
///
/// The records are written through the shared harness because this emulator has
/// no Organizations control plane, so no operation creates another account's
/// enrollment.
fn paged_router() -> MultiServiceRouter {
    let (router, store) = management_account_router_with_store();
    record_enrollment(&store, engine::emulator_account_id(), "Active", true);
    record_enrollment(&store, MEMBER_ACTIVE, "Active", true);
    record_enrollment(&store, MEMBER_INACTIVE, "Inactive", false);
    router
}

/// The request one case sends: the operation's base request with the page size
/// and the token, if any, the case is about.
fn request(base: &Value, max_results: Option<u32>, token: Option<&str>) -> Value {
    let mut request = base.clone();
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        request["nextToken"] = json!(token);
    }
    request
}

/// One served page: the rows it carries and the token that resumes after them.
async fn page(
    router: &MultiServiceRouter,
    operation: &str,
    base: &Value,
    max_results: Option<u32>,
    token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let payload = ok(router, operation, &request(base, max_results, token)).await;
    let items = payload["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {operation}: expected a list of items in {payload}"))
        .clone();
    let next = payload["nextToken"].as_str().map(str::to_string);
    (items, next)
}

/// The forms of `nextToken` a read never issued, so each is refused: a value
/// that is not the encoding this service's tokens are carried in, a well-formed
/// encoding of a body this service never writes, a token this service issues for
/// a list of another kind, the empty value, and one of the read's OWN tokens
/// with a byte appended.
fn foreign_tokens(own: &str) -> Vec<(String, String)> {
    vec![
        (
            "not the token encoding".to_string(),
            "not-a-token".to_string(),
        ),
        (
            "a well-formed encoding of a body this service never issues".to_string(),
            hex::encode("page=1"),
        ),
        (
            "a token for a list of another kind".to_string(),
            cost_optimization_hub::state::encode_page_token("no-such-list", 1),
        ),
        ("the empty value".to_string(), String::new()),
        (
            "this list's own token with a byte appended".to_string(),
            format!("{own}00"),
        ),
    ]
}

/// Assert `payload` is the refusal every paginated read of this service answers
/// a token it did not issue with: HTTP 400 `ValidationException` whose `message`
/// states the rejected member and which list would have issued an acceptable
/// token, `reason` = `FieldValidationFailed`, and one `fields` entry attributing
/// the same diagnosis to `nextToken`.
fn assert_refused(case: &str, operation: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    let code = payload["__type"].as_str().unwrap_or_default();
    assert!(
        code.ends_with("ValidationException"),
        "case {case:?}: expected a ValidationException, got {code:?} in {payload}"
    );
    let message = payload["message"].as_str().unwrap_or_default();
    let expected_message =
        format!("Invalid nextToken specified. The token was not issued by {operation}.");
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected {expected_message:?}, got {message:?} in {payload}"
    );
    assert_ne!(
        message, FIELD_VALIDATION_SUMMARY,
        "case {case:?}: expected the refusal to state the rule rather than the \
         field-validation summary, got {payload}"
    );
    let reason = payload["reason"].as_str().unwrap_or_default();
    assert_eq!(
        reason, FIELD_VALIDATION_FAILED,
        "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {reason:?} in {payload}"
    );
    let expected_fields = vec![(
        "nextToken".to_string(),
        format!("the value is not a pagination token issued by {operation}"),
    )];
    let answered = fields(payload);
    assert_eq!(
        answered, expected_fields,
        "case {case:?}: expected fields {expected_fields:?}, got {answered:?} in {payload}"
    );
}

/// Every paginated read refuses a `nextToken` it did not issue — whichever form
/// that value takes, including a token another list of the same service issued —
/// with the one refusal body, naming the list that would have issued an
/// acceptable token.
#[tokio::test]
async fn a_token_a_read_did_not_issue_is_refused_by_every_paginated_read() {
    let router = paged_router();
    let reads = paginated_reads();

    // One token genuinely issued by each list, so a case can hand a read a token
    // that is valid for a DIFFERENT list of the same service.
    let mut issued: Vec<(&str, String)> = Vec::new();
    for (operation, base) in &reads {
        let (items, token) = page(&router, operation, base, Some(1), None).await;
        assert_eq!(
            items.len(),
            1,
            "case {operation}: expected a page of one row, got {items:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {operation}: expected a token after a page of one row, got none")
        });
        issued.push((operation, token));
    }

    for (operation, base) in &reads {
        let own = &issued
            .iter()
            .find(|(named, _)| named == operation)
            .expect("every read issued a token above")
            .1;

        let mut refused: Vec<(String, String)> = foreign_tokens(own);
        for (other, token) in &issued {
            if other != operation {
                refused.push((format!("a token issued by {other}"), token.clone()));
            }
        }

        for (form, token) in &refused {
            let case = format!("{operation} given {form}");
            let (status, payload) =
                call(&router, operation, &request(base, Some(1), Some(token))).await;
            assert_refused(&case, operation, status, &payload);
        }
    }
}

/// The other half of the same rule: a read with no `nextToken` starts at the
/// first row, and a token the read itself issued resumes at the row the previous
/// page stopped at — so walking a list one row at a time yields exactly the rows
/// the unpaged read answers, in the same order, and the last page carries no
/// token.
#[tokio::test]
async fn an_absent_token_starts_at_the_first_row_and_an_issued_one_resumes() {
    let router = paged_router();

    for (operation, base) in paginated_reads() {
        let (whole, token) = page(&router, operation, &base, None, None).await;
        assert!(
            whole.len() > 1,
            "case {operation}: expected more than one row to page over, got {whole:?}"
        );
        assert!(
            token.is_none(),
            "case {operation}: a read of every row carries no resume token, got {token:?}"
        );

        // Walk the list a row at a time, bounded by the rows the unpaged read
        // answered plus one, so a token that never stops fails rather than hangs.
        let mut walked: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..=whole.len() {
            let (items, next) =
                page(&router, operation, &base, Some(1), cursor.as_deref()).await;
            walked.extend(items);
            match next {
                None => break,
                Some(next) => cursor = Some(next),
            }
        }
        assert_eq!(
            walked, whole,
            "case {operation}: expected paging to yield the unpaged rows {whole:?}, \
             got {walked:?}"
        );
    }
}

/// A `nextToken` in the encoding `issued` is carried in — the one the list that
/// issued `issued` writes its tokens in — naming `row` instead.
///
/// Built from a token the list really handed out, so a case states only the row
/// it is about and nothing about how this service encodes a token or which list
/// it belongs to.
fn token_at_row(issued: &str, row: usize) -> String {
    let decoded = hex::decode(issued).expect("an issued token is hex");
    let text = String::from_utf8(decoded).expect("an issued token decodes to text");
    let (head, _) = text
        .rsplit_once(':')
        .expect("an issued token carries the row it resumes at last");
    hex::encode(format!("{head}:{row}"))
}

/// A row of a paginated read is a row a token can name only when the list could
/// have issued a token for it: a list issues one only for a page with rows
/// before it and rows after it, so the first row and every row from the end of
/// the collection onwards were never issued, whatever else the token looks like.
/// Each is refused with the one refusal body, not answered with the empty final
/// page a completed listing answers.
#[tokio::test]
async fn a_token_naming_a_row_this_list_never_issued_is_refused_by_every_paginated_read() {
    let router = paged_router();

    for (operation, base) in paginated_reads() {
        let (whole, _) = page(&router, operation, &base, None, None).await;
        let rows = whole.len();
        assert!(
            rows > 1,
            "case {operation}: expected more than one row to page over, got {whole:?}"
        );

        // A token this list genuinely issued, so the cases differ from it only in
        // the row they name.
        let (_, issued) = page(&router, operation, &base, Some(1), None).await;
        let issued = issued.unwrap_or_else(|| {
            panic!("case {operation}: expected a token after a page of one row, got none")
        });

        let never_issued: Vec<(String, usize)> = vec![
            ("the first row of the collection".to_string(), 0),
            ("the row one past the last".to_string(), rows),
            ("two rows past the last".to_string(), rows + 1),
            ("a row far past the last".to_string(), rows + 4_000),
        ];
        for (form, row) in &never_issued {
            let case = format!("{operation} given a token naming {form}");
            let token = token_at_row(&issued, *row);
            let (status, payload) =
                call(&router, operation, &request(&base, None, Some(&token))).await;
            assert_refused(&case, operation, status, &payload);
            assert!(
                payload["items"].is_null(),
                "case {case:?}: expected a refusal rather than a page of rows, got {payload}"
            );
        }

        // The complement, from the same table: the last row a token of this list
        // can name is served, and it resumes exactly there — so the boundary is
        // the end of the collection and not the shape of any one request.
        let last = rows - 1;
        let case = format!("{operation} given a token naming its last resumable row");
        let (items, next) = page(
            &router,
            operation,
            &base,
            None,
            Some(&token_at_row(&issued, last)),
        )
        .await;
        assert_eq!(
            items,
            whole[last..].to_vec(),
            "case {case:?}: expected the rows from {last} on ({:?}), got {items:?}",
            &whole[last..]
        );
        assert!(
            next.is_none(),
            "case {case:?}: a page ending at the last row carries no token, got {next:?}"
        );
    }
}

/// The same rule holds for a token this service really issued: the rows it can
/// resume at are the rows of the collection the read it is presented to answers,
/// so a token the organization-wide enrollment read issued names no row of the
/// caller's own one-record read and is refused there, rather than answered with
/// the empty final page.
#[tokio::test]
async fn an_issued_token_naming_a_row_outside_the_read_it_is_given_to_is_refused() {
    let router = paged_router();
    let operation = "ListEnrollmentStatuses";
    let organization = json!({"includeOrganizationInfo": true});
    let caller_only = json!({"includeOrganizationInfo": false});

    // The organization's view pages over the caller and its two members, so its
    // second page resumes at a row the caller's own one-record view does not have.
    let (_, issued) = page(&router, operation, &organization, Some(2), None).await;
    let issued = issued.unwrap_or_else(|| {
        panic!("case {operation}: expected a token after two of three rows, got none")
    });

    let (own, _) = page(&router, operation, &caller_only, None, None).await;
    assert_eq!(
        own.len(),
        1,
        "case setup: the caller's own read answers its one record, got {own:?}"
    );

    let (status, payload) = call(
        &router,
        operation,
        &request(&caller_only, None, Some(&issued)),
    )
    .await;
    assert_refused(
        "ListEnrollmentStatuses given the organization view's token",
        operation,
        status,
        &payload,
    );
}
