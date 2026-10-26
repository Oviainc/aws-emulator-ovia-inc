// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListTrafficPolicies`.
//!
//! The rule under test: the operation REPORTS the latest version of every
//! traffic policy the account holds at the moment of the call, read from the
//! service's state in the order the policies were created in, as a page that
//! starts at the requested `TrafficPolicyIdMarker`, carries at most `MaxItems`
//! summaries, echoes that `maxitems` back and names in `TrafficPolicyIdMarker`
//! the first policy of the next page — so the pages concatenate to the whole
//! population exactly once, every summary reports the policy's LATEST version
//! (its number, that version's record type and how many versions the policy
//! holds), and nothing a caller reads changes what a later caller sees.
//!
//! The tables therefore vary the axes the rule generalises over rather than
//! renaming a policy: the POPULATION (none, then one more policy at a time),
//! the number of VERSIONS a policy holds (one, two, three — with the latest
//! version's record type differing from the first version's, so a summary that
//! reported the first version could not pass), the PAGE SIZE (every size from
//! one to one past the population, plus the page bound and one past it) and the
//! MARKER (every position in the listing, plus an id the account holds no
//! policy under). Each listed summary is cross-checked against what
//! `GetTrafficPolicy` reports for the same `(Id, LatestVersion)`, so the
//! listing is proved against another reader of the same state rather than
//! against itself.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the other traffic-policy suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::MAX_TRAFFIC_POLICIES;

use crate::hosted_zone_records::{call, router, Reply};
use crate::traffic_policies::{
    create_policy, document_with_start_endpoint, document_with_start_rule, get_policy,
    policy_count, policy_of,
};
use crate::traffic_policy_versions::create_version;

/// `TrafficPolicyName` is `len 0..512`
/// (smithy_query(query="shape",shape="TrafficPolicyName")).
const NAME_MAX_LEN: usize = 512;

/// Send one `ListTrafficPolicies` request, with `query` as its query string.
async fn list_policies(router: &MultiServiceRouter, query: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/trafficpolicies{query}"),
        "",
    )
    .await
}

/// List with `query`, asserting the call succeeded.
async fn listing(router: &MultiServiceRouter, case: &str, query: &str) -> Reply {
    let reply = list_policies(router, query).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListTrafficPolicies, got {} with body {}",
        reply.status, reply.body
    );
    reply
}

/// One `TrafficPolicySummary` as the wire reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SummaryView {
    id: String,
    name: String,
    record_type: String,
    latest_version: String,
    policy_count: String,
}

/// Every summary a listing reports, in the order it reports them.
///
/// Each member of `TrafficPolicySummary` is required, so a reply that reports
/// one summary whole reports as many of each member as it has summaries;
/// anything else is a malformed listing rather than a different order.
fn summaries_of(reply: &Reply, case: &str) -> Vec<SummaryView> {
    let ids = reply.all("TrafficPolicySummary/Id");
    let names = reply.all("TrafficPolicySummary/Name");
    let types = reply.all("TrafficPolicySummary/Type");
    let latest = reply.all("TrafficPolicySummary/LatestVersion");
    let counts = reply.all("TrafficPolicySummary/TrafficPolicyCount");
    let widths = (names.len(), types.len(), latest.len(), counts.len());
    assert_eq!(
        widths,
        (ids.len(), ids.len(), ids.len(), ids.len()),
        "case {case:?}: every listed policy must be summarised whole, got {widths:?} \
         beside {} id(s); body {}",
        ids.len(),
        reply.body
    );
    (0..ids.len())
        .map(|at| SummaryView {
            id: ids[at].to_string(),
            name: names[at].to_string(),
            record_type: types[at].to_string(),
            latest_version: latest[at].to_string(),
            policy_count: counts[at].to_string(),
        })
        .collect()
}

/// The `TrafficPolicyIdMarker` of a page: `None` once the listing has ended,
/// where there is no policy left to resume at.
fn next_marker(reply: &Reply) -> Option<String> {
    reply
        .first("TrafficPolicyIdMarker")
        .filter(|marker| !marker.is_empty())
        .map(str::to_string)
}

/// One row of the population table: a policy to create, the further versions to
/// add to it, and what every listing of the account must then report for it.
struct PolicyRow {
    case: &'static str,
    name: String,
    /// The document the policy's first version carries.
    document: String,
    /// The documents of the versions created after the first, in order.
    later_documents: Vec<String>,
    /// The `Type` of the policy's LATEST version, which is the version a
    /// summary reports.
    expect_type: &'static str,
    /// The number of the policy's latest version, and how many versions it
    /// holds.
    expect_latest_version: i32,
    expect_version_count: i32,
}

