// SPDX-License-Identifier: CC0-1.0

//! An optional `@httpQuery` page size the request does not carry is ABSENT.
//!
//! Every list operation of this service binds `MaxResults` as `@httpQuery("maxResults")`
//! against `com.amazonaws.dataexchange#MaxResults`, which the model gives
//! `@range(min: 1, max: 200)` AND `@default: 0` - a default outside the member's own
//! bound (`smithy_query(query="operation", operation="ListDataSets,ListDataSetRevisions,\
//! ListJobs,ListEventActions,ListRevisionAssets")` reports `"default": 0` with
//! `"range": {"min": 1, "max": 200}` on each). The member is not `@required`, so a request
//! may omit it, and then it names NO page size: the service serves its own page, and the
//! modelled range is checked only against a value the request actually carried.
//!
//! The two rules below are asserted over every one of this service's five `MaxResults`
//! members, so the fix cannot hold for the two operations the issue named and leak on the
//! other three. `ListDataGrants` / `ListReceivedDataGrants` are deliberately NOT in the
//! table: their own `MaxResults` members carry `"default": null`, i.e. no default at all,
//! so there is nothing for this rule to be about and the boundary walk keeps checking
//! their range.

use crate::data_sets::{call, create_data_set, emulator, Answer};
use smithy_server_runtime::MultiServiceRouter;

/// The published bounds of `com.amazonaws.dataexchange#MaxResults` (`@range(min: 1,
/// max: 200)`), and the value its `@default` would materialise to.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 200;
const MODELLED_DEFAULT: i64 = 0;

/// One list operation under test: where it lives, and what it answers when the request
/// is not refused about `maxResults`.
struct ListOperation {
    case: &'static str,
    path: String,
    /// 200 for a list operation this emulator implements; 501 for one still scaffolded,
    /// whose answer is about the OPERATION rather than about the request.
    served: u16,
    /// Whether this operation's handler enforces the modelled range itself. Only an
    /// implemented handler can: a scaffolded one answers 501 for every request, which
    /// says nothing about the page size.
    enforces_range: bool,
}

/// Every list operation of this service, over a data set and a revision that exist, so
/// nothing in the table answers 404 for a reason of its own.
async fn list_operations(router: &MultiServiceRouter) -> Vec<ListOperation> {
    let data_set_id = create_data_set(router, "paging", "p", "S3_SNAPSHOT", None)
        .await
        .string("Id");
    let revision_id = call(
        router,
        "POST",
        &format!("/v1/data-sets/{data_set_id}/revisions"),
        Some(serde_json::json!({})),
    )
    .await
    .string("Id");

    vec![
        ListOperation {
            case: "ListDataSets",
            path: "/v1/data-sets".to_string(),
            served: 200,
            enforces_range: true,
        },
        ListOperation {
            case: "ListDataSetRevisions",
            path: format!("/v1/data-sets/{data_set_id}/revisions"),
            served: 200,
            enforces_range: true,
        },
        ListOperation {
            case: "ListJobs",
            path: "/v1/jobs".to_string(),
            served: 501,
            enforces_range: false,
        },
        ListOperation {
            case: "ListEventActions",
            path: "/v1/event-actions".to_string(),
            served: 501,
            enforces_range: false,
        },
        ListOperation {
            case: "ListRevisionAssets",
            path: format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}/assets"),
            served: 501,
            enforces_range: false,
        },
    ]
}

/// `?maxResults=<value>` when the page size is given, and no query string at all when it
/// is absent.
fn uri(operation: &ListOperation, max_results: Option<i64>) -> String {
    match max_results {
        Some(value) => format!("{}?maxResults={value}", operation.path),
        None => operation.path.clone(),
    }
}

/// RULE: a request that omits an optional `@httpQuery` page size is SERVED - the
/// modelled `@default` is not materialised into a value and then range-checked against
/// the member's own bound.
#[tokio::test]
async fn an_absent_page_size_is_absent_not_the_modelled_default() {
    let router = emulator();
    for operation in list_operations(&router).await {
        let answer = call(&router, "GET", &uri(&operation, None), None).await;
        assert_eq!(
            answer.status, operation.served,
            "case {} with no maxResults: expected {}, got {} body {:?}",
            operation.case, operation.served, answer.status, answer.body
        );
        assert!(
            !refused_about_max_results(&answer),
            "case {} with no maxResults: expected no refusal about maxResults, got {:?}",
            operation.case,
            answer.body
        );
    }
}

