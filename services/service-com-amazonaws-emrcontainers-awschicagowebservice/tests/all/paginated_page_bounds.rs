// SPDX-License-Identifier: CC0-1.0

//! The page-bounds rule of every `@paginated` list in this service, driven through ONE
//! operation per listing.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The five listings — `ListVirtualClusters`, `ListJobRuns`, `ListJobTemplates`,
//! `ListSecurityConfigurations` and `ListManagedEndpoints` — answer one page rule, so this
//! suite asserts that rule over a TABLE whose rows are the listings themselves, each with
//! its own stored count and its own cursor scope:
//!
//! 1. `maxResults` is the page size: no page carries more rows than it asked for, and
//!    walking the pages reports exactly the unpaged listing, in the same (newest-first)
//!    order, with no repeats and no gaps.
//! 2. A `nextToken` comes back only while rows remain after the page.
//! 3. A page of fewer than one row, and a `nextToken` the listing's own scope never
//!    issued, are refused with the modelled `ValidationException` (HTTP 400) carrying one
//!    wording for every listing, and change nothing a later reader sees.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::{create_endpoint, endpoint_body};
use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr};
use crate::virtual_cluster_scheduler_status::run_id;

/// One `@paginated` listing under test: where it is served, which output member carries
/// its page, the records it holds, and a scope that belongs to ANOTHER of this service's
/// listings (so a cursor minted under it is one this listing must refuse).
struct Listing {
    label: &'static str,
    /// The request path, one query string away from a call.
    path: String,
    /// The JSON pointer of the output member the page's records come back in.
    member: &'static str,
    /// Another listing's cursor scope.
    foreign_scope: String,
    /// The ids the fixture stored, oldest first.
    stored: Vec<String>,
}

/// The listings this suite drives: one per paginated operation the service serves.
#[derive(Clone, Copy, Debug)]
enum Which {
    VirtualClusters,
    JobRuns,
    JobTemplates,
    SecurityConfigurations,
    ManagedEndpoints,
}

const EVERY_LISTING: [Which; 5] = [
    Which::VirtualClusters,
    Which::JobRuns,
    Which::JobTemplates,
    Which::SecurityConfigurations,
    Which::ManagedEndpoints,
];

/// The smallest `jobTemplateData` the model accepts: `executionRoleArn`, `releaseLabel`
/// and `jobDriver` are all `@required` (smithy_query: shape `JobTemplateData`).
fn minimal_template_data() -> Value {
    json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
        "releaseLabel": "emr-6.15.0-latest",
        "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/page.py"}},
    })
}

