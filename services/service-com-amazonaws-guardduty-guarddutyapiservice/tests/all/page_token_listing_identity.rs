// SPDX-License-Identifier: CC0-1.0

//! A page token belongs to the LISTING that minted it, end to end over the wire.
//!
//! Every request goes through `MultiServiceRouter::dispatch` (the harness lives in the
//! [`super::detector_lifecycle`] suite), so routing, restJson1 (de)serialization and the
//! handlers are exercised together.
//!
//! The rule under test, asserted over a SET of listings rather than one recorded call:
//! a `nextToken` names a position in ONE ordered list — the report that issued it, over
//! the request values that select and order its entries — so a token handed to any
//! other listing is refused with HTTP 400 under this service's `InvalidInputException`
//! class naming `nextToken`, instead of being answered with a page of a list the caller
//! never paged. Handed back to the listing that minted it, the same token resumes that
//! list at the entry after the page it ended, whatever page size the next call asks
//! for: the page size selects nothing and orders nothing, so it is not part of a
//! listing's identity.
//!
//! The rows span the ways two listings can differ:
//!
//! 1. the `usageStatisticsType` one `GetUsageStatistics` call aggregates by, each over a
//!    key list of its own (accounts, features, data sources, resources);
//! 2. the OPERATION reporting the list;
//! 3. the criteria SELECTING its entries, and the criteria ORDERING them, on one
//!    operation — including a `findingCriteria` carrying several criterion keys, whose
//!    identity is the SET of criteria the request named and not the order the map one
//!    request was deserialized into hands them out in;
//! 4. the detector and Region the list belongs to, on one operation.
//!
//! Both sides of every crossed pair hold at least two entries, so the position a
//! foreign token names is one the receiving listing could have answered: each refusal is
//! about the token naming another listing and not about the position being out of range.

use serde_json::{json, Value};

use super::client_rejections::page_token_refusal;
use super::detector_lifecycle::{call_in, enabled_detector, error_code, new_router, Router};
use super::invitation_listing::enabled_detector_in;

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The Region the detector under test is created through.
const REGION: &str = "us-east-1";

/// A second Region, whose endpoint serves a detector of its own.
const OTHER_REGION: &str = "us-west-2";

/// Finding types spanning five different features, so the usage aggregations have
/// several rows each, and spanning two affected resource types, so a criterion over
/// `resource.resourceType` selects a strict subset. They come from this service's
/// sample-finding catalogue, which is what `CreateSampleFindings` generates.
const FINDING_TYPES: [&str; 5] = [
    "Recon:EC2/Portscan",
    "Backdoor:EC2/C&CActivity.B!DNS",
    "PenTest:IAMUser/PentooLinux",
    "Discovery:S3/MaliciousIPCaller",
    "Policy:Kubernetes/ExposedDashboard",
];

/// Two EC2 resources a malware scan runs over, in the ARN forms `StartMalwareScan`
/// accepts, so the resource aggregations have more than one row.
const SCANNED_ARNS: [&str; 2] = [
    "arn:aws:ec2:us-east-1:123456789012:instance/i-0abc123def4567890",
    "arn:aws:ec2:us-east-1:123456789012:volume/vol-0abc123def4567890",
];

/// Every `UsageStatisticType`, with the `UsageStatistics` member it fills and the
/// member each of that list's entries carries its key under. Source:
/// `smithy_query(shape="UsageStatisticType")` beside
/// `smithy_query(shape="UsageStatistics")`.
const USAGE_STATISTICS: [(&str, &str, &str); 6] = [
    ("SUM_BY_ACCOUNT", "sumByAccount", "accountId"),
    ("SUM_BY_DATA_SOURCE", "sumByDataSource", "dataSource"),
    ("SUM_BY_FEATURES", "sumByFeature", "feature"),
    ("SUM_BY_RESOURCE", "sumByResource", "resource"),
    ("TOP_ACCOUNTS_BY_FEATURE", "topAccountsByFeature", "feature"),
    ("TOP_RESOURCES", "topResources", "resource"),
];