/// The population every test in this suite lists: policies that differ in how
/// many versions they hold, in the record type their latest version names, and
/// in the form and length of their name.
fn population() -> Vec<PolicyRow> {
    vec![
        PolicyRow {
            case: "one version, one endpoint",
            name: "listing-single-version".to_string(),
            document: document_with_start_endpoint("A", "192.0.2.1"),
            later_documents: Vec::new(),
            expect_type: "A",
            expect_latest_version: 1,
            expect_version_count: 1,
        },
        PolicyRow {
            case: "two versions, the latest a rule over two endpoints",
            name: "listing-two-versions".to_string(),
            document: document_with_start_endpoint("A", "192.0.2.2"),
            later_documents: vec![document_with_start_rule(
                "CNAME",
                "a.example.com",
                "b.example.com",
            )],
            expect_type: "CNAME",
            expect_latest_version: 2,
            expect_version_count: 2,
        },
        PolicyRow {
            case: "three versions, each of a different record type",
            name: "listing-three-versions".to_string(),
            document: document_with_start_endpoint("A", "192.0.2.3"),
            later_documents: vec![
                document_with_start_endpoint("MX", "10 mail.example.com"),
                document_with_start_endpoint("AAAA", "2001:db8::3"),
            ],
            expect_type: "AAAA",
            expect_latest_version: 3,
            expect_version_count: 3,
        },
        PolicyRow {
            case: "a name carrying XML special characters",
            name: "quoted & escaped <listing>".to_string(),
            document: document_with_start_endpoint("TXT", "v=spf1 ~all"),
            later_documents: Vec::new(),
            expect_type: "TXT",
            expect_latest_version: 1,
            expect_version_count: 1,
        },
        PolicyRow {
            case: "a name at the 512-character maximum",
            name: "n".repeat(NAME_MAX_LEN),
            document: document_with_start_rule("SRV", "1 1 80 a.example.com", "1 1 80 b.example.com"),
            later_documents: Vec::new(),
            expect_type: "SRV",
            expect_latest_version: 1,
            expect_version_count: 1,
        },
    ]
}

/// Create the policy `row` describes, with every version it names, and return
/// the summary the listing must report for it.
async fn create_row(router: &MultiServiceRouter, row: &PolicyRow) -> SummaryView {
    let created = create_policy(router, &row.name, &row.document, Some(row.case)).await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected 201 from CreateTrafficPolicy, got {} with body {}",
        row.case, created.status, created.body
    );
    let id = policy_of(&created, row.case).id;
    for (added, document) in row.later_documents.iter().enumerate() {
        let version = create_version(router, &id, document, None).await;
        assert_eq!(
            version.status, 201,
            "case {:?}: expected 201 from CreateTrafficPolicyVersion {}, got {} with body {}",
            row.case,
            added + 2,
            version.status,
            version.body
        );
    }
    SummaryView {
        id,
        name: row.name.clone(),
        record_type: row.expect_type.to_string(),
        latest_version: row.expect_latest_version.to_string(),
        policy_count: row.expect_version_count.to_string(),
    }
}

/// Create every row of [`population`], in order, and return the summaries the
/// listing must report, in that same creation order.
async fn populate(router: &MultiServiceRouter) -> Vec<SummaryView> {
    let mut expected = Vec::new();
    for row in &population() {
        expected.push(create_row(router, row).await);
    }
    expected
}

