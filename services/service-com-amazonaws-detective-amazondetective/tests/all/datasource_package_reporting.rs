// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the part of `ListDatasourcePackages` that is about the
//! REPORT rather than the ingest states themselves: which graph's records it reports,
//! that it answers all of them at once, and the `NextToken` it will not accept.
//!
//! `datasource_package_ingest` asserts the write-then-read rule (what
//! `UpdateDatasourcePackages` stores and what `LastIngestStateChange` then says); this
//! suite asserts the three rules that are independent of any package's state:
//!
//! 1. A report is scoped to the behavior graph the request names: it carries that
//!    graph's records and no other graph's, however the two graphs differ.
//! 2. The report is not paginated: one call carries every package the graph holds and
//!    no `NextToken`, whatever the graph holds, and a request that names `MaxResults` is
//!    refused instead of being served a part of the map.
//! 3. A `NextToken` this operation did not issue is refused with the modelled
//!    `ValidationException` (400) and one fixed sentence, for a token anywhere within
//!    the modelled `PaginationToken` length of 1..1024 — the refusal never echoes it.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_detective_amazondetective::state::{
    MANDATORY_DATASOURCE_PACKAGE, MAX_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, call_in, create_graph_ok, emulator, error_code, error_message, REGION};

/// Every value the modelled `DatasourcePackage` enum accepts
/// (`smithy_query(query="shape",shape="DatasourcePackage")`), written in the order a
/// package name sorts.
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
async fn one_report_carries_every_held_package_whatever_the_graph_holds() {
    let router = emulator();

    // The rows differ along HOW MANY packages the graph holds and WHICH: the one package
    // a fresh graph starts with, one optional package beside it, and every modelled value
    // at once. Each row is one emulator, so each gets its own graph.
    let rows: [(&str, Vec<&str>); 3] = [
        ("only-the-mandatory-package", vec![MANDATORY_DATASOURCE_PACKAGE]),
        (
            "the-mandatory-package-and-one-more",
            vec![MANDATORY_DATASOURCE_PACKAGE, "EKS_AUDIT"],
        ),
        ("every-modelled-package", PACKAGES_IN_ORDER.to_vec()),
    ];
    for (case, packages) in rows {
        let router = emulator();
        let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
        update_ok(&router, case, REGION, &graph_arn, &packages).await;

        let (reported, token) = page_of(&router, case, &graph_arn, None, None).await;
        assert_eq!(
            token, None,
            "case {case:?}: an unpaginated report issues no NextToken"
        );
        let expected: BTreeSet<String> = packages.iter().map(|p| (*p).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected every held package in one response, got {reported:?}"
        );
    }

    // And a request that asks for a page of them is refused rather than served part of
    // the map: the rows differ along the size asked for, inside the modelled
    // `MemberResultsLimit` range (1..200) and at each of its published bounds.
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    update_ok(&router, "seed", REGION, &graph_arn, &PACKAGES_IN_ORDER).await;
    for max_results in [1, 2, 3, MAX_PAGE_SIZE as i32] {
        let case = format!("max-results-{max_results}");
        let (status, answered) = call(
            &router,
            LIST_URI,
            &json!({ "GraphArn": graph_arn, "MaxResults": max_results }),
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
        assert_eq!(
            answered.get("NextToken"),
            None,
            "case {case:?}: a refusal carries no NextToken, got {answered:?}"
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

    // The control: this report issues no token at all, so a caller has none to send.
    let (first, token) = page_of(&router, "issued token", &graph_arn, None, None).await;
    assert_eq!(
        token, None,
        "the report must issue no NextToken, got {token:?}"
    );
    assert_eq!(
        first,
        PACKAGES_IN_ORDER
            .iter()
            .map(|p| (*p).to_string())
            .collect::<BTreeSet<String>>(),
        "one response must carry every held package, got {first:?}"
    );
}