/// One ordered list a caller can page: the report, over the values that select and
/// order its entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listed {
    /// `ListFindings` over every finding the detector holds.
    Findings,
    /// `ListFindings` narrowed to the findings on one affected resource type — the same
    /// operation over a SELECTION of the same records.
    FindingsOnInstances,
    /// `ListFindings` under a criteria of THREE criterion keys. `FindingCriteria`
    /// carries its criteria in a map, so this is the row that pins the identity on the
    /// SET of criteria the request named and not on the order one deserialized map
    /// hands them out in.
    FindingsUnderThreeCriteria,
    /// `ListFindings` under the same three criterion KEYS, one of them conditioned
    /// differently: another selection, so another listing.
    FindingsUnderThreeOtherCriteria,
    /// `ListFindings` over every finding, ordered by severity — the same operation and
    /// the same records in another ORDER.
    FindingsBySeverity,
    /// `ListIPSets`.
    IpSets,
    /// `ListFilters`.
    Filters,
    /// `GetUsageStatistics` for one `usageStatisticsType`.
    Usage(&'static str),
}

/// Every listing the crossing tests page, in one table.
const LISTED: [Listed; 13] = [
    Listed::Findings,
    Listed::FindingsOnInstances,
    Listed::FindingsUnderThreeCriteria,
    Listed::FindingsUnderThreeOtherCriteria,
    Listed::FindingsBySeverity,
    Listed::IpSets,
    Listed::Filters,
    Listed::Usage(USAGE_STATISTICS[0].0),
    Listed::Usage(USAGE_STATISTICS[1].0),
    Listed::Usage(USAGE_STATISTICS[2].0),
    Listed::Usage(USAGE_STATISTICS[3].0),
    Listed::Usage(USAGE_STATISTICS[4].0),
    Listed::Usage(USAGE_STATISTICS[5].0),
];

impl Listed {
    /// Ask this listing for one page: `max_results` entries (the service's own page when
    /// `None`), resuming at `token` when one is given.
    async fn page(
        self,
        router: &Router,
        region: &str,
        detector_id: &str,
        max_results: Option<i64>,
        token: Option<&str>,
    ) -> (u16, Value) {
        match self {
            Self::Findings
            | Self::FindingsOnInstances
            | Self::FindingsUnderThreeCriteria
            | Self::FindingsUnderThreeOtherCriteria
            | Self::FindingsBySeverity => {
                let mut body = json!({});
                if self == Self::FindingsOnInstances {
                    // `resource.resourceType` is a field of a stored finding, and
                    // `Instance` is what the EC2 sample findings record under it.
                    body["findingCriteria"] =
                        json!({"criterion": {"resource.resourceType": {"equals": ["Instance"]}}});
                }
                if self == Self::FindingsUnderThreeCriteria {
                    body["findingCriteria"] = json!({"criterion": {
                        "severity": {"greaterThanOrEqual": 1},
                        "service.archived": {"equals": ["false"]},
                        "resource.resourceType": {"notEquals": ["S3Bucket"]}
                    }});
                }
                if self == Self::FindingsUnderThreeOtherCriteria {
                    body["findingCriteria"] = json!({"criterion": {
                        "severity": {"greaterThanOrEqual": 1},
                        "service.archived": {"equals": ["false"]},
                        "resource.resourceType": {"equals": ["Instance"]}
                    }});
                }
                if self == Self::FindingsBySeverity {
                    body["sortCriteria"] = json!({"attributeName": "severity", "orderBy": "DESC"});
                }
                if let Some(size) = max_results {
                    body["maxResults"] = json!(size);
                }
                if let Some(token) = token {
                    body["nextToken"] = json!(token);
                }
                call_in(
                    router,
                    region,
                    "POST",
                    &format!("/detector/{detector_id}/findings"),
                    Some(body),
                )
                .await
            }
            Self::IpSets | Self::Filters => {
                let collection = if self == Self::IpSets { "ipset" } else { "filter" };
                let mut parameters: Vec<String> = Vec::new();
                if let Some(size) = max_results {
                    parameters.push(format!("maxResults={size}"));
                }
                if let Some(token) = token {
                    parameters.push(format!("nextToken={token}"));
                }
                let query = if parameters.is_empty() {
                    String::new()
                } else {
                    format!("?{}", parameters.join("&"))
                };
                call_in(
                    router,
                    region,
                    "GET",
                    &format!("/detector/{detector_id}/{collection}{query}"),
                    None,
                )
                .await
            }
            Self::Usage(statistic) => {
                let mut body = json!({"usageStatisticsType": statistic, "usageCriteria": {}});
                if let Some(size) = max_results {
                    body["maxResults"] = json!(size);
                }
                if let Some(token) = token {
                    body["nextToken"] = json!(token);
                }
                call_in(
                    router,
                    region,
                    "POST",
                    &format!("/detector/{detector_id}/usage/statistics"),
                    Some(body),
                )
                .await
            }
        }
    }