#[tokio::test]
async fn the_listing_reports_the_latest_version_of_every_policy_the_account_holds() {
    // The population is the varying axis: none, then one more policy after
    // every row — so the listing is asserted against a set that grows by a
    // policy holding a different number of versions each time, and the order it
    // reports is the order the policies were created in (a policy id is minted
    // opaque, so that is NOT the order of the ids).
    let router = router();
    let mut expected: Vec<SummaryView> = Vec::new();

    for row in &population() {
        let case = format!(
            "{} policy/policies, before creating {}",
            expected.len(),
            row.case
        );
        let reply = listing(&router, &case, "").await;
        let listed = summaries_of(&reply, &case);
        assert_eq!(
            listed, expected,
            "case {case:?}: the listing must report every policy the account holds, in the \
             order they were created in; body {}",
            reply.body
        );
        assert!(
            reply.first("IsTruncated") != Some("true"),
            "case {case:?}: a listing that fits one page must not be truncated; body {}",
            reply.body
        );
        assert_eq!(
            next_marker(&reply),
            None,
            "case {case:?}: an untruncated listing names no policy to resume at; body {}",
            reply.body
        );

        // Every listed summary is cross-checked against what GetTrafficPolicy
        // reports for the version the summary says is the latest.
        for summary in &listed {
            let read = get_policy(&router, &summary.id, &summary.latest_version).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the latest version of {} must be readable, got {} with body {}",
                summary.id, read.status, read.body
            );
            let policy = policy_of(&read, &case);
            let actual = (
                policy.id.clone(),
                policy.name.clone(),
                policy.record_type.clone(),
                policy.version.clone(),
            );
            let wanted = (
                summary.id.clone(),
                summary.name.clone(),
                summary.record_type.clone(),
                summary.latest_version.clone(),
            );
            assert_eq!(
                actual, wanted,
                "case {case:?}: the summary must report the policy as GetTrafficPolicy does, \
                 got {actual:?}; body {}",
                read.body
            );
            // The version one past the one the summary calls the latest is not
            // a version the policy holds.
            let past = summary
                .latest_version
                .parse::<i32>()
                .expect("the reported latest version must be a number")
                + 1;
            let beyond = get_policy(&router, &summary.id, &past.to_string()).await;
            assert_eq!(
                beyond.status, 404,
                "case {case:?}: version {past} of {} must not exist, got {} with body {}",
                summary.id, beyond.status, beyond.body
            );
        }

        expected.push(create_row(&router, row).await);
    }
}

#[tokio::test]
async fn traffic_policy_paging_covers_the_listing_exactly_once() {
    // The rule as an invariant: a page holds at most MaxItems summaries and
    // TrafficPolicyIdMarker names where the next page starts, so the pages
    // concatenate to the full listing for EVERY page size. The page size is the
    // varying axis.
    let router = router();
    let created = populate(&router).await;

    let unpaged = listing(&router, "the whole listing", "").await;
    let all = summaries_of(&unpaged, "the whole listing");
    assert_eq!(
        all, created,
        "the whole listing must report every created policy in creation order; body {}",
        unpaged.body
    );

    for page_size in 1..=created.len() + 1 {
        let mut walked: Vec<SummaryView> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let case = format!("maxitems={page_size}, marker={marker:?}");
            let query = match &marker {
                Some(resume) => format!("?maxitems={page_size}&trafficpolicyid={resume}"),
                None => format!("?maxitems={page_size}"),
            };
            let page = listing(&router, &case, &query).await;
            assert_eq!(
                page.first("MaxItems"),
                Some(page_size.to_string()).as_deref(),
                "case {case:?}: the response must echo the request's maxitems; body {}",
                page.body
            );
            let summaries = summaries_of(&page, &case);
            assert!(
                summaries.len() <= page_size,
                "case {case:?}: a page held {} summaries; body {}",
                summaries.len(),
                page.body
            );
            walked.extend(summaries);
            pages += 1;
            assert!(
                pages <= created.len() + 2,
                "case {case:?}: paging did not terminate"
            );
            if page.first("IsTruncated") != Some("true") {
                assert_eq!(
                    next_marker(&page),
                    None,
                    "case {case:?}: a final page names no policy to resume at; body {}",
                    page.body
                );
                break;
            }
            let resume = next_marker(&page).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a truncated page must name the policy the next page \
                     starts at; body {}",
                    page.body
                )
            });
            assert_eq!(
                Some(resume.as_str()),
                all.get(walked.len()).map(|summary| summary.id.as_str()),
                "case {case:?}: the marker must name the first policy of the next page; body {}",
                page.body
            );
            marker = Some(resume);
        }
        assert_eq!(
            walked, all,
            "case maxitems={page_size}: paging must cover the listing exactly once"
        );
    }
}

