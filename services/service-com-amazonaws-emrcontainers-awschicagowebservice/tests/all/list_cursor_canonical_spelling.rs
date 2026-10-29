// SPDX-License-Identifier: CC0-1.0

//! Which `nextToken` SPELLINGS a `@paginated` listing of this service resumes from.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests, so routing, deserialization, constraint
//! validation, the handler and serialization all run exactly as they do for an AWS SDK
//! client.
//!
//! The rule, asserted over a table whose rows are the five listings
//! (`ListVirtualClusters`, `ListJobRuns`, `ListJobTemplates`,
//! `ListSecurityConfigurations`, `ListManagedEndpoints`) crossed with a table of token
//! spellings: a continuation token is opaque, so a listing resumes ONLY from a token it
//! issued itself — the exact bytes its cursor encoder writes for the position the token
//! names — and refuses every other spelling of that position with its modelled
//! `ValidationException` (HTTP 400) instead of reading it as a place in the order.
//!
//! The two halves matter together, so both are driven here:
//!
//! * a spelling the encoder never writes (a `+` sign, leading zeros, a signed zero,
//!   trailing whitespace) is refused, even when the position it would name is a real one;
//! * a canonically spelled cursor is accepted whatever position it names — including one
//!   older or newer than every stored record, and a negative millisecond, which the
//!   encoder CAN write — so the refusal keys on the spelling and not on the position
//!   happening to be occupied.

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::{create_endpoint, endpoint_body};
use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr};
use crate::virtual_cluster_scheduler_status::run_id;

/// The one wording every listing here refuses an unissued token with
/// (`state::PageRefusal::UnknownNextToken`).
const UNKNOWN_TOKEN: &str = "nextToken is not a valid pagination token";

/// One `@paginated` listing under test: where it is served, which output member carries
/// its page, and the cursor scope its own tokens are tagged with.
struct Listing {
    label: &'static str,
    /// The request path, one query string away from a call.
    path: String,
    /// The JSON pointer of the output member the page's records come back in.
    member: &'static str,
    /// The scope this listing's own cursors carry.
    scope: String,
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
        "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/cursor.py"}},
    })
}