/// RULE: a page size the request DOES carry is checked against the modelled
/// `@range(min: 1, max: 200)` - the published bounds are accepted, and every value
/// outside them (the modelled default 0 among them, when a caller sends it explicitly) is
/// refused as a 400 `ValidationException` naming the member and the bound it broke.
#[tokio::test]
async fn a_supplied_page_size_is_checked_against_the_modelled_range() {
    let router = emulator();
    for operation in list_operations(&router).await {
        // Rows differ along the page size asked for: both published bounds, one inside
        // them, the value the modelled `@default` names, one below the minimum, and one
        // past the maximum.
        for accepted in [MAX_RESULTS_MIN, 2, MAX_RESULTS_MAX] {
            let answer = call(&router, "GET", &uri(&operation, Some(accepted)), None).await;
            assert_eq!(
                answer.status, operation.served,
                "case {} maxResults={accepted}: expected {}, got {} body {:?}",
                operation.case, operation.served, answer.status, answer.body
            );
        }

        for refused in [MODELLED_DEFAULT, -1, MAX_RESULTS_MAX + 1] {
            let answer = call(&router, "GET", &uri(&operation, Some(refused)), None).await;
            if !operation.enforces_range {
                // A scaffolded operation answers 501 whatever the page size: the refusal
                // it would make has not been written, and a 501 is not a claim about the
                // request.
                assert_eq!(
                    answer.status, operation.served,
                    "case {} maxResults={refused}: expected the scaffold's {}, got {} \
                     body {:?}",
                    operation.case, operation.served, answer.status, answer.body
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {} maxResults={refused}: expected 400 outside the published bounds, \
                 got {} body {:?}",
                operation.case, answer.status, answer.body
            );
            assert_eq!(
                answer.error.as_deref(),
                Some("ValidationException"),
                "case {} maxResults={refused}: expected ValidationException, got {:?}",
                operation.case,
                answer.error
            );
            let expected = if refused > MAX_RESULTS_MAX {
                format!(
                    "1 validation error detected: Value '{refused}' at 'maxResults' failed to \
                     satisfy constraint: Member must have value less than or equal to \
                     {MAX_RESULTS_MAX}"
                )
            } else {
                format!(
                    "1 validation error detected: Value '{refused}' at 'maxResults' failed to \
                     satisfy constraint: Member must have value greater than or equal to \
                     {MAX_RESULTS_MIN}"
                )
            };
            assert_eq!(
                message(&answer),
                expected,
                "case {} maxResults={refused}: expected the modelled range refusal, got {:?}",
                operation.case,
                answer.body
            );
        }
    }
}

/// RULE: with no page size asked for, the page is bounded by the largest the model admits
/// - so a collection smaller than that is served whole, in one page, with no `NextToken`.
#[tokio::test]
async fn an_absent_page_size_serves_the_whole_collection_in_one_page() {
    let router = emulator();

    // Rows differ along how much state holds: nothing, one, and a handful.
    let mut created: Vec<String> = Vec::new();
    for count in [0_usize, 1, 5] {
        while created.len() < count {
            let answer = create_data_set(
                &router,
                &format!("set-{}", created.len()),
                "absent-page-size",
                "S3_SNAPSHOT",
                None,
            )
            .await;
            created.push(answer.string("Id"));
        }

        let answer = call(&router, "GET", "/v1/data-sets", None).await;
        assert_eq!(
            answer.status, 200,
            "case {count} data sets: expected 200, got {} body {:?}",
            answer.status, answer.body
        );
        let served: Vec<String> = answer
            .member("DataSets")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            served, created,
            "case {count} data sets: expected every data set in state exactly once, got \
             {served:?} for {created:?}"
        );
        assert!(
            answer.member("NextToken").is_null(),
            "case {count} data sets: expected no NextToken on a page that exhausted the \
             collection, got {:?}",
            answer.member("NextToken")
        );
    }
}

/// The refusal message a 400 carries, or the empty string when the answer carries none.
fn message(answer: &Answer) -> String {
    answer
        .member("Message")
        .as_str()
        .or_else(|| answer.member("message").as_str())
        .unwrap_or_default()
        .to_string()
}

/// Whether the answer is a refusal that names `maxResults` - the misattribution this
/// suite exists to keep out of every other refusal.
fn refused_about_max_results(answer: &Answer) -> bool {
    answer.status >= 400 && message(answer).contains("maxResults")
}