/// A fresh emulator holding the records one listing pages, and that listing.
///
/// The rows differ in how many records are stored (3 to 6), so a page size that divides
/// one listing's count does not divide another's.
async fn fixture(which: Which) -> (Emr, Listing) {
    let emr = Emr::new();
    let listing = match which {
        Which::VirtualClusters => {
            let mut stored = Vec::new();
            for index in 0..5 {
                stored.push(
                    emr.cluster(
                        &format!("page-cluster-{index}"),
                        &format!("tok-page-cluster-{index}"),
                    )
                    .await,
                );
            }
            Listing {
                label: "ListVirtualClusters",
                path: "/virtualclusters".to_string(),
                member: "/virtualClusters",
                foreign_scope: state::LIST_JOB_TEMPLATES_SCOPE.to_string(),
                stored,
            }
        }
        Which::JobRuns => {
            let cluster = emr.cluster("page-runs", "tok-page-runs").await;
            let mut stored = Vec::new();
            for index in 0..4 {
                stored.push(
                    run_id(
                        &emr,
                        "ListJobRuns",
                        &cluster,
                        &format!("tok-page-run-{index}"),
                    )
                    .await,
                );
            }
            Listing {
                label: "ListJobRuns",
                path: format!("/virtualclusters/{cluster}/jobruns"),
                member: "/jobRuns",
                // The SAME cluster's endpoint listing: a cursor of one of its listings is
                // not a position in the order of the other.
                foreign_scope: state::list_managed_endpoints_scope(&cluster),
                stored,
            }
        }
        Which::JobTemplates => {
            let mut stored = Vec::new();
            for index in 0..3 {
                let created = emr
                    .send(
                        "POST",
                        "/jobtemplates",
                        Some(json!({
                            "name": format!("page-template-{index}"),
                            "clientToken": format!("tok-page-template-{index}"),
                            "jobTemplateData": minimal_template_data(),
                        })),
                    )
                    .await;
                stored.push(text(created.ok("ListJobTemplates"), "/id", "ListJobTemplates"));
            }
            Listing {
                label: "ListJobTemplates",
                path: "/jobtemplates".to_string(),
                member: "/templates",
                foreign_scope: state::LIST_SECURITY_CONFIGURATIONS_SCOPE.to_string(),
                stored,
            }
        }
        Which::SecurityConfigurations => {
            let mut stored = Vec::new();
            for index in 0..6 {
                let created = emr
                    .send(
                        "POST",
                        "/securityconfigurations",
                        Some(json!({
                            "name": format!("page-config-{index}"),
                            "clientToken": format!("tok-page-config-{index}"),
                            "securityConfigurationData": {},
                        })),
                    )
                    .await;
                stored.push(text(
                    created.ok("ListSecurityConfigurations"),
                    "/id",
                    "ListSecurityConfigurations",
                ));
            }
            Listing {
                label: "ListSecurityConfigurations",
                path: "/securityconfigurations".to_string(),
                member: "/securityConfigurations",
                foreign_scope: state::LIST_VIRTUAL_CLUSTERS_SCOPE.to_string(),
                stored,
            }
        }
        Which::ManagedEndpoints => {
            let cluster = emr.cluster("page-endpoints", "tok-page-endpoints").await;
            let mut stored = Vec::new();
            for index in 0..4 {
                let created = create_endpoint(
                    &emr,
                    &cluster,
                    endpoint_body(
                        &format!("page-endpoint-{index}"),
                        &format!("tok-page-endpoint-{index}"),
                        "JUPYTER_ENTERPRISE_GATEWAY",
                    ),
                )
                .await;
                stored.push(text(
                    created.ok("ListManagedEndpoints"),
                    "/id",
                    "ListManagedEndpoints",
                ));
            }
            Listing {
                label: "ListManagedEndpoints",
                path: format!("/virtualclusters/{cluster}/endpoints"),
                member: "/endpoints",
                // The same cluster's job-run listing scopes its cursors by the cluster id
                // alone, which is not this listing's scope.
                foreign_scope: cluster,
                stored,
            }
        }
    };
    (emr, listing)
}

/// One call to a listing with `query` appended to its path.
async fn list(emr: &Emr, listing: &Listing, query: &str) -> Answer {
    emr.send("GET", &format!("{}{query}", listing.path), None)
        .await
}

/// The ids a page carries, in the order it reported them.
fn page_ids(body: &Value, listing: &Listing, case: &str) -> Vec<String> {
    field(body, listing.member, case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {} is not a list: {body}", listing.member))
        .iter()
        .map(|record| text(record, "/id", case))
        .collect()
}

/// The page sizes a listing of `stored` records is walked at: one row at a time, divisors
/// and non-divisors of the stored count, exactly the stored count, one past it, and the
/// largest page a request can ask for — `maxResults` is a `JavaInteger`, a 32-bit integer
/// (smithy_query: operation `ListJobRuns`, member `maxResults`).
fn page_sizes(stored: usize) -> Vec<usize> {
    let mut sizes = vec![1, 2, 3, stored, stored + 2];
    sizes.push(usize::try_from(i32::MAX).expect("a 32-bit page size fits a usize"));
    sizes
}

/// RULES 1 and 2: every listing walks its stored set under one page rule — no page longer
/// than `maxResults`, the walk equal to the unpaged listing, and a `nextToken` only while
/// rows remain.
#[tokio::test]
async fn every_paginated_list_walks_its_stored_set_under_the_shared_page_bounds() {
    for which in EVERY_LISTING {
        let (emr, listing) = fixture(which).await;
        let label = listing.label;

        // The unpaged listing is the yardstick: everything stored, newest first (the
        // reverse of creation order), and no `nextToken` because nothing remains.
        let answer = list(&emr, &listing, "").await;
        let body = answer.ok(label).clone();
        let unpaged = page_ids(&body, &listing, label);
        let mut newest_first = listing.stored.clone();
        newest_first.reverse();
        assert_eq!(
            unpaged, newest_first,
            "case {label:?}: expected the stored records newest first {newest_first:?}, got {unpaged:?}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case {label:?}: expected no nextToken on a listing that reported everything, got {body}"
        );

        for page_size in page_sizes(listing.stored.len()) {
            let case = format!("{label} at maxResults {page_size}");
            let mut walked: Vec<String> = Vec::new();
            let mut pages = 0usize;
            let mut query = format!("?maxResults={page_size}");
            loop {
                let answer = list(&emr, &listing, &query).await;
                let body = answer.ok(&case).clone();
                let page = page_ids(&body, &listing, &case);
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: expected at most {page_size} records in a page, got {}",
                    page.len()
                );
                walked.extend(page);
                pages += 1;
                assert!(
                    pages <= unpaged.len() + 1,
                    "case {case:?}: expected the walk to end, got {pages} pages for {} records",
                    unpaged.len()
                );
                match body.get("nextToken").and_then(Value::as_str) {
                    Some(token) => {
                        assert!(
                            walked.len() < unpaged.len(),
                            "case {case:?}: expected no nextToken once all {} records were reported, got one after {}",
                            unpaged.len(),
                            walked.len()
                        );
                        query = format!("?maxResults={page_size}&nextToken={token}");
                    }
                    None => break,
                }
            }

            let unique: BTreeSet<&String> = walked.iter().collect();
            assert_eq!(
                unique.len(),
                walked.len(),
                "case {case:?}: expected no record twice, got {walked:?}"
            );
            assert_eq!(
                walked, unpaged,
                "case {case:?}: expected the walk to report the unpaged listing {unpaged:?}, got {walked:?}"
            );
        }
    }
}