/// A fresh emulator holding two records of one listing — enough that a one-row page
/// leaves a `nextToken` to resume from — and that listing.
async fn fixture(which: Which) -> (Emr, Listing) {
    let emr = Emr::new();
    let listing = match which {
        Which::VirtualClusters => {
            let mut stored = Vec::new();
            for index in 0..2 {
                stored.push(
                    emr.cluster(
                        &format!("cursor-cluster-{index}"),
                        &format!("tok-cursor-cluster-{index}"),
                    )
                    .await,
                );
            }
            Listing {
                label: "ListVirtualClusters",
                path: "/virtualclusters".to_string(),
                member: "/virtualClusters",
                scope: state::LIST_VIRTUAL_CLUSTERS_SCOPE.to_string(),
                stored,
            }
        }
        Which::JobRuns => {
            let cluster = emr.cluster("cursor-runs", "tok-cursor-runs").await;
            let mut stored = Vec::new();
            for index in 0..2 {
                stored.push(
                    run_id(
                        &emr,
                        "ListJobRuns",
                        &cluster,
                        &format!("tok-cursor-run-{index}"),
                    )
                    .await,
                );
            }
            Listing {
                label: "ListJobRuns",
                path: format!("/virtualclusters/{cluster}/jobruns"),
                member: "/jobRuns",
                // A job-run listing scopes its cursors by the cluster id alone.
                scope: cluster,
                stored,
            }
        }
        Which::JobTemplates => {
            let mut stored = Vec::new();
            for index in 0..2 {
                let created = emr
                    .send(
                        "POST",
                        "/jobtemplates",
                        Some(json!({
                            "name": format!("cursor-template-{index}"),
                            "clientToken": format!("tok-cursor-template-{index}"),
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
                scope: state::LIST_JOB_TEMPLATES_SCOPE.to_string(),
                stored,
            }
        }
        Which::SecurityConfigurations => {
            let mut stored = Vec::new();
            for index in 0..2 {
                let created = emr
                    .send(
                        "POST",
                        "/securityconfigurations",
                        Some(json!({
                            "name": format!("cursor-config-{index}"),
                            "clientToken": format!("tok-cursor-config-{index}"),
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
                scope: state::LIST_SECURITY_CONFIGURATIONS_SCOPE.to_string(),
                stored,
            }
        }
        Which::ManagedEndpoints => {
            let cluster = emr.cluster("cursor-endpoints", "tok-cursor-endpoints").await;
            let mut stored = Vec::new();
            for index in 0..2 {
                let created = create_endpoint(
                    &emr,
                    &cluster,
                    endpoint_body(
                        &format!("cursor-endpoint-{index}"),
                        &format!("tok-cursor-endpoint-{index}"),
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
                scope: state::list_managed_endpoints_scope(&cluster),
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

/// The `nextToken` a one-row page of this listing hands back: a cursor the service itself
/// minted, and the position every forged spelling below re-spells.
async fn issued_token(emr: &Emr, listing: &Listing) -> String {
    let case = format!("{} issues a nextToken", listing.label);
    let answer = list(emr, listing, "?maxResults=1").await;
    let body = answer.ok(&case).clone();
    text(&body, "/nextToken", &case)
}

/// The `(created_at_millis, sequence)` components of a token this listing issued, as the
/// decimal text the service wrote them as.
fn components(listing: &Listing, token: &str) -> (String, String) {
    let rest = token
        .strip_prefix(&listing.scope)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the issued token {token:?} is not tagged with the scope {:?}",
                listing.label, listing.scope
            )
        });
    let (millis, sequence) = rest
        .split_once(':')
        .unwrap_or_else(|| panic!("case {:?}: {token:?} carries no sequence", listing.label));
    (millis.to_string(), sequence.to_string())
}

/// Spellings of a position that the service's cursor encoder can never write, so no
/// listing may resume from one. The rows differ along the component that is re-spelled
/// (the millisecond, the sequence, both), the way it is re-spelled (a `+` sign, leading
/// zeros, a signed zero, trailing whitespace) and whether the position named is a real
/// one (`millis`/`sequence` come from a token the listing just issued) or one older than
/// every stored record (`0`, which read as a position would report an EMPTY page).
fn forged_spellings(scope: &str, millis: &str, sequence: &str) -> Vec<(String, String)> {
    vec![
        (
            "a plus-signed millisecond".to_string(),
            format!("{scope}:+{millis}:{sequence}"),
        ),
        (
            "a plus-signed sequence".to_string(),
            format!("{scope}:{millis}:+{sequence}"),
        ),
        (
            "both components plus-signed".to_string(),
            format!("{scope}:+{millis}:+{sequence}"),
        ),
        (
            "a millisecond padded with a leading zero".to_string(),
            format!("{scope}:0{millis}:{sequence}"),
        ),
        (
            "a sequence padded with a leading zero".to_string(),
            format!("{scope}:{millis}:0{sequence}"),
        ),
        (
            "both components zero-padded".to_string(),
            format!("{scope}:000{millis}:000{sequence}"),
        ),
        (
            "a plus-signed zero position".to_string(),
            format!("{scope}:+0:+0"),
        ),
        (
            "a zero-padded zero position".to_string(),
            format!("{scope}:0000:0000"),
        ),
        (
            "a negatively signed zero millisecond".to_string(),
            format!("{scope}:-0:0"),
        ),
        (
            "a negatively signed zero sequence".to_string(),
            format!("{scope}:0:-0"),
        ),
        (
            "a trailing space after the sequence".to_string(),
            format!("{scope}:{millis}:{sequence}%20"),
        ),
    ]
}

/// Every listing refuses a token it did not issue, however close that token's spelling is
/// to one it did, and reports the same records afterwards.
#[tokio::test]
async fn every_paginated_list_refuses_a_non_canonical_spelling_of_a_cursor() {
    for which in EVERY_LISTING {
        let (emr, listing) = fixture(which).await;
        let label = listing.label;
        let before = page_ids(list(&emr, &listing, "").await.ok(label), &listing, label);
        assert_eq!(
            before.len(),
            listing.stored.len(),
            "case {label:?}: expected the fixture's {} records listed, got {before:?}",
            listing.stored.len()
        );

        let token = issued_token(&emr, &listing).await;
        let (millis, sequence) = components(&listing, &token);

        for (row, forged) in forged_spellings(&listing.scope, &millis, &sequence) {
            let case = format!("{label} / {row}");
            let answer = list(&emr, &listing, &format!("?nextToken={forged}")).await;
            answer.refused(&case, 400, "ValidationException");
            let reported = text(&answer.body, "/message", &case);
            assert_eq!(
                reported, UNKNOWN_TOKEN,
                "case {case:?}: expected the refusal {UNKNOWN_TOKEN:?} for the token {forged:?}, got {reported:?}"
            );
        }

        let after = page_ids(list(&emr, &listing, "").await.ok(label), &listing, label);
        assert_eq!(
            after, before,
            "case {label:?}: expected the refused requests to leave {before:?} listed, got {after:?}"
        );
    }
}

/// Every listing resumes from a canonically spelled cursor whatever position it names, so
/// the refusal above is about the spelling and not about the position being occupied.
#[tokio::test]
async fn every_paginated_list_resumes_from_a_canonically_spelled_cursor() {
    for which in EVERY_LISTING {
        let (emr, listing) = fixture(which).await;
        let label = listing.label;
        let newest_first: Vec<String> = listing.stored.iter().rev().cloned().collect();

        // The token the listing itself issued after a one-row page: the rest of the set.
        let token = issued_token(&emr, &listing).await;
        let case = format!("{label} / the token it issued");
        let resumed = page_ids(
            list(&emr, &listing, &format!("?nextToken={token}"))
                .await
                .ok(&case),
            &listing,
            &case,
        );
        assert_eq!(
            resumed,
            newest_first[1..].to_vec(),
            "case {case:?}: expected the records after the first page {:?}, got {resumed:?}",
            &newest_first[1..]
        );

        // Canonical cursors the encoder can write for positions no record occupies: one
        // newer than every record (the whole set follows it), and two older than every
        // record (nothing follows). A negative millisecond is canonical — the encoder
        // writes an `i64`, whose Display carries a leading `-`.
        let rows: Vec<(String, String, Vec<String>)> = vec![
            (
                "a position newer than every record".to_string(),
                state::encode_list_cursor(&listing.scope, i64::MAX, u64::MAX),
                newest_first.clone(),
            ),
            (
                "the epoch position".to_string(),
                state::encode_list_cursor(&listing.scope, 0, 0),
                Vec::new(),
            ),
            (
                "a negative millisecond position".to_string(),
                state::encode_list_cursor(&listing.scope, -1, 7),
                Vec::new(),
            ),
        ];

        for (row, cursor, expected) in rows {
            let case = format!("{label} / {row}");
            let answer = list(&emr, &listing, &format!("?nextToken={cursor}")).await;
            let body = answer.ok(&case).clone();
            let reported = page_ids(&body, &listing, &case);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the cursor {cursor:?} to report {expected:?}, got {reported:?}"
            );
            assert!(
                body.get("nextToken").is_none(),
                "case {case:?}: expected no nextToken once everything after {cursor:?} was reported, got {body}"
            );
        }
    }
}
