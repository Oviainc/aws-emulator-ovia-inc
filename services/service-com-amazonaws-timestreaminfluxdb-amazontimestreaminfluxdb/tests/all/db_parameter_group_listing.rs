// SPDX-License-Identifier: CC0-1.0

//! `ListDbParameterGroups`, end to end over the wire.
//!
//! The suite drives real `awsJson1_0` requests through the same [`MultiServiceRouter`]
//! the production router dispatches with, reusing the harness in
//! [`crate::db_instance_lifecycle`] (`router`, `call`, `ok`) so there is one wire harness
//! for this crate.
//!
//! The assertions are RULES over a table of inputs, not a replay of one recorded call:
//! the operation reports a page of the DB parameter groups the account and Region has —
//! the service-managed catalog and the customer-created groups — each customer summary
//! projected from the stored record, pages at `maxResults`, hands back a `nextToken` only
//! while a further page exists, and changes nothing a later caller sees. The rows vary the
//! number of stored groups, the page size, and the form of each group (name length and
//! shape, description present / absent / empty, parameters supplied or not).

use serde_json::{json, Value};
use service_com_amazonaws_timestreaminfluxdb_amazontimestreaminfluxdb as influxdb;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_lifecycle::{call, ok, router, ACCOUNT, REGION};

/// Members of `DbParameterGroupSummary`, from
/// `smithy_query(query="shape", shape="DbParameterGroupSummary")`: `id`, `name` and `arn`
/// are `@required`, `description` is optional. A summary must agree with the stored
/// record on each one.
const SUMMARY_MEMBERS: &[&str] = &["arn", "description", "id", "name"];

/// The `@required` members of `DbParameterGroupSummary`: present on every summary.
const REQUIRED_SUMMARY_MEMBERS: &[&str] = &["arn", "id", "name"];

/// The ids of the service-managed DB parameter groups every account and Region has
/// without anyone creating them.
///
/// They are reported ahead of the customer-created groups, so every rule this suite
/// asserts about paging and projection holds over them too. What the catalog CONTAINS is
/// pinned in `db_parameter_group_catalog`, independently of the implementation.
fn service_managed_ids() -> Vec<String> {
    influxdb::state::service_managed_db_parameter_groups()
        .iter()
        .map(|group| group.id.clone())
        .collect()
}

/// The `id` in a response body.
fn id_of(case: &str, body: &Value) -> String {
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string id, got {body}"))
        .to_string()
}

/// The create requests this suite stores before listing: one row per shape of a stored
/// record the projection has to hold over. `DbParameterGroupName` is length 3..64,
/// pattern `^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$`; `description` is length 0..500.
fn stored_cases() -> Vec<(String, Value)> {
    vec![
        (
            "a name at the modelled minimum length 3, no description".to_string(),
            json!({ "name": "Abc" }),
        ),
        (
            "a name at the modelled maximum length 64".to_string(),
            json!({ "name": format!("A{}", "b".repeat(63)) }),
        ),
        (
            "a hyphenated name with a description at the modelled maximum length 500"
                .to_string(),
            json!({ "name": "pg-with-hyphens-9", "description": "d".repeat(500) }),
        ),
        (
            "an empty description, the modelled minimum length 0".to_string(),
            json!({ "name": "EmptyDescription", "description": "" }),
        ),
        (
            "parameters supplied, which no summary member carries".to_string(),
            json!({
                "name": "v2-parameters",
                "description": "InfluxDB v2 tuning",
                "parameters": {
                    "InfluxDBv2": {
                        "fluxLogEnabled": true,
                        "logLevel": "debug",
                        "queryConcurrency": 12,
                        "httpIdleTimeout": { "durationType": "minutes", "value": 3 }
                    }
                },
            }),
        ),
        (
            "tags supplied, which no parameter-group output shape carries".to_string(),
            json!({ "name": "Tagged", "tags": { "team": "data", "env": "test" } }),
        ),
    ]
}

