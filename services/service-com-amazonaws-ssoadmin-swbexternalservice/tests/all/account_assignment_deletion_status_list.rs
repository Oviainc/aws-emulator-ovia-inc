// SPDX-License-Identifier: CC0-1.0

//! `ListAccountAssignmentDeletionStatus` — the enumeration of an instance's
//! assignment-deletion history — driven end to end over the `awsJson1_1` wire.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! example:
//!
//! 1. the enumeration reports exactly the deletions STORED for the instance the request
//!    names, one row per recorded deletion, oldest first, each row carrying the three
//!    members the model's `AccountAssignmentOperationStatusMetadata` declares — the status
//!    the clock implies, the `RequestId` the delete minted and the date it was accepted —
//!    and none of the detail members the status DESCRIBE reports, with the status and the
//!    date agreeing member for member with that describe;
//! 2. a page holds at most `MaxResults` rows and carries a `NextToken` only while rows are
//!    left, so paging through any page size the model admits walks the whole history once;
//! 3. `Filter$Status` selects exactly the rows whose status as of the call is that one of
//!    the model's `StatusValues`, so the filtered pages partition the unfiltered one;
//! 4. an `InstanceArn` the account holds no instance under is a missing resource, and a
//!    `NextToken` this enumeration did not issue — a token of another filter's enumeration
//!    included — is refused rather than resumed;
//! 5. the enumeration is a REPORT: it answers identically however often it is asked and
//!    consumes nothing, so the history, the assignments and the status describes are all
//!    exactly what they were before it.
//!
//! The rows differ along what the rules generalise over: the history is built from the
//! table `account_assignment_delete.rs` owns (both `PrincipalType` values, three forms of
//! `PrincipalId`, both ends of `TargetId`'s `^\d{12}$`), recorded under two permission
//! sets, enumerated at every page size from one row to the model's `MaxResults` ceiling
//! and with none at all, and read with the stored completion instants on either side of
//! the clock and MIXED across the history. Those instants are moved in the store rather
//! than slept for; each status is still derived by the service, exactly as in production.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, the one path a socket-served
//! request takes. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::account_assignment_delete::{
    assigned, assignment_body, describe, grant, hold_completion, permission_set, Case, CASES,
    DETAIL_MEMBERS,
};
use crate::ssoadmin_permission_sets::{
    call, error_code, organization_engine, router_over, with_store, UNKNOWN_INSTANCE_ARN,
};

/// The operation under test, as `X-Amz-Target` names it.
const LIST: &str = "ListAccountAssignmentDeletionStatus";

/// The model's `MaxResults` ceiling for this operation (`range 1..100`).
const MAX_RESULTS_CEILING: i64 = 100;

/// One `ListAccountAssignmentDeletionStatus` call, as a client makes it: the instance, and
/// the page size, cursor and status filter when the caller sends them.
async fn list(
    router: &MultiServiceRouter,
    instance_arn: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
    status_filter: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({"InstanceArn": instance_arn});
    if let Some(max_results) = max_results {
        body["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["NextToken"] = json!(next_token);
    }
    if let Some(status) = status_filter {
        body["Filter"] = json!({"Status": status});
    }
    call(router, LIST, &body).await
}

/// Every row of the enumeration, walked page by page the way a paginating client does.
///
/// Asserts rule 2 on the way through: no page holds more than `max_results` rows, and a
/// `NextToken` is carried only while the enumeration has more to give.
async fn enumerate(
    router: &MultiServiceRouter,
    instance_arn: &str,
    max_results: Option<i64>,
    status_filter: Option<&str>,
    case: &str,
) -> Vec<Value> {
    let limit = max_results.unwrap_or(MAX_RESULTS_CEILING);
    let mut rows: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // A bound on the walk, so a token that never clears fails as an assertion rather than
    // hanging the suite: one page per row the ceiling allows, plus the final empty page.
    for page in 1..=(MAX_RESULTS_CEILING + 2) {
        let (status, body) = list(
            router,
            instance_arn,
            max_results,
            token.as_deref(),
            status_filter,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {page} expected 200, got {status} {body}"
        );
        let page_rows = body["AccountAssignmentsDeletionStatus"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: page {page} carried no status list: {body}")
            })
            .clone();
        assert!(
            page_rows.len() as i64 <= limit,
            "case {case:?}: page {page} may hold at most {limit} rows, got {}",
            page_rows.len()
        );
        rows.extend(page_rows.iter().cloned());
        token = body["NextToken"].as_str().map(str::to_string);
        match &token {
            None => return rows,
            Some(_) => assert!(
                page_rows.len() as i64 == limit,
                "case {case:?}: page {page} carried a NextToken with only {} of {limit} rows \
                 filled, so the enumeration was already complete",
                page_rows.len()
            ),
        }
    }
    panic!("case {case:?}: the enumeration never ran out of pages after {} rows", rows.len())
}