    /// The keys one page of this listing reported, in the order it reported them.
    fn entries(self, body: &Value, case: &str) -> Vec<String> {
        match self {
            Self::Findings
            | Self::FindingsOnInstances
            | Self::FindingsUnderThreeCriteria
            | Self::FindingsUnderThreeOtherCriteria
            | Self::FindingsBySeverity => strings(&body["findingIds"], case),
            Self::IpSets => strings(&body["ipSetIds"], case),
            Self::Filters => strings(&body["filterNames"], case),
            Self::Usage(statistic) => {
                let (_, member, key) = USAGE_STATISTICS
                    .iter()
                    .find(|(name, _, _)| *name == statistic)
                    .unwrap_or_else(|| panic!("case {case:?}: {statistic} is not a modelled statistic"));
                body["usageStatistics"][member]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|row| {
                        row[key]
                            .as_str()
                            .unwrap_or_else(|| {
                                panic!("case {case:?}: expected a {key} on every row, got {row:?}")
                            })
                            .to_string()
                    })
                    .collect()
            }
        }
    }

    /// One page of this listing, failing the test unless the call succeeded.
    async fn page_ok(
        self,
        router: &Router,
        region: &str,
        detector_id: &str,
        max_results: Option<i64>,
        token: Option<&str>,
        case: &str,
    ) -> (Vec<String>, Option<String>) {
        let (status, body) = self.page(router, region, detector_id, max_results, token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from {self:?}, got {status} {body:?}"
        );
        // A listing reports no token once its last page is reached; `GetUsageStatistics`
        // reports the terminal one as the EMPTY string instead, which names no page
        // either, so both read as "there is no page after this one".
        let next = body["nextToken"]
            .as_str()
            .filter(|next| !next.is_empty())
            .map(str::to_string);
        (self.entries(&body, case), next)
    }
}

/// The strings a reported list carries, failing the test on an entry that is not one.
fn strings(value: &Value, case: &str) -> Vec<String> {
    value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a string entry, got {entry:?}"))
                .to_string()
        })
        .collect()
}

