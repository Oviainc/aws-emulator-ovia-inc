// SPDX-License-Identifier: CC0-1.0

//! `ListReportGroups`, over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.ListReportGroups` on awsJson1_1.
//!
//! RULE: the listing reports the ARNs of exactly the report groups the account holds —
//! every one `CreateReportGroup` wrote and none `DeleteReportGroup` removed — ordered by
//! the criterion and direction the request asks for, cut into pages of at most
//! `maxResults` entries that `nextToken` resumes; and the read leaves the store exactly
//! as it found it.
//!
//! The harness (`call`, `emulator_on`, `error_type`, `REGION`) is the one
//! `project_build_lifecycle` already drives this service with; the report groups under
//! test are seeded with `list_shared_report_groups`'s `create_report_groups` (itself built
//! on `report_group_lifecycle`'s request builder) and its answer is read with that suite's
//! `listed`, because both report-group listings state their result in a `reportGroups`
//! member of the same shape.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::list_shared_report_groups::{create_report_groups, listed};
use super::project_build_lifecycle::{call, emulator_on, error_type, REGION};

/// Call `ListReportGroups` with `request`, asserting it succeeded, and hand back the body.
async fn list(router: &MultiServiceRouter, request: Value) -> Value {
    let (status, body) = call(router, "ListReportGroups", request.clone()).await;
    assert_eq!(
        status, 200,
        "ListReportGroups {request}: expected 200, got {status} with {body}"
    );
    body
}

/// RULE: exactly the report groups the account holds are listed — the listing reflects
/// every creation and every deletion, and reports an empty list when the account holds
/// none.
#[tokio::test]
async fn the_listing_reflects_exactly_the_report_groups_the_account_holds() {
    // The rows differ along how many report groups the account holds and how many of
    // them were then deleted. `deleted` indexes into `names`.
    let cases: [(&str, &[&str], &[usize]); 5] = [
        ("an account holding nothing", &[], &[]),
        ("one group", &["alpha"], &[]),
        ("one group, then deleted", &["alpha"], &[0]),
        ("three groups", &["alpha", "bravo", "charlie"], &[]),
        (
            "three groups, the middle one deleted",
            &["alpha", "bravo", "charlie"],
            &[1],
        ),
    ];

    for (case, names, deleted) in cases {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let arns = create_report_groups(&router, names).await;
        for index in deleted {
            let (status, body) = call(
                &router,
                "DeleteReportGroup",
                json!({"arn": arns[*index].clone()}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteReportGroup must succeed, got {status} with {body}"
            );
        }

        let mut expected: Vec<String> = arns
            .iter()
            .enumerate()
            .filter(|(index, _)| !deleted.contains(index))
            .map(|(_, arn)| arn.clone())
            .collect();
        expected.sort();

        let body = list(&router, json!({})).await;
        assert_eq!(
            listed(&body, case),
            expected,
            "case {case:?}: expected the held report groups {expected:?}, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: one page holds every entry, so no nextToken may be stated, got {body}"
        );

        // The read is reporting only: a second call answers the same thing and the store
        // is untouched.
        let again = list(&router, json!({})).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read must answer the same listing, got {again}"
        );
        let store = engine.state::<CodeBuildState>();
        let held = store.lock();
        assert_eq!(
            held.report_groups.len(),
            expected.len(),
            "case {case:?}: a read must neither add to nor drop from the store"
        );
    }
}