/// The `RequestId` of every row of the enumeration, in the order it reported them.
async fn handles(
    router: &MultiServiceRouter,
    instance_arn: &str,
    max_results: Option<i64>,
    status_filter: Option<&str>,
    case: &str,
) -> Vec<String> {
    enumerate(router, instance_arn, max_results, status_filter, case)
        .await
        .iter()
        .map(|row| {
            row["RequestId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a row carried no RequestId: {row}"))
                .to_string()
        })
        .collect()
}

/// Grant one row under `permission_set_arn` and withdraw it again, returning the
/// deletion's `RequestId` — the handle the enumeration reports it by.
async fn withdraw(
    router: &MultiServiceRouter,
    instance_arn: &str,
    permission_set_arn: &str,
    case: &Case,
) -> String {
    grant(router, instance_arn, permission_set_arn, case).await;
    let (status, body) = call(
        router,
        "DeleteAccountAssignment",
        &assignment_body(instance_arn, permission_set_arn, case),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: DeleteAccountAssignment expected 200, got {status} {body}",
        case.label
    );
    body["AccountAssignmentDeletionStatus"]["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: the delete minted no RequestId: {body}", case.label))
        .to_string()
}

/// A history of deletions on a fresh organization instance: every row of the shared table
/// withdrawn under each of two permission sets, in the order they were recorded.
async fn recorded_history(
    router: &MultiServiceRouter,
    instance_arn: &str,
) -> Vec<String> {
    let first = permission_set(router, instance_arn, "FirstSet").await;
    let second = permission_set(router, instance_arn, "SecondSet").await;
    let mut history = Vec::new();
    for permission_set_arn in [&first, &second] {
        for case in CASES {
            history.push(withdraw(router, instance_arn, permission_set_arn, case).await);
        }
    }
    history
}

/// Move the stored completion instant of the deletion at each listed index to `due_at`,
/// leaving the rest where they are — so the history can be read with MIXED statuses. The
/// statuses themselves are still derived by the service, exactly as in production.
fn hold_completion_of(
    engine: &Arc<engine::InMemoryEngine>,
    indexes: &[usize],
    due_at: SystemTime,
) {
    with_store(engine, |store| {
        for index in indexes {
            let record = store
                .assignment_deletions
                .get_mut(*index)
                .unwrap_or_else(|| panic!("the history holds a deletion at index {index}"));
            record.succeeded_due = due_at;
        }
    });
}