/// RULE 3: every listing refuses an unservable page the same way — one status, one error
/// shape and one wording — and a refused request changes nothing.
#[tokio::test]
async fn every_paginated_list_refuses_an_unservable_page_the_same_way() {
    // The wording the shared rule answers with, once for all five listings.
    let bad_token = "nextToken is not a valid pagination token".to_string();

    for which in EVERY_LISTING {
        let (emr, listing) = fixture(which).await;
        let label = listing.label;
        let before = page_ids(list(&emr, &listing, "").await.ok(label), &listing, label);

        // The foreign cursor is minted by the one encoder every paginated list here uses,
        // so this row cannot drift from the cursor format.
        let foreign_cursor = state::encode_list_cursor(&listing.foreign_scope, 1, 2);
        let rows: Vec<(String, String, String)> = vec![
            (
                "maxResults below one".to_string(),
                "?maxResults=0".to_string(),
                "maxResults must be at least 1, but 0 was requested".to_string(),
            ),
            (
                "a negative maxResults".to_string(),
                "?maxResults=-3".to_string(),
                "maxResults must be at least 1, but -3 was requested".to_string(),
            ),
            (
                "the most negative maxResults a 32-bit integer carries".to_string(),
                format!("?maxResults={}", i32::MIN),
                format!(
                    "maxResults must be at least 1, but {} was requested",
                    i32::MIN
                ),
            ),
            (
                "a forged nextToken".to_string(),
                "?nextToken=not-a-cursor".to_string(),
                bad_token.clone(),
            ),
            (
                "a nextToken with one component".to_string(),
                "?nextToken=12345".to_string(),
                bad_token.clone(),
            ),
            (
                "a nextToken carrying no scope".to_string(),
                "?nextToken=1:2".to_string(),
                bad_token.clone(),
            ),
            (
                "a nextToken whose parts are not numbers".to_string(),
                "?nextToken=a:b".to_string(),
                bad_token.clone(),
            ),
            (
                "a cursor another listing issued".to_string(),
                format!("?nextToken={foreign_cursor}"),
                bad_token.clone(),
            ),
        ];

        for (row, query, expected) in rows {
            let case = format!("{label} / {row}");
            let answer = list(&emr, &listing, &query).await;
            answer.refused(&case, 400, "ValidationException");
            let reported = text(&answer.body, "/message", &case);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the refusal {expected:?}, got {reported:?}"
            );
        }

        let after = page_ids(list(&emr, &listing, "").await.ok(label), &listing, label);
        assert_eq!(
            after, before,
            "case {label:?}: expected the refused requests to leave {before:?} listed, got {after:?}"
        );
    }
}
