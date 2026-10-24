// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the part of `ListDatasourcePackages` that is about the
//! REPORT rather than the ingest states themselves: which graph's records it reports,
//! the order its pages walk them in, and the `NextToken` it will not accept.
//!
//! `datasource_package_ingest` asserts the write-then-read rule (what
//! `UpdateDatasourcePackages` stores and what `LastIngestStateChange` then says); this
//! suite asserts the three rules that are independent of any package's state:
//!
//! 1. A report is scoped to the behavior graph the request names: it carries that
//!    graph's records and no other graph's, however the two graphs differ.
//! 2. The pages walk the held packages in ONE stable order — the order a package name
//!    sorts in — so a `NextToken` names a position: at a page size of one each page
//!    carries the next package in that order, and at a larger size each page carries the
//!    next block of it.
//! 3. A `NextToken` this operation did not issue is refused with the modelled
//!    `ValidationException` (400) and one fixed sentence, for a token anywhere within
//!    the modelled `PaginationToken` length of 1..1024 — the refusal never echoes it.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_detective_amazondetective::state::MANDATORY_DATASOURCE_PACKAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, call_in, create_graph_ok, emulator, error_code, error_message, REGION};

/// Every value the modelled `DatasourcePackage` enum accepts
/// (`smithy_query(query="shape",shape="DatasourcePackage")`), written in the order a
/// package name sorts, which is the order rule 2 says the pages walk.
const PACKAGES_IN_ORDER: [&str; 3] = ["ASFF_SECURITYHUB_FINDING", "DETECTIVE_CORE", "EKS_AUDIT"];

/// `ListDatasourcePackages`' URI, from
/// `smithy_query(query="operation",operation="ListDatasourcePackages")`.
const LIST_URI: &str = "/graph/datasources/list";

/// `UpdateDatasourcePackages`' URI, from the same model.
const UPDATE_URI: &str = "/graph/datasources/update";

/// The refusal `ListDatasourcePackages` answers a token it did not issue with: one fixed
/// sentence, which is what keeps it from echoing the caller's token back.
const NOT_A_TOKEN_MESSAGE: &str = "NextToken is not a pagination token issued by ListDatasourcePackages";