/// RULE 1 — the enumeration reports the stored history of the instance, oldest first, in
/// the metadata projection, at the status the clock implies and agreeing with the describe.
#[tokio::test]
async fn the_enumeration_reports_every_stored_deletion_in_the_metadata_projection() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));

    // An instance under which nothing has been deleted: an empty list, not an error.
    let (status, body) = list(&router, &instance_arn, None, None, None).await;
    assert_eq!(status, 200, "an empty history expected 200, got {status} {body}");
    assert_eq!(
        body["AccountAssignmentsDeletionStatus"],
        json!([]),
        "case \"no deletion recorded\": expected an empty list, got {body}"
    );
    assert_eq!(
        body["NextToken"],
        Value::Null,
        "case \"no deletion recorded\": an empty page carries no NextToken, got {body}"
    );

    let history = recorded_history(&router, &instance_arn).await;

    // The clock table: a status is derived from the stored due instant, so the history is
    // enumerated with that instant well ahead of the call and behind it, and with it set
    // to the moment the row starts — the handler samples its own clock at or just after
    // that, and a completion that is already due has happened, so the status is terminal.
    let clock: [(&str, i64, &str); 4] = [
        ("an hour before the completions are due", 3_600_000, "IN_PROGRESS"),
        ("a second before the completions are due", 1_000, "IN_PROGRESS"),
        ("no more than a moment after they fell due", 0, "SUCCEEDED"),
        ("an hour after they fell due", -3_600_000, "SUCCEEDED"),
    ];
    for (when, offset_millis, expected_status) in clock {
        let span = Duration::from_millis(offset_millis.unsigned_abs());
        let due_at = if offset_millis >= 0 {
            SystemTime::now() + span
        } else {
            SystemTime::now() - span
        };
        hold_completion(&engine, due_at);

        let rows = enumerate(&router, &instance_arn, None, None, when).await;
        assert_eq!(
            rows.len(),
            history.len(),
            "case {when:?}: expected one row per recorded deletion ({}), got {} in {rows:?}",
            history.len(),
            rows.len()
        );
        for (index, row) in rows.iter().enumerate() {
            let case = format!("{when}, row {index}");
            // Oldest first: the order the deletions were recorded in.
            assert_eq!(
                row["RequestId"].as_str(),
                Some(history[index].as_str()),
                "case {case:?}: expected the handle {:?} recorded at this position, got {:?}",
                history[index],
                row["RequestId"]
            );
            assert_eq!(
                row["Status"].as_str(),
                Some(expected_status),
                "case {case:?}: expected status {expected_status:?}, got {:?}",
                row["Status"]
            );
            assert!(
                row["CreatedDate"].is_number(),
                "case {case:?}: expected the date the deletion was accepted, got {:?}",
                row["CreatedDate"]
            );
            // The METADATA projection: the three members above and nothing else — the
            // detail of the withdrawn access is what the status describe reports.
            for member in DETAIL_MEMBERS.iter().chain(["FailureReason"].iter()) {
                assert_eq!(
                    row[*member],
                    Value::Null,
                    "case {case:?}: the metadata projection carries no {member}, got {:?}",
                    row[*member]
                );
            }
            // The list and the describe read the same record, so they cannot disagree.
            let (status, described) = describe(&router, &instance_arn, &history[index]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the describe of the listed handle expected 200, got \
                 {status} {described}"
            );
            let described = &described["AccountAssignmentDeletionStatus"];
            for member in ["Status", "RequestId", "CreatedDate"] {
                assert_eq!(
                    row[member], described[member],
                    "case {case:?}: the list and the describe must agree on {member}, got \
                     {:?} and {:?}",
                    row[member], described[member]
                );
            }
        }
    }
}