/// Drain every page of `ListDbParameterGroups`, asserting the page-size bound and the
/// required members on the way, and answer the ids in the order they were yielded.
async fn drain(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<i64>,
    page_bound: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        pages += 1;
        assert!(
            pages <= page_bound,
            "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
        );
        let mut request = json!({});
        if let Some(limit) = max_results {
            request["maxResults"] = json!(limit);
        }
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = ok(case, call(router, "ListDbParameterGroups", request).await);
        let items = page["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected an `items` list, got {page}"))
            .clone();
        if let Some(limit) = max_results {
            assert!(
                items.len() as i64 <= limit,
                "case {case:?}: a page of {} exceeds maxResults {limit}",
                items.len()
            );
        }
        for item in &items {
            for member in REQUIRED_SUMMARY_MEMBERS {
                assert!(
                    item.get(*member).is_some(),
                    "case {case:?}: the summary omits {member:?}: {item}"
                );
            }
            seen.push(id_of(case, item));
        }
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => {
                assert!(
                    !items.is_empty(),
                    "case {case:?}: an empty page must not hand back a nextToken: {page}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    seen
}

#[tokio::test]
async fn paging_yields_every_stored_group_exactly_once() {
    // (groups created, maxResults): `MaxResults` is modelled range 1..100, so the rows
    // span both published bounds plus the sizes between them, and an omitted page size.
    let cases: [(usize, Option<i64>); 7] = [
        (0, None),
        (1, None),
        (2, Some(1)),
        (5, Some(2)),
        (3, Some(100)),
        (7, Some(3)),
        (4, Some(4)),
    ];

    for (count, max_results) in cases {
        let case = format!("{count} groups at maxResults {max_results:?}");
        let router = router();
        // Every group the account has: the service-managed catalog, plus what this row
        // creates.
        let mut expected: Vec<String> = service_managed_ids();
        for index in 0..count {
            let created = ok(
                &case,
                call(
                    &router,
                    "CreateDbParameterGroup",
                    json!({ "name": format!("page-{index}") }),
                )
                .await,
            );
            expected.push(id_of(&case, &created));
        }
        expected.sort();

        let seen = drain(&router, &case, max_results, expected.len() + 1).await;

        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            seen.len(),
            sorted.len(),
            "case {case:?}: paging repeated a group: {seen:?}"
        );
        assert_eq!(
            sorted, expected,
            "case {case:?}: paging must yield exactly the stored groups, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn every_summary_is_projected_from_the_stored_record() {
    let router = router();
    let mut created_by_id: Vec<(String, String, Value)> = Vec::new();
    for (case, request) in stored_cases() {
        let created = ok(
            &case,
            call(&router, "CreateDbParameterGroup", request.clone()).await,
        );
        let id = id_of(&case, &created);
        created_by_id.push((case, id, created));
    }

    let listed = ok(
        "every stored group",
        call(
            &router,
            "ListDbParameterGroups",
            json!({ "maxResults": 100 }),
        )
        .await,
    );
    let items = listed["items"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an `items` list, got {listed}"));
    assert_eq!(
        items.len(),
        created_by_id.len() + service_managed_ids().len(),
        "case \"every stored group\": every created group and every service-managed one \
         must be listed, got {listed}"
    );

    for (case, id, created) in &created_by_id {
        let summary = items
            .iter()
            .find(|item| item["id"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {id} is missing from {listed}"));

        // The summary is the stored record projected: it agrees with the record the
        // create and the reader report, member for member.
        let fetched = ok(
            case,
            call(&router, "GetDbParameterGroup", json!({ "identifier": id })).await,
        );
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                &summary[*member], &created[*member],
                "case {case:?}: the summary disagrees with the create on {member:?}: \
                 expected {:?}, got {:?}",
                created[*member], summary[*member]
            );
            assert_eq!(
                &summary[*member], &fetched[*member],
                "case {case:?}: the summary disagrees with GetDbParameterGroup on \
                 {member:?}: expected {:?}, got {:?}",
                fetched[*member], summary[*member]
            );
        }

        // The ARN is scoped to the request's region and derived from the minted id.
        assert_eq!(
            summary["arn"].as_str(),
            Some(
                format!("arn:aws:timestream-influxdb:{REGION}:{ACCOUNT}:db-parameter-group/{id}")
                    .as_str()
            ),
            "case {case:?}: unexpected arn {:?}",
            summary["arn"]
        );

        // `DbParameterGroupSummary` models neither `parameters` nor `tags`, so a
        // summary reports neither however the group was created.
        for absent in ["parameters", "tags"] {
            assert!(
                summary[absent].is_null(),
                "case {case:?}: the summary models no {absent:?} member, got {:?}",
                summary[absent]
            );
        }
    }
}

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = router();
    for (case, request) in stored_cases() {
        ok(&case, call(&router, "CreateDbParameterGroup", request).await);
    }

    // Repeating a list — whole, and page by page — reports the same answer: the
    // operation only reports.
    let first = ok(
        "repeated list",
        call(
            &router,
            "ListDbParameterGroups",
            json!({ "maxResults": 100 }),
        )
        .await,
    );
    for attempt in 0..3 {
        let case = format!("repeated list, attempt {attempt}");
        let again = ok(
            &case,
            call(
                &router,
                "ListDbParameterGroups",
                json!({ "maxResults": 100 }),
            )
            .await,
        );
        assert_eq!(
            again, first,
            "case {case:?}: a later list must report the same groups, got {again}"
        );

        let paged = drain(
            &router,
            &case,
            Some(1),
            stored_cases().len() + service_managed_ids().len() + 1,
        )
        .await;
        let whole: Vec<String> = first["items"]
            .as_array()
            .expect("an `items` list")
            .iter()
            .map(|item| id_of(&case, item))
            .collect();
        assert_eq!(
            paged, whole,
            "case {case:?}: paging must yield the same groups in the same order as one \
             page, got {paged:?}"
        );
    }
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = router();
    ok(
        "a stored group",
        call(
            &router,
            "CreateDbParameterGroup",
            json!({ "name": "OnlyGroup" }),
        )
        .await,
    );

    // `MaxResults` is modelled range 1..100: one below the minimum and one above the
    // maximum are refused, and a token shorter than the modelled `NextToken` minimum
    // length 1 is refused with them.
    let refused: [(&str, Value); 3] = [
        ("maxResults one below the modelled minimum 1", json!({ "maxResults": 0 })),
        (
            "maxResults one above the modelled maximum 100",
            json!({ "maxResults": 101 }),
        ),
        (
            "an empty nextToken, below the modelled minimum length 1",
            json!({ "nextToken": "" }),
        ),
    ];
    for (case, request) in refused {
        let (status, body) = call(&router, "ListDbParameterGroups", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
    }

    // A token naming an id nothing minted is not an error: paging resumes strictly
    // after it, so the page is simply the groups ordered after that id.
    let (status, body) = call(
        &router,
        "ListDbParameterGroups",
        json!({ "nextToken": "zzzzzzzzzz" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a token past the last group\": expected HTTP 200, got {status} with {body}"
    );
    assert!(
        body["items"].as_array().is_some_and(Vec::is_empty),
        "case \"a token past the last group\": expected no items, got {body}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case \"a token past the last group\": expected no nextToken, got {body}"
    );
}