/// RULE: the listing is ordered by the requested criterion in the requested direction —
/// by name, by when each report group was created or by when it was last changed,
/// ascending or descending — and an absent member takes the ascending-by-name default.
#[tokio::test]
async fn the_listing_is_ordered_by_the_requested_criterion_and_direction() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let names = ["ord-a", "ord-b", "ord-c", "ord-d"];
    create_report_groups(&router, &names).await;

    // The instants each group was created and last changed at, seeded as fixed values
    // rather than taken from the clock, so all three criteria DISAGREE and each row below
    // can only pass for its own criterion. `ord-c` and `ord-d` share a created instant,
    // which is what the name tie-break is for.
    let stamps = [
        // (name, created, lastModified)
        ("ord-a", 1_700_000_300_000_i64, 1_700_000_900_000_i64),
        ("ord-b", 1_700_000_100_000, 1_700_000_800_000),
        ("ord-c", 1_700_000_200_000, 1_700_000_600_000),
        ("ord-d", 1_700_000_200_000, 1_700_000_700_000),
    ];
    {
        let store = engine.state::<CodeBuildState>();
        let mut held = store.lock();
        for (name, created, last_modified) in stamps {
            let stored = held
                .report_groups
                .get_mut(name)
                .unwrap_or_else(|| panic!("the store holds {name:?} after CreateReportGroup"));
            stored.created_millis = created;
            stored.report_group.created = Some(state::datetime(created));
            stored.last_modified_millis = last_modified;
            stored.report_group.last_modified = Some(state::datetime(last_modified));
        }
    }

    let arn = |name: &str| state::report_group_arn(REGION, name);
    let by_name: Vec<String> = {
        let mut sorted: Vec<String> = names.iter().map(|name| arn(name)).collect();
        sorted.sort();
        sorted
    };
    let ordered_by = |key: fn(&(&str, i64, i64)) -> (i64, String)| -> Vec<String> {
        let mut stamped: Vec<((i64, String), String)> = stamps
            .iter()
            .map(|entry| (key(entry), arn(entry.0)))
            .collect();
        stamped.sort();
        stamped.into_iter().map(|(_, arn)| arn).collect()
    };
    let by_created = ordered_by(|&(name, created, _)| (created, name.to_string()));
    let by_last_modified = ordered_by(|&(name, _, last_modified)| (last_modified, name.to_string()));
    assert!(
        by_name != by_created && by_name != by_last_modified && by_created != by_last_modified,
        "the three criteria must disagree for this table to mean anything: \
         {by_name:?} / {by_created:?} / {by_last_modified:?}"
    );

    let descending = |order: &[String]| -> Vec<String> {
        let mut reversed = order.to_vec();
        reversed.reverse();
        reversed
    };

    // The rows span every value of `ReportGroupSortByType` (CREATED_TIME |
    // LAST_MODIFIED_TIME | NAME, from
    // `smithy_query(query="shape",shape="ReportGroupSortByType")`) and both of
    // `SortOrderType` (ASCENDING | DESCENDING), plus each member absent.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("neither member", json!({}), by_name.clone()),
        (
            "NAME, ascending",
            json!({"sortBy": "NAME", "sortOrder": "ASCENDING"}),
            by_name.clone(),
        ),
        (
            "NAME, descending",
            json!({"sortBy": "NAME", "sortOrder": "DESCENDING"}),
            descending(&by_name),
        ),
        ("NAME, no order", json!({"sortBy": "NAME"}), by_name.clone()),
        (
            "CREATED_TIME, ascending",
            json!({"sortBy": "CREATED_TIME", "sortOrder": "ASCENDING"}),
            by_created.clone(),
        ),
        (
            "CREATED_TIME, descending",
            json!({"sortBy": "CREATED_TIME", "sortOrder": "DESCENDING"}),
            descending(&by_created),
        ),
        (
            "CREATED_TIME, no order",
            json!({"sortBy": "CREATED_TIME"}),
            by_created.clone(),
        ),
        (
            "LAST_MODIFIED_TIME, ascending",
            json!({"sortBy": "LAST_MODIFIED_TIME", "sortOrder": "ASCENDING"}),
            by_last_modified.clone(),
        ),
        (
            "LAST_MODIFIED_TIME, descending",
            json!({"sortBy": "LAST_MODIFIED_TIME", "sortOrder": "DESCENDING"}),
            descending(&by_last_modified),
        ),
        (
            "LAST_MODIFIED_TIME, no order",
            json!({"sortBy": "LAST_MODIFIED_TIME"}),
            by_last_modified.clone(),
        ),
        (
            "only an order",
            json!({"sortOrder": "DESCENDING"}),
            descending(&by_name),
        ),
    ];

    for (case, request, expected) in rows {
        let body = list(&router, request.clone()).await;
        assert_eq!(
            listed(&body, case),
            expected,
            "case {case:?} ({request}): expected {expected:?}, got {body}"
        );
    }
}

/// RULE: the listing is cut into pages of at most `maxResults` entries, `nextToken`
/// resumes the next page, the last page states none, and walking the pages yields the
/// whole ordering exactly once.
#[tokio::test]
async fn the_listing_pages_by_max_results_and_resumes_at_the_next_token() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    let names: Vec<String> = (0..5).map(|index| format!("paged-{index}")).collect();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    let arns = create_report_groups(&router, &borrowed).await;
    let mut expected = arns.clone();
    expected.sort();

    // The sizes span one entry per page, a size that divides the count, one that does
    // not, the exact count, and `PageSize`'s published maximum of 100 (from
    // `smithy_query(query="operation",operation="ListReportGroups")`: @range(min: 1,
    // max: 100)), where one page holds everything.
    for page_size in [1_usize, 2, 3, 5, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let (Value::Object(target), Some(token)) = (&mut request, token.as_ref()) {
                target.insert("nextToken".to_string(), json!(token));
            }
            let case = format!("maxResults {page_size}, page {pages}");
            let body = list(&router, request).await;
            let page = listed(&body, &case);
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case {case:?}: a page holds 1..={page_size} entries, got {body}"
            );
            walked.extend(page);
            pages += 1;
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= expected.len(),
                "case {case:?}: paging must terminate, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, expected,
            "case {page_size}: walking the pages must yield every report group once, in \
             order, got {walked:?}"
        );
        assert_eq!(
            pages,
            expected.len().div_ceil(page_size),
            "case {page_size}: unexpected page count for {} entries",
            expected.len()
        );
    }

    // The token is the entry the page opens at, so the one the FIRST page reported
    // resumes at the second entry.
    let first = list(&router, json!({"maxResults": 1})).await;
    let resumed = list(
        &router,
        json!({"maxResults": 1, "nextToken": first["nextToken"]}),
    )
    .await;
    assert_eq!(
        listed(&resumed, "resumed"),
        vec![expected[1].clone()],
        "the reported token must resume at the next entry, got {resumed}"
    );

    // A token naming nothing in the listing is a bad input, not an empty page.
    for bogus in [
        "not-a-token".to_string(),
        state::report_group_arn(REGION, "never-created"),
        state::report_group_arn("eu-west-1", "paged-0"),
    ] {
        let (status, body) = call(
            &router,
            "ListReportGroups",
            json!({"nextToken": bogus.clone()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: an unresolvable nextToken is refused, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case {bogus:?}: CodeBuild states a bad input as InvalidInputException, got {body}"
        );
    }

    // A `maxResults` outside `PageSize`'s published range is refused — each bound is
    // accepted and the value just past it is not.
    for size in [1, 100] {
        let body = list(&router, json!({"maxResults": size})).await;
        assert!(
            !listed(&body, "a published bound").is_empty(),
            "case maxResults {size}: a published bound is accepted, got {body}"
        );
    }
    for size in [0, 101] {
        let (status, body) = call(&router, "ListReportGroups", json!({"maxResults": size})).await;
        assert_eq!(
            status, 400,
            "case maxResults {size}: a value past the modelled range is refused, got \
             {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "InvalidInputException",
            "case maxResults {size}: CodeBuild states a bad input as InvalidInputException, \
             got {body}"
        );
    }
}