/// RULE 2 — every page size the model admits walks the whole history once, and the page
/// sizes outside its range are refused.
#[tokio::test]
async fn every_page_size_the_model_admits_walks_the_whole_history_once() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let history = recorded_history(&router, &instance_arn).await;
    hold_completion(&engine, SystemTime::now() - Duration::from_secs(1));

    // Page sizes spanning the model's `MaxResults` range: its lower bound, several sizes
    // on both sides of the history's length, its upper bound, and no page size at all.
    let sizes: [Option<i64>; 8] = [
        Some(1),
        Some(2),
        Some(3),
        Some(history.len() as i64 - 1),
        Some(history.len() as i64),
        Some(history.len() as i64 + 1),
        Some(MAX_RESULTS_CEILING),
        None,
    ];
    for max_results in sizes {
        let case = format!("MaxResults {max_results:?}");
        let walked = handles(&router, &instance_arn, max_results, None, &case).await;
        assert_eq!(
            walked, history,
            "case {case:?}: expected the whole history once, in order, got {walked:?}"
        );
    }

    // Outside the modelled range: refused, never answered with a clamped page.
    for max_results in [0, -1, MAX_RESULTS_CEILING + 1] {
        let case = format!("MaxResults {max_results}");
        let (status, body) = list(&router, &instance_arn, Some(max_results), None, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a MaxResults outside the modelled range must be refused, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE 3 — the status filter selects exactly the rows in that status, so the filtered
/// enumerations partition the unfiltered one.
#[tokio::test]
async fn the_status_filter_selects_exactly_the_rows_in_that_status() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let history = recorded_history(&router, &instance_arn).await;

    // A MIXED history: the deletions at the even positions have completed, the rest are
    // still in flight, so neither filter can be satisfied by answering the whole history.
    let done: Vec<usize> = (0..history.len()).filter(|index| index % 2 == 0).collect();
    let in_flight: Vec<usize> = (0..history.len()).filter(|index| index % 2 == 1).collect();
    hold_completion_of(&engine, &done, SystemTime::now() - Duration::from_secs(1));
    hold_completion_of(&engine, &in_flight, SystemTime::now() + Duration::from_secs(3_600));

    let expected_succeeded: Vec<String> =
        done.iter().map(|index| history[*index].clone()).collect();
    let expected_in_progress: Vec<String> =
        in_flight.iter().map(|index| history[*index].clone()).collect();

    // Each value of the model's `StatusValues` enum, with the rows it must select. Nothing
    // fails an assignment operation, so `FAILED` selects none — an empty page, not an
    // error.
    let filters: [(&str, &Vec<String>); 3] = [
        ("SUCCEEDED", &expected_succeeded),
        ("IN_PROGRESS", &expected_in_progress),
        ("FAILED", &Vec::new()),
    ];
    for (status_filter, expected) in filters {
        // At two page sizes, so the filter is applied to the enumeration and not to the
        // page: a filter applied per page would lose rows as soon as one is split.
        for max_results in [None, Some(1)] {
            let case = format!("Filter {status_filter}, MaxResults {max_results:?}");
            let selected =
                handles(&router, &instance_arn, max_results, Some(status_filter), &case).await;
            assert_eq!(
                &selected, expected,
                "case {case:?}: expected exactly the rows in that status, got {selected:?}"
            );
        }
    }
    // The two filled filters partition the unfiltered enumeration.
    let unfiltered = handles(&router, &instance_arn, None, None, "unfiltered").await;
    assert_eq!(
        unfiltered, history,
        "the unfiltered enumeration is the whole history, got {unfiltered:?}"
    );
    assert_eq!(
        expected_succeeded.len() + expected_in_progress.len(),
        unfiltered.len(),
        "the filtered enumerations must partition the unfiltered one, got {} and {} of {}",
        expected_succeeded.len(),
        expected_in_progress.len(),
        unfiltered.len()
    );

    // A status outside the model's enum is refused rather than answered with an empty
    // page, which would read as "no deletion is in that state".
    for status_filter in ["COMPLETED", "succeeded", "IN-PROGRESS"] {
        let case = format!("Filter {status_filter}");
        let (status, body) = list(&router, &instance_arn, None, None, Some(status_filter)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a status outside the model's enum must be refused, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE 4 — an unknown instance is a missing resource, and a cursor this enumeration did
/// not issue is refused.
#[tokio::test]
async fn an_unknown_instance_and_a_foreign_cursor_are_refused() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let history = recorded_history(&router, &instance_arn).await;
    hold_completion(&engine, SystemTime::now() - Duration::from_secs(1));

    let (status, body) = list(&router, UNKNOWN_INSTANCE_ARN, None, None, None).await;
    assert_eq!(
        status, 400,
        "case \"an unknown InstanceArn\": expected a refusal, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case \"an unknown InstanceArn\": expected ResourceNotFoundException, got {body}"
    );

    // The cursor this enumeration DID issue, which resumes it.
    let (status, body) = list(&router, &instance_arn, Some(1), None, None).await;
    assert_eq!(status, 200, "the first page expected 200, got {status} {body}");
    let own_token = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page of one row out of {} carried no cursor: {body}", history.len()))
        .to_string();
    let (status, resumed) = list(&router, &instance_arn, Some(1), Some(&own_token), None).await;
    assert_eq!(
        status, 200,
        "the cursor this enumeration issued must resume it, got {status} {resumed}"
    );
    assert_eq!(
        resumed["AccountAssignmentsDeletionStatus"][0]["RequestId"].as_str(),
        Some(history[1].as_str()),
        "the resumed page must start after the first row, got {resumed}"
    );

    // Cursors this enumeration did not issue: a token of its own walk under a DIFFERENT
    // filter (the keys mean something else there), one issued for another instance's walk
    // and one that was never a token at all.
    let foreign: [(&str, &str); 3] = [
        ("the same walk under another filter", own_token.as_str()),
        ("a token this service never issued", "not-a-cursor"),
        ("an empty token", ""),
    ];
    for (case, token) in foreign {
        let (status, body) = list(
            &router,
            &instance_arn,
            Some(1),
            Some(token),
            // The first row's filter: the cursor above was issued for the UNFILTERED walk,
            // so under this one it belongs to another enumeration.
            Some("SUCCEEDED"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a cursor this enumeration did not issue must be refused, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }
}

/// RULE 5 — the enumeration is a REPORT: it answers the same pages however often it is
/// asked and changes nothing a later caller sees.
#[tokio::test]
async fn listing_deletion_statuses_changes_nothing_a_later_caller_sees() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let permission_set_arn = permission_set(&router, &instance_arn, "Listed").await;
    let withdrawn = CASES[0];
    let kept = CASES[1];
    assert_eq!(
        withdrawn.account_id, kept.account_id,
        "the two rows must share an account so one ListAccountAssignments call sees both"
    );

    let kept_creation_id = grant(&router, &instance_arn, &permission_set_arn, &kept).await;
    let request_id = withdraw(&router, &instance_arn, &permission_set_arn, &withdrawn).await;
    // The clock is the only thing that may move a status, so it is held still: any
    // difference between the readings below would then be the READ's doing.
    hold_completion(&engine, SystemTime::now() - Duration::from_secs(1));

    let rows_before = assigned(
        &router,
        &instance_arn,
        withdrawn.account_id,
        &permission_set_arn,
    )
    .await;
    let history_before = with_store(&engine, |store| store.assignment_deletions.len());

    let (status, first) = list(&router, &instance_arn, None, None, None).await;
    assert_eq!(status, 200, "the enumeration expected 200, got {status} {first}");
    for attempt in 2..=4 {
        let (status, again) = list(&router, &instance_arn, None, None, None).await;
        assert_eq!(status, 200, "attempt {attempt}: expected 200, got {status} {again}");
        assert_eq!(
            again, first,
            "attempt {attempt}: an enumeration must report the same page, expected {first}, \
             got {again}"
        );
    }

    let rows_after = assigned(
        &router,
        &instance_arn,
        withdrawn.account_id,
        &permission_set_arn,
    )
    .await;
    assert_eq!(
        rows_after, rows_before,
        "listing deletion statuses must leave the assignments untouched, expected \
         {rows_before:?}, got {rows_after:?}"
    );
    let history_after = with_store(&engine, |store| store.assignment_deletions.len());
    assert_eq!(
        history_after, history_before,
        "listing must consume no deletion record, expected {history_before} stored, got \
         {history_after}"
    );

    // The describe of the listed handle still answers, and so does the CREATION status of
    // the assignment that is still granted — another operation's history.
    let (status, described) = describe(&router, &instance_arn, &request_id).await;
    assert_eq!(
        status, 200,
        "the listed handle must still describe, got {status} {described}"
    );
    let (status, creation) = call(
        &router,
        "DescribeAccountAssignmentCreationStatus",
        &json!({
            "InstanceArn": instance_arn,
            "AccountAssignmentCreationRequestId": kept_creation_id,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the surviving assignment's creation status must still read, got {status} {creation}"
    );
    assert_eq!(
        creation["AccountAssignmentCreationStatus"]["PrincipalId"].as_str(),
        Some(kept.principal_id),
        "expected the granted access's PrincipalId {:?}, got {:?}",
        kept.principal_id,
        creation["AccountAssignmentCreationStatus"]["PrincipalId"]
    );
}
