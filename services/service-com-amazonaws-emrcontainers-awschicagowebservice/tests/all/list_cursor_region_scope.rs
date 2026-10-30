// SPDX-License-Identifier: CC0-1.0

//! Which ENDPOINT a `@paginated` listing of this service resumes a `nextToken` at.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests whose SigV4 credential scope — the thing a regional
//! endpoint is addressed through — names the region under test, so routing,
//! deserialization, constraint validation, the handler and serialization all run exactly as
//! they do for an AWS SDK client.
//!
//! The RULE under test: a `nextToken` is a POSITION in one newest-first order, and the
//! order a listing of this regional service pages is the records of the ONE region the
//! request is signed for. A cursor therefore names a place only in the listing and at the
//! endpoint that issued it: offered to any other listing, or to another region's endpoint,
//! it names no position there and is refused with the modelled `ValidationException`
//! (HTTP 400) carrying this service's one unknown-token wording — never read as a place in
//! a set it was not computed over, which would silently skip or truncate records.
//!
//! The tables span what the rule generalises over: the region PAIR (four pairs, one of them
//! driven both ways round), the listing (all five `@paginated` operations, the two scoped to
//! a virtual cluster included) and, for the forged cursors, the POSITION the foreign cursor
//! names (one a record really occupies, one newer than every record, one older than every
//! record).

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::endpoint_body;
use crate::virtual_cluster_job_run_loop::{
    field, runnable_body, spark_submit_driver, text, Answer, Emr,
};

/// The one wording every listing here refuses a token it did not issue with
/// (`state::PageRefusal::UnknownNextToken`).
const UNKNOWN_TOKEN: &str = "nextToken is not a valid pagination token";

/// The region pairs the tables below are driven over: `(home, other)`, two DIFFERENT
/// regions, so a cursor minted at `home`'s endpoint is offered to `other`'s.
///
/// The rows differ in the geography and the form of the region code, the first pair appears
/// both ways round so neither direction is privileged, and no pair is the emulator's own
/// default alone (`state::DEFAULT_REGION` is `us-east-1`).
const REGION_PAIRS: [(&str, &str); 4] = [
    ("us-east-1", "us-west-2"),
    ("us-west-2", "us-east-1"),
    ("eu-west-1", "ap-southeast-2"),
    ("ca-central-1", "eu-central-1"),
];

/// The listings this suite drives: one per `@paginated` operation the service serves.
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

impl Which {
    fn label(self) -> &'static str {
        match self {
            Self::VirtualClusters => "ListVirtualClusters",
            Self::JobRuns => "ListJobRuns",
            Self::JobTemplates => "ListJobTemplates",
            Self::SecurityConfigurations => "ListSecurityConfigurations",
            Self::ManagedEndpoints => "ListManagedEndpoints",
        }
    }

    /// The JSON pointer of the output member this listing's page comes back in.
    fn member(self) -> &'static str {
        match self {
            Self::VirtualClusters => "/virtualClusters",
            Self::JobRuns => "/jobRuns",
            Self::JobTemplates => "/templates",
            Self::SecurityConfigurations => "/securityConfigurations",
            Self::ManagedEndpoints => "/endpoints",
        }
    }
}

/// One listing as one region's endpoint serves it: where it is served, the records it
/// holds there, and the virtual cluster it pages when it is scoped to one.
struct Listing {
    which: Which,
    /// The request path, one query string away from a call.
    path: String,
    /// The virtual cluster this listing pages, for the two listings scoped to one.
    parent: Option<String>,
    /// The ids the fixture stored, oldest first.
    stored: Vec<String>,
}

impl Listing {
    fn label(&self) -> &'static str {
        self.which.label()
    }

    /// The cursor scope this listing's tokens carry at `region`'s endpoint — built by the
    /// service's own scope builders, so these rows cannot drift from the cursor format.
    fn scope_in(&self, region: &str) -> String {
        match (self.which, self.parent.as_deref()) {
            (Which::VirtualClusters, _) => state::list_virtual_clusters_scope(region),
            (Which::JobTemplates, _) => state::list_job_templates_scope(region),
            (Which::SecurityConfigurations, _) => {
                state::list_security_configurations_scope(region)
            }
            (Which::JobRuns, Some(cluster)) => state::list_job_runs_scope(region, cluster),
            (Which::ManagedEndpoints, Some(cluster)) => {
                state::list_managed_endpoints_scope(region, cluster)
            }
            (which, None) => panic!("case {:?}: {which:?} pages a virtual cluster", self.label()),
        }
    }

    /// The records this listing reports, newest first — the order every page walks.
    fn newest_first(&self) -> Vec<String> {
        self.stored.iter().rev().cloned().collect()
    }
}

/// `POST {path}` at `region`'s endpoint, answering with the `id` it minted.
async fn create_in(emr: &Emr, region: &str, path: &str, body: Value, case: &str) -> String {
    let answer = emr.send_in(region, "POST", path, Some(body)).await;
    text(answer.ok(case), "/id", case)
}