/// One `ListDatasourcePackages` page, which must succeed; answers its
/// `DatasourcePackages` keys and its `NextToken`.
async fn page_of(
    router: &MultiServiceRouter,
    case: &str,
    graph_arn: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> (BTreeSet<String>, Option<String>) {
    let mut body = Map::new();
    body.insert("GraphArn".to_string(), json!(graph_arn));
    if let Some(max_results) = max_results {
        body.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(token) = next_token {
        body.insert("NextToken".to_string(), json!(token));
    }
    let (status, listed) = call(router, LIST_URI, &Value::Object(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {listed:?}"
    );
    let keys = listed
        .get("DatasourcePackages")
        .and_then(Value::as_object)
        .unwrap_or_else(|| {
            panic!("case {case:?}: response carried no DatasourcePackages map, got {listed:?}")
        })
        .keys()
        .cloned()
        .collect();
    let token = listed
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (keys, token)
}

/// `UpdateDatasourcePackages` naming `packages` on `graph_arn`, signed for `region`,
/// which must be accepted.
async fn update_ok(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    graph_arn: &str,
    packages: &[&str],
) {
    let (status, answered) = call_in(
        router,
        region,
        UPDATE_URI,
        &json!({ "GraphArn": graph_arn, "DatasourcePackages": packages }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateDatasourcePackages {packages:?} expected 200, got {status} {answered:?}"
    );
}

#[tokio::test]
async fn a_report_carries_the_named_graphs_records_and_no_other_graphs() {
    let router = emulator();

    // Two graphs in one emulator, each ingesting a different set. They are in different
    // Regions because an account administers one behavior graph per Region — a second
    // `CreateGraph` in the one Region answers the same graph rather than minting another.
    // The rows differ along which packages their graph holds, so neither report could
    // pass by accident if the handler ignored the ARN it was given.
    let rows = [
        ("eks-graph", REGION, vec![MANDATORY_DATASOURCE_PACKAGE, "EKS_AUDIT"]),
        (
            "asff-graph",
            "eu-west-1",
            vec![MANDATORY_DATASOURCE_PACKAGE, "ASFF_SECURITYHUB_FINDING"],
        ),
    ];
    let mut graphs = Vec::new();
    for (case, region, packages) in &rows {
        let graph_arn = create_graph_ok(&router, region, &json!({})).await;
        update_ok(&router, case, region, &graph_arn, packages).await;
        graphs.push((case, graph_arn, packages));
    }

    for (case, graph_arn, packages) in &graphs {
        let (reported, token) = page_of(&router, case, graph_arn, None, None).await;
        assert_eq!(
            token, None,
            "case {case:?}: a report that fits on one page carries no NextToken"
        );
        let expected: BTreeSet<String> = packages.iter().map(|p| (*p).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly this graph's records, got {reported:?}"
        );
        // And the other graph's record is not in it: the two sets differ by exactly the
        // optional package each one started.
        for (other_case, _, other_packages) in &graphs {
            if other_case == case {
                continue;
            }
            for package in other_packages.iter().filter(|p| !packages.contains(p)) {
                assert!(
                    !reported.contains(*package),
                    "case {case:?}: {package:?} belongs to {other_case:?}, got {reported:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_pages_walk_the_held_packages_in_one_stable_order() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    update_ok(&router, "seed", REGION, &graph_arn, &PACKAGES_IN_ORDER).await;

    // The rows differ along the page size, and each row's expectation is the SAME order
    // cut into blocks of that size: a page size of one makes each page's single key fix
    // its position, and a larger size makes each page the next block of that order. (The
    // order WITHIN one page is not asserted anywhere: a JSON object's member order is not
    // part of the protocol.)
    for page_size in [1usize, 2, 3] {
        let case = format!("page size {page_size}");
        let expected: Vec<BTreeSet<String>> = PACKAGES_IN_ORDER
            .chunks(page_size)
            .map(|chunk| chunk.iter().map(|p| (*p).to_string()).collect())
            .collect();

        let mut walked: Vec<BTreeSet<String>> = Vec::new();
        let mut token: Option<String> = None;
        // One page per held package is the most a walk can need; one more would mean it
        // is not making progress.
        for page in 0..=PACKAGES_IN_ORDER.len() {
            let case = format!("{case} page {page}");
            let (keys, next) = page_of(
                &router,
                &case,
                &graph_arn,
                Some(page_size as i32),
                token.as_deref(),
            )
            .await;
            assert!(
                keys.len() <= page_size,
                "case {case:?}: expected at most {page_size} entries, got {keys:?}"
            );
            walked.push(keys);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the sorted order cut into blocks of {page_size}, \
             got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_report_did_not_issue_is_refused_without_being_echoed() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    update_ok(&router, "seed", REGION, &graph_arn, &PACKAGES_IN_ORDER).await;

    // The rows differ along WHERE in the modelled `PaginationToken` length of 1..1024 the
    // token sits and how it fails to be one of this operation's: its lower bound, its
    // upper bound, something that is not this encoding at all, and this encoding wrapping
    // a package name that is not a modelled value.
    let rows = [
        ("at-the-lower-length-bound", "z".to_string()),
        ("at-the-upper-length-bound", "ab".repeat(512)),
        ("not-this-encoding", "not-a-pagination-token".to_string()),
        // This operation's own encoding, wrapping a value the modelled
        // `DatasourcePackage` enum does not have.
        (
            "encodes-a-non-package",
            hex::encode("DETECTIVE_CORE_EXTRA"),
        ),
    ];
    for (case, token) in rows {
        assert!(
            (1..=1024).contains(&token.len()),
            "case {case:?}: the row itself must be a modelled PaginationToken, got {} chars",
            token.len()
        );
        let (status, answered) = call(
            &router,
            LIST_URI,
            &json!({ "GraphArn": graph_arn, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answered:?}"
        );
        let message = error_message(&answered);
        assert_eq!(
            message, NOT_A_TOKEN_MESSAGE,
            "case {case:?}: expected the fixed not-a-token message, got {message:?}"
        );
        assert!(
            !message.contains(token.as_str()),
            "case {case:?}: the refusal must not echo the token, got {message:?}"
        );
    }

    // The control: a token this report DID issue is accepted, and names the position the
    // page before it stopped at.
    let (first, token) = page_of(&router, "issued token", &graph_arn, Some(1), None).await;
    let token = token.unwrap_or_else(|| panic!("the first of three pages carried no NextToken"));
    let (second, _) = page_of(
        &router,
        "issued token",
        &graph_arn,
        Some(1),
        Some(token.as_str()),
    )
    .await;
    assert_eq!(
        first,
        BTreeSet::from([PACKAGES_IN_ORDER[0].to_string()]),
        "expected the first package in sorted order, got {first:?}"
    );
    assert_eq!(
        second,
        BTreeSet::from([PACKAGES_IN_ORDER[1].to_string()]),
        "expected the issued token to name the next package, got {second:?}"
    );
}
