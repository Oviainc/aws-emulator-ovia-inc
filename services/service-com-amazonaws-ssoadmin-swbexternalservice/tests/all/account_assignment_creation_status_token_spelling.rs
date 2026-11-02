// SPDX-License-Identifier: CC0-1.0

//! The SPELLING a `NextToken` of a FILTER-SCOPED enumeration has to be in, driven end to
//! end over the `awsJson1_1` wire on `ListAccountAssignmentCreationStatus`.
//!
//! THE RULE: a paginated read resumes only from a cursor byte for byte as this service
//! minted it — the decoder's acceptance set is the encoder's output set and nothing wider
//! — so every other value, a respelling of an issued cursor included, is refused the
//! modelled `ValidationException` (HTTP 400) carrying the unissued-cursor message rather
//! than answered with a page. The encoding is hex, whose digits this service writes in
//! lower case, so a respelling is an upper- or mixed-case one; the model's `Token`
//! (`^[-a-zA-Z0-9+=/_]*$`) admits those characters, so such a value reaches the handler
//! rather than being stopped by the constraint walk, and a page positioned from a value
//! this service never handed out would let a client fabricate a cursor.
//!
//! `page_token_canonical_spelling.rs` asserts the same rule over an unscoped catalog, the
//! account's own listing and a listing scoped to one instance. The rows here are the
//! enumerations whose cursor also carries the FILTER that selected the rows
//! (`Filter$Status`, one of the model's `StatusValues`): the unfiltered walk of an
//! instance's creation history, and the walk under each status a tracked creation can be
//! in. They are crossed with the respellings `hex` reads but never writes (every digit
//! upper-cased, and a single digit of each end upper-cased), and each row also asserts the
//! cursor as MINTED still resumes its enumeration at the right row, so the acceptance set
//! is the encoder's output set and not an empty one.
//!
//! The history is built from the table `account_assignment_delete.rs` owns (both
//! `PrincipalType` values, three forms of `PrincipalId`, both ends of `TargetId`'s
//! `^\d{12}$`) under two permission sets, with the stored completion instants set on
//! either side of the clock so the `IN_PROGRESS` and the `SUCCEEDED` walks both hold more
//! than one row at once. Those instants are moved in the store rather than slept for; each
//! status is still derived by the service, exactly as in production.
//!
//! The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::account_assignment_delete::{grant, permission_set, CASES};
use crate::page_token_canonical_spelling::respellings;
use crate::ssoadmin_permission_sets::{
    call, error_code, organization_engine, router_over, with_store,
};

/// The operation under test, as `X-Amz-Target` names it.
const LIST: &str = "ListAccountAssignmentCreationStatus";

/// The permission sets the history below is granted under (`Name`: `^[\w+=,.@-]+$`, 1..32).
const PERMISSION_SET_NAMES: [&str; 2] = ["TokenSpellingStatusA", "TokenSpellingStatusB"];

/// The filter scopes the rule is asserted over: the unfiltered walk, and the walk under
/// each `StatusValues` a tracked creation of this emulator can be in.
///
/// The enum's third value, `FAILED`, is left out: nothing here fails an assignment
/// operation, so that walk holds no row and mints no cursor to respell.
const FILTERS: [Option<&str>; 3] = [None, Some("IN_PROGRESS"), Some("SUCCEEDED")];

/// One `ListAccountAssignmentCreationStatus` call, as a client makes it.
async fn list(
    router: &MultiServiceRouter,
    instance_arn: &str,
    filter: Option<&str>,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({"InstanceArn": instance_arn});
    if let Some(status) = filter {
        body["Filter"] = json!({"Status": status});
    }
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["NextToken"] = json!(next_token);
    }
    call(router, LIST, &body).await
}