#[tokio::test]
async fn a_marker_resumes_the_listing_at_the_policy_it_names() {
    // The marker is "the ID of the first traffic policy in the next group", so
    // a listing resumed at any position in it reports exactly the tail from
    // that position. Every position is a row, plus an id the account holds no
    // policy under, which positions nothing.
    let router = router();
    let all = populate(&router).await;
    assert!(
        all.len() > 1,
        "the suite needs several policies to resume at, got {}",
        all.len()
    );

    for (at, summary) in all.iter().enumerate() {
        let case = format!("trafficpolicyid={}", summary.id);
        let page = listing(&router, &case, &format!("?trafficpolicyid={}", summary.id)).await;
        let tail = summaries_of(&page, &case);
        assert_eq!(
            tail,
            all[at..].to_vec(),
            "case {case:?}: the listing must resume at the policy the marker names; body {}",
            page.body
        );
    }

    // An id the account holds no policy under names no position in the
    // listing, so the request is refused rather than answered from somewhere
    // else. The rows are an id of the minted form and a shorter one.
    for unknown in [
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "no-such-traffic-policy",
    ] {
        let page = list_policies(&router, &format!("?trafficpolicyid={unknown}")).await;
        assert_eq!(
            page.status, 400,
            "case trafficpolicyid={unknown:?}: expected 400, got {} with body {}",
            page.status, page.body
        );
        assert_eq!(
            page.error_code(),
            "InvalidInput",
            "case trafficpolicyid={unknown:?}: expected the modelled InvalidInput, got {:?}; \
             body {}",
            page.error_code(),
            page.body
        );
        assert!(
            page.first("Error/Message").unwrap_or("").contains(unknown),
            "case trafficpolicyid={unknown:?}: the refusal must name the marker it could not \
             place; body {}",
            page.body
        );
    }
}

#[tokio::test]
async fn the_listing_echoes_the_page_size_it_was_asked_for() {
    // `MaxItems` in the response is "the value that you specified for the
    // MaxItems parameter", so it is echoed whatever the caller sent — while the
    // page itself never exceeds the account's traffic policy quota, which no
    // listing can be longer than. The rows are a size below the population,
    // that bound, and one past it.
    let router = router();
    let created = populate(&router).await;
    let page_bound = MAX_TRAFFIC_POLICIES;

    for requested in [1, 2, page_bound, page_bound + 1] {
        let case = format!("maxitems={requested}");
        let page = listing(&router, &case, &format!("?maxitems={requested}")).await;
        let echoed = page.first("MaxItems").unwrap_or("");
        assert_eq!(
            echoed,
            requested.to_string(),
            "case {case:?}: expected the requested maxitems echoed, got {echoed:?}; body {}",
            page.body
        );
        let summaries = summaries_of(&page, &case);
        let wanted = requested.min(page_bound).min(created.len());
        assert_eq!(
            summaries.len(),
            wanted,
            "case {case:?}: expected {wanted} summary/summaries, got {}; body {}",
            summaries.len(),
            page.body
        );
        let truncated = page.first("IsTruncated") == Some("true");
        assert_eq!(
            truncated,
            summaries.len() < created.len(),
            "case {case:?}: IsTruncated must say whether more policies remain; body {}",
            page.body
        );
    }

    // With no maxitems at all the whole population comes back in one page.
    let whole = listing(&router, "no maxitems", "").await;
    assert_eq!(
        summaries_of(&whole, "no maxitems"),
        created,
        "case {:?}: a request without maxitems must list every policy; body {}",
        "no maxitems",
        whole.body
    );
}

#[tokio::test]
async fn listing_the_traffic_policies_leaves_the_account_exactly_as_it_was() {
    // The rule is a report: repeating it answers the same listing, creates and
    // consumes nothing, and the population it reports still moves with a later
    // create — which the creation order puts at the END of the listing.
    let router = router();
    let created = populate(&router).await;
    let before = policy_count(&router).await;

    for attempt in 1..=3 {
        let case = format!("listing {attempt}");
        let again = summaries_of(&listing(&router, &case, "").await, &case);
        assert_eq!(
            again, created,
            "case {case:?}: a read must report what the first read did"
        );
        let counted = policy_count(&router).await;
        assert_eq!(
            counted, before,
            "case {case:?}: listing must not change how many policies the account holds"
        );
    }

    // Every listed policy is still readable at the version the listing calls
    // its latest, so no read moved a version on.
    for summary in &created {
        let read = get_policy(&router, &summary.id, &summary.latest_version).await;
        assert_eq!(
            read.status, 200,
            "the listed policy {} must still be readable, got {} with body {}",
            summary.id, read.status, read.body
        );
    }

    let added = create_row(
        &router,
        &PolicyRow {
            case: "a policy created after the listing",
            name: "listing-after-the-read".to_string(),
            document: document_with_start_endpoint("A", "192.0.2.9"),
            later_documents: Vec::new(),
            expect_type: "A",
            expect_latest_version: 1,
            expect_version_count: 1,
        },
    )
    .await;
    let mut expected = created.clone();
    expected.push(added);
    let case = "after a later create";
    let after = summaries_of(&listing(&router, case, "").await, case);
    assert_eq!(
        after, expected,
        "case {case:?}: the listing must follow the state, with the newest policy last"
    );
}