/// An enabled detector in `region` holding the sample findings, three IPSets and three
/// filters, with two malware scans recorded against it — the work every listing below
/// is a view of.
async fn detector_with_listings(router: &Router, region: &str, detector_id: &str) {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": FINDING_TYPES})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-sample-findings in {region}: expected 200, got {status} {body:?}"
    );

    // Names created out of sorted order, so a page is a window of the LIST's order and
    // not of the creation order. Only the first list asks for activation: the
    // account's allowance for the lists GuardDuty is USING is
    // `ACTIVATED_IP_SETS_PER_ACCOUNT` in a Region.
    for name in ["zeta", "alpha", "mid"] {
        let (status, body) = call_in(
            router,
            region,
            "POST",
            &format!("/detector/{detector_id}/ipset"),
            Some(json!({
                "name": format!("ipset-{name}"),
                "format": "TXT",
                "location": format!("s3://my-bucket/lists/{name}.txt"),
                "activate": name == "zeta"
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "create-ip-set {name} in {region}: expected 200, got {status} {body:?}"
        );
        let (status, body) = call_in(
            router,
            region,
            "POST",
            &format!("/detector/{detector_id}/filter"),
            Some(json!({
                "name": format!("filter-{name}"),
                "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 1}}}
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "create-filter {name} in {region}: expected 200, got {status} {body:?}"
        );
    }

    for arn in SCANNED_ARNS {
        let (status, body) = call_in(
            router,
            region,
            "POST",
            "/malware-scan/start",
            Some(json!({"resourceArn": arn})),
        )
        .await;
        assert_eq!(
            status, 200,
            "start-malware-scan {arn} in {region}: expected 200, got {status} {body:?}"
        );
    }
}

/// A router holding one such detector in [`REGION`], with its id.
async fn router_with_listings() -> (Router, String) {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    detector_with_listings(&router, REGION, &detector_id).await;
    (router, detector_id)
}

/// Every listing of [`LISTED`] that holds at least two entries, with those entries in
/// the order the unpaged answer reports them: both sides of a crossed pair come from
/// here, so the position a token names is in range for either.
async fn pageable(router: &Router, detector_id: &str) -> Vec<(Listed, Vec<String>)> {
    let mut pageable = Vec::new();
    for listed in LISTED {
        let case = format!("{listed:?} unpaged");
        let (entries, next) = listed
            .page_ok(router, REGION, detector_id, None, None, &case)
            .await;
        assert!(
            next.is_none(),
            "case {case:?}: the whole list fits in one page here, so it must carry no nextToken, got {next:?}"
        );
        if entries.len() >= 2 {
            pageable.push((listed, entries));
        }
    }
    pageable
}

/// Assert one refusal: HTTP 400 under this service's rejection class, with the ONE
/// page-token sentence the whole pagination surface answers — which names no member
/// — and quoting the token back nowhere.
fn assert_refused(case: &str, status: u16, body: &Value, token: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the token to be refused, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(page_token_refusal().as_str()),
        "case {case:?}: expected the page-token refusal, got {body:?}"
    );
    assert!(
        !body["message"]
            .as_str()
            .unwrap_or_default()
            .contains(token),
        "case {case:?}: the refusal must not quote the token back, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// A token resolves against the listing that minted it, and against no other.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_token_one_listing_minted_is_refused_by_every_other_listing() {
    let (router, detector_id) = router_with_listings().await;
    let pageable = pageable(&router, &detector_id).await;
    assert!(
        pageable.len() >= 10,
        "expected at least ten listings of two or more entries to cross, got {:?}",
        pageable
            .iter()
            .map(|(listed, entries)| (*listed, entries.len()))
            .collect::<Vec<_>>()
    );

    for (minter, minted) in &pageable {
        let case = format!("{minter:?} at maxResults=1");
        let (first, token) = minter
            .page_ok(&router, REGION, &detector_id, Some(1), None, &case)
            .await;
        assert_eq!(
            first,
            minted[..1],
            "case {case:?}: the first page must be the first entry of the unpaged order, got {first:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!(
                "case {case:?}: a listing of {} entries owes a nextToken after one of them",
                minted.len()
            )
        });

        // The listing that minted it resumes at the entry after that page.
        let case = format!("{minter:?} resuming its own token");
        let (resumed, _) = minter
            .page_ok(
                &router,
                REGION,
                &detector_id,
                None,
                Some(&token),
                &case,
            )
            .await;
        assert_eq!(
            resumed,
            minted[1..],
            "case {case:?}: the token must resume at the entry after the page, got {resumed:?}"
        );

        // Every other listing refuses it, each of them holding entries enough for the
        // position it names.
        for (other, other_entries) in &pageable {
            if other == minter {
                continue;
            }
            let case = format!(
                "a {minter:?} token handed to {other:?}, which holds {} entries",
                other_entries.len()
            );
            let (status, body) = other
                .page(&router, REGION, &detector_id, None, Some(&token))
                .await;
            assert_refused(&case, status, &body, &token);
        }
    }
}

// ---------------------------------------------------------------------------
// The page size selects nothing and orders nothing, so it is not part of the
// identity a token carries.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_token_resumes_its_own_listing_whatever_page_size_the_next_call_asks_for() {
    let (router, detector_id) = router_with_listings().await;
    let pageable = pageable(&router, &detector_id).await;
    assert!(
        !pageable.is_empty(),
        "expected a listing of two or more entries to page"
    );

    // The size that minted the token, and the size the next call asks for. 1 and 50 are
    // the published bounds of `MaxResults` on these operations
    // (`smithy_query(operation="ListFindings")` reports range 1..50), with 2 between
    // them.
    for (minting_size, resuming_size) in [(1, 1), (1, 2), (2, 1), (1, 50), (2, 50)] {
        for (listed, entries) in &pageable {
            let skipped = usize::try_from(minting_size).unwrap_or(0);
            if entries.len() <= skipped {
                continue;
            }
            let case = format!(
                "{listed:?} minted at maxResults={minting_size}, resumed at maxResults={resuming_size}"
            );
            let (_, token) = listed
                .page_ok(
                    &router,
                    REGION,
                    &detector_id,
                    Some(minting_size),
                    None,
                    &case,
                )
                .await;
            let token = token.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a listing of {} entries owes a nextToken after {skipped} of them",
                    entries.len()
                )
            });
            let (resumed, _) = listed
                .page_ok(
                    &router,
                    REGION,
                    &detector_id,
                    Some(resuming_size),
                    Some(&token),
                    &case,
                )
                .await;
            let wanted = usize::try_from(resuming_size).unwrap_or(entries.len());
            let end = skipped.saturating_add(wanted).min(entries.len());
            assert_eq!(
                resumed,
                entries[skipped..end],
                "case {case:?}: expected the window [{skipped}..{end}) of the unpaged order, got {resumed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A criteria-bearing listing is identified by the SET of criteria the request
// named, so the same request pages the same list on every call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_listing_under_several_criteria_resumes_its_own_token_on_every_call() {
    let (router, detector_id) = router_with_listings().await;

    for listed in [
        Listed::FindingsUnderThreeCriteria,
        Listed::FindingsUnderThreeOtherCriteria,
    ] {
        let case = format!("{listed:?} unpaged");
        let (entries, _) = listed
            .page_ok(&router, REGION, &detector_id, None, None, &case)
            .await;
        assert!(
            entries.len() >= 2,
            "case {case:?}: the criteria must select entries enough to page, got {entries:?}"
        );

        // Repeated over fresh calls: a criteria map's iteration order is private to
        // the instance ONE request was deserialized into, so an identity taken from
        // that order would resolve on some calls and not on others.
        for attempt in 0..12 {
            let case = format!("{listed:?} minted and resumed, attempt {attempt}");
            let (_, token) = listed
                .page_ok(&router, REGION, &detector_id, Some(1), None, &case)
                .await;
            let token = token.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a listing of {} entries owes a nextToken after one of them",
                    entries.len()
                )
            });
            let (resumed, _) = listed
                .page_ok(&router, REGION, &detector_id, None, Some(&token), &case)
                .await;
            assert_eq!(
                resumed,
                entries[1..],
                "case {case:?}: the token must resume the list that minted it, got {resumed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A listing belongs to its detector, which is regional: the same operation on
// another Region's detector is another listing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_token_from_another_detectors_listing_of_the_same_operation_is_refused() {
    let router = new_router();
    let home = enabled_detector(&router).await;
    detector_with_listings(&router, REGION, &home).await;
    let away = enabled_detector_in(&router, OTHER_REGION).await;
    detector_with_listings(&router, OTHER_REGION, &away).await;

    // Both directions, over both a GET-paged listing and a POST-paged one.
    for listed in [Listed::IpSets, Listed::Findings] {
        for (minting_region, minting_detector, consuming_region, consuming_detector) in [
            (REGION, &home, OTHER_REGION, &away),
            (OTHER_REGION, &away, REGION, &home),
        ] {
            let case = format!("{listed:?} in {minting_region} at maxResults=1");
            let (_, token) = listed
                .page_ok(
                    &router,
                    minting_region,
                    minting_detector,
                    Some(1),
                    None,
                    &case,
                )
                .await;
            let token = token
                .unwrap_or_else(|| panic!("case {case:?}: expected a nextToken to carry over"));

            let unpaged_case = format!("{listed:?} in {consuming_region} unpaged");
            let (consuming_entries, _) = listed
                .page_ok(
                    &router,
                    consuming_region,
                    consuming_detector,
                    None,
                    None,
                    &unpaged_case,
                )
                .await;
            let case = format!(
                "a {listed:?} token from {minting_region} handed to {consuming_region}, which holds {} entries",
                consuming_entries.len()
            );
            assert!(
                consuming_entries.len() >= 2,
                "case {case:?}: the receiving listing must hold entries enough for the position the token names, got {consuming_entries:?}"
            );
            let (status, body) = listed
                .page(
                    &router,
                    consuming_region,
                    consuming_detector,
                    None,
                    Some(&token),
                )
                .await;
            assert_refused(&case, status, &body, &token);

            // The listing that minted it still resumes it, so the refusal above is
            // about the listing and not about the token having been used.
            let own_case = format!("{listed:?} in {minting_region} resuming its own token");
            let (resumed, _) = listed
                .page_ok(
                    &router,
                    minting_region,
                    minting_detector,
                    None,
                    Some(&token),
                    &own_case,
                )
                .await;
            assert!(
                !resumed.is_empty(),
                "case {own_case:?}: the token must resume the list it was minted in, got {resumed:?}"
            );
        }
    }
}