/// The rows one filter's walk reports in one page of up to `MAX_RESULTS_CEILING`.
async fn rows_under(
    router: &MultiServiceRouter,
    instance_arn: &str,
    filter: Option<&str>,
) -> Vec<Value> {
    let (status, body) = list(router, instance_arn, filter, None, None).await;
    assert_eq!(
        status, 200,
        "case {filter:?}: the unpaged walk expected 200, got {status} {body}"
    );
    body["AccountAssignmentsCreationStatus"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Move the stored completion instant of ONE tracked creation, so the status the clock
/// implies for it is the one a row needs. The status itself is still derived by the
/// service.
fn hold_completion(engine: &Arc<engine::InMemoryEngine>, request_id: &str, due_at: SystemTime) {
    with_store(engine, |store| {
        store
            .assignment_operations
            .get_mut(request_id)
            .unwrap_or_else(|| panic!("no stored creation record under {request_id:?}"))
            .succeeded_due = due_at;
    });
}

/// An instance whose creation history holds more than one row in EACH of [`FILTERS`]:
/// every row of the shared table granted under the first permission set and the first row
/// under the second as well, with the earlier half of the resulting records still in
/// flight and the rest completed.
async fn instance_with_a_mixed_history() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter, String)
{
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let mut sets = Vec::new();
    for name in PERMISSION_SET_NAMES {
        sets.push(permission_set(&router, &instance_arn, name).await);
    }

    let mut request_ids = Vec::new();
    for case in CASES {
        request_ids.push(grant(&router, &instance_arn, &sets[0], case).await);
    }
    // The same principal on ANOTHER permission set is another assignment, so it is tracked
    // as its own creation.
    request_ids.push(grant(&router, &instance_arn, &sets[1], &CASES[0]).await);

    let in_flight = SystemTime::now() + Duration::from_secs(3_600);
    let completed = SystemTime::now() - Duration::from_secs(3_600);
    let still_in_flight = request_ids.len() / 2;
    for (at, request_id) in request_ids.iter().enumerate() {
        let due_at = if at < still_in_flight {
            in_flight
        } else {
            completed
        };
        hold_completion(&engine, request_id, due_at);
    }
    (engine, router, instance_arn)
}

/// THE RULE: a respelling of an issued cursor is a value this service never handed out, so
/// the enumeration refuses it instead of positioning a page from it — under every filter
/// scope, since the cursor carries that scope.
#[tokio::test]
async fn a_respelled_cursor_is_refused_under_every_filter() {
    let (_engine, router, instance_arn) = instance_with_a_mixed_history().await;

    for filter in FILTERS {
        let rows = rows_under(&router, &instance_arn, filter).await;
        assert!(
            rows.len() >= 2,
            "case {filter:?}: this walk must hold more than one row to mint a cursor, got \
             {rows:?}"
        );

        let (status, first_page) = list(&router, &instance_arn, filter, Some(1), None).await;
        assert_eq!(
            status, 200,
            "case {filter:?}: the first page expected 200, got {status} {first_page}"
        );
        let issued = first_page["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {filter:?}: this walk must mint a cursor: {first_page}"))
            .to_string();
        // The encoder's output set: lower-case hex digits only, which is what makes any
        // other spelling a value this service never issued.
        assert!(
            !issued.is_empty()
                && issued
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "case {filter:?}: a minted cursor is lower-case hex, got {issued:?}"
        );

        let respelled_rows = respellings(&issued);
        assert_eq!(
            respelled_rows.len(),
            3,
            "case {filter:?}: every respelling row must differ from the minted cursor \
             {issued:?}, got {respelled_rows:?}"
        );
        for (label, respelled) in respelled_rows {
            let case = format!("{filter:?} / {label}");
            let (status, body) =
                list(&router, &instance_arn, filter, Some(1), Some(&respelled)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for {respelled:?}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException for {respelled:?}, got {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(state::BAD_CURSOR_MESSAGE),
                "case {case:?}: expected the unissued-cursor refusal, got {body}"
            );
        }
    }
}

/// The other half of the same rule: the cursor as MINTED resumes its own walk at the row
/// after the page that issued it, so the rows above are refused because they are
/// respellings and not because the enumeration stopped paging.
#[tokio::test]
async fn the_cursor_as_minted_resumes_each_filters_walk() {
    let (_engine, router, instance_arn) = instance_with_a_mixed_history().await;

    for filter in FILTERS {
        let rows = rows_under(&router, &instance_arn, filter).await;
        let (status, first_page) = list(&router, &instance_arn, filter, Some(1), None).await;
        assert_eq!(
            status, 200,
            "case {filter:?}: the first page expected 200, got {status} {first_page}"
        );
        assert_eq!(
            first_page["AccountAssignmentsCreationStatus"],
            json!([rows[0]]),
            "case {filter:?}: the first page is the walk's first row, got {first_page}"
        );
        let issued = first_page["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {filter:?}: this walk must mint a cursor: {first_page}"))
            .to_string();

        let (status, resumed) = list(&router, &instance_arn, filter, Some(1), Some(&issued)).await;
        assert_eq!(
            status, 200,
            "case {filter:?}: the minted cursor expected 200, got {status} {resumed}"
        );
        assert_eq!(
            resumed["AccountAssignmentsCreationStatus"],
            json!([rows[1]]),
            "case {filter:?}: the resumed page is the walk's second row {}, got {resumed}",
            rows[1]
        );
    }
}