/// One acceptable `CreateVirtualCluster` body. The container provider names no namespace,
/// so two fixtures never collide on one.
fn cluster_body(name: &str, token: &str) -> Value {
    json!({
        "name": name,
        "clientToken": token,
        "containerProvider": {"id": "eks-cursor-region", "type": "EKS"},
    })
}

/// One acceptable `CreateJobTemplate` body: `executionRoleArn`, `releaseLabel` and
/// `jobDriver` are all `@required` of `JobTemplateData` (smithy_query: shape
/// `JobTemplateData`).
fn template_body(name: &str, token: &str) -> Value {
    json!({
        "name": name,
        "clientToken": token,
        "jobTemplateData": {
            "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
            "releaseLabel": "emr-6.15.0-latest",
            "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/cursor.py"}},
        },
    })
}

/// One acceptable `CreateSecurityConfiguration` body.
fn configuration_body(name: &str, token: &str) -> Value {
    json!({
        "name": name,
        "clientToken": token,
        "securityConfigurationData": {},
    })
}

/// `count` records of `which`, created at `region`'s endpoint, and the listing that pages
/// them there. `tag` keeps one region's fixture from sharing a `clientToken` with another's.
async fn fixture_in(emr: &Emr, which: Which, region: &str, tag: &str, count: usize) -> Listing {
    let case = format!("{} fixture {tag} in {region}", which.label());
    let mut stored = Vec::new();
    let mut parent = None;
    let path = match which {
        Which::VirtualClusters => {
            for index in 0..count {
                stored.push(
                    create_in(
                        emr,
                        region,
                        "/virtualclusters",
                        cluster_body(
                            &format!("cursor-cluster-{tag}-{index}"),
                            &format!("tok-cursor-cluster-{tag}-{index}"),
                        ),
                        &case,
                    )
                    .await,
                );
            }
            "/virtualclusters".to_string()
        }
        Which::JobTemplates => {
            for index in 0..count {
                stored.push(
                    create_in(
                        emr,
                        region,
                        "/jobtemplates",
                        template_body(
                            &format!("cursor-template-{tag}-{index}"),
                            &format!("tok-cursor-template-{tag}-{index}"),
                        ),
                        &case,
                    )
                    .await,
                );
            }
            "/jobtemplates".to_string()
        }
        Which::SecurityConfigurations => {
            for index in 0..count {
                stored.push(
                    create_in(
                        emr,
                        region,
                        "/securityconfigurations",
                        configuration_body(
                            &format!("cursor-config-{tag}-{index}"),
                            &format!("tok-cursor-config-{tag}-{index}"),
                        ),
                        &case,
                    )
                    .await,
                );
            }
            "/securityconfigurations".to_string()
        }
        Which::JobRuns | Which::ManagedEndpoints => {
            let cluster = create_in(
                emr,
                region,
                "/virtualclusters",
                cluster_body(
                    &format!("cursor-parent-{tag}"),
                    &format!("tok-cursor-parent-{tag}"),
                ),
                &case,
            )
            .await;
            let segment = match which {
                Which::JobRuns => "jobruns",
                _ => "endpoints",
            };
            let path = format!("/virtualclusters/{cluster}/{segment}");
            for index in 0..count {
                let body = match which {
                    Which::JobRuns => runnable_body(
                        &format!("tok-cursor-run-{tag}-{index}"),
                        spark_submit_driver(),
                    ),
                    _ => endpoint_body(
                        &format!("cursor-endpoint-{tag}-{index}"),
                        &format!("tok-cursor-endpoint-{tag}-{index}"),
                        "JUPYTER_ENTERPRISE_GATEWAY",
                    ),
                };
                stored.push(create_in(emr, region, &path, body, &case).await);
            }
            parent = Some(cluster);
            path
        }
    };
    Listing {
        which,
        path,
        parent,
        stored,
    }
}

/// One call to a listing at `region`'s endpoint, with `query` appended to its path.
async fn list_at(emr: &Emr, region: &str, listing: &Listing, query: &str) -> Answer {
    emr.send_in(region, "GET", &format!("{}{query}", listing.path), None)
        .await
}

/// The ids a page carries, in the order it reported them.
fn page_ids(body: &Value, listing: &Listing, case: &str) -> Vec<String> {
    field(body, listing.which.member(), case)
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} is not a list: {body}",
                listing.which.member()
            )
        })
        .iter()
        .map(|record| text(record, "/id", case))
        .collect()
}

/// Assert this call was refused as an unservable page, with the one wording every listing
/// here answers an unissued token with.
fn refused_as_unknown_token(answer: &Answer, case: &str) {
    answer.refused(case, 400, "ValidationException");
    let reported = text(&answer.body, "/message", case);
    assert_eq!(
        reported, UNKNOWN_TOKEN,
        "case {case:?}: expected the refusal {UNKNOWN_TOKEN:?}, got {reported:?}"
    );
}

/// The `nextToken` a one-row page of this listing hands back at `region`'s endpoint.
async fn issued_token(emr: &Emr, region: &str, listing: &Listing) -> String {
    let case = format!("{} issues a nextToken in {region}", listing.label());
    let answer = list_at(emr, region, listing, "?maxResults=1").await;
    let body = answer.ok(&case).clone();
    text(&body, "/nextToken", &case)
}

/// A token one endpoint issued is resumed only THERE: the issuing endpoint reports the rest
/// of its own set from it, and every other region's endpoint refuses it instead of reading
/// it as a position in the records IT holds.
#[tokio::test]
async fn a_next_token_resumes_only_at_the_endpoint_that_issued_it() {
    for (home, other) in REGION_PAIRS {
        for which in EVERY_LISTING {
            let emr = Emr::new();
            // Two records at home — so a one-row page leaves a cursor to resume from — and
            // one at the other endpoint, which a cursor read in ITS order would skip.
            let here = fixture_in(&emr, which, home, "here", 2).await;
            let there = fixture_in(&emr, which, other, "there", 1).await;
            let label = format!("{} from {home} at {other}", which.label());

            let token = issued_token(&emr, home, &here).await;

            // At the endpoint that issued it, the cursor is a position in that endpoint's
            // order: everything after the first page, with no repeats and no gaps.
            let case = format!("{label} / resumed where it was issued");
            let resumed = page_ids(
                list_at(&emr, home, &here, &format!("?nextToken={token}"))
                    .await
                    .ok(&case),
                &here,
                &case,
            );
            let expected = here.newest_first()[1..].to_vec();
            assert_eq!(
                resumed, expected,
                "case {case:?}: expected the records after the first page {expected:?}, got {resumed:?}"
            );

            // At any other endpoint the same token names no position, so it is refused
            // rather than positioning into a set it was never computed over.
            let case = format!("{label} / replayed at the other endpoint");
            let answer = list_at(&emr, other, &there, &format!("?nextToken={token}")).await;
            refused_as_unknown_token(&answer, &case);

            // And the refusal left that endpoint's own listing exactly as it was: nothing
            // was skipped, nothing was truncated.
            let listed = page_ids(
                list_at(&emr, other, &there, "").await.ok(&case),
                &there,
                &case,
            );
            assert_eq!(
                listed,
                there.newest_first(),
                "case {case:?}: expected {other} to still list its own records {:?}, got {listed:?}",
                there.newest_first()
            );
        }
    }
}

/// A cursor spelled with ANOTHER region's scope is refused where the records it would
/// position into actually are, whatever position it names — so the refusal is about the
/// endpoint that minted the cursor and not about the position happening to be empty.
#[tokio::test]
async fn a_cursor_tagged_with_another_region_is_refused_where_the_records_are() {
    for (home, other) in REGION_PAIRS {
        for which in EVERY_LISTING {
            let emr = Emr::new();
            let here = fixture_in(&emr, which, home, "forged", 3).await;
            let label = format!("{} at {home} with a {other} cursor", which.label());

            // The position a record of this endpoint really occupies, read back out of a
            // token this endpoint issued through the service's own cursor decoder.
            let token = issued_token(&emr, home, &here).await;
            let home_scope = here.scope_in(home);
            let (millis, sequence) = state::decode_list_cursor(&home_scope, &token)
                .unwrap_or_else(|| {
                    panic!("case {label:?}: {token:?} is not a cursor of the scope {home_scope:?}")
                });

            // The rows differ in the position the foreign cursor names: one a record
            // occupies, one newer than every record (read as a position it would report the
            // whole set) and one older than every record (it would report nothing).
            let other_scope = here.scope_in(other);
            let rows = [
                (
                    "a position one of these records occupies",
                    state::encode_list_cursor(&other_scope, millis, sequence),
                ),
                (
                    "a position newer than every record",
                    state::encode_list_cursor(&other_scope, i64::MAX, u64::MAX),
                ),
                (
                    "a position older than every record",
                    state::encode_list_cursor(&other_scope, 0, 0),
                ),
            ];

            for (row, cursor) in rows {
                let case = format!("{label} / {row}");
                let answer = list_at(&emr, home, &here, &format!("?nextToken={cursor}")).await;
                refused_as_unknown_token(&answer, &case);
            }

            // Walking the pages at the endpoint that holds the records still reports every
            // one of them, so the region in the scope costs the listing nothing.
            let case = format!("{label} / the walk at its own endpoint");
            let mut walked: Vec<String> = Vec::new();
            let mut query = "?maxResults=2".to_string();
            loop {
                let body = list_at(&emr, home, &here, &query).await.ok(&case).clone();
                walked.extend(page_ids(&body, &here, &case));
                match body.get("nextToken").and_then(Value::as_str) {
                    Some(next) => query = format!("?maxResults=2&nextToken={next}"),
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
                walked,
                here.newest_first(),
                "case {case:?}: expected the walk to report {:?}, got {walked:?}",
                here.newest_first()
            );
        }
    }
}
