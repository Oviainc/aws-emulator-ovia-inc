// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetIpamDiscoveredAccounts`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the accounts a resource discovery reports for a Region are DERIVED from the store: the
//!    one account this emulator holds every resource of is the only account a discovery of it
//!    monitors, so it is reported once for the Region asked about, carrying that Region, the
//!    instants the discovery was last attempted and last succeeded at, no failure reason and
//!    no organizational unit — and the account it names is the account
//!    `DescribeIpamResourceDiscoveries` reports as the discovery's owner;
//! 2. the report is of the Region asked about, through a discovery that OPERATES in it: a
//!    Region the discovery does not operate in has no discovered account rather than another
//!    Region's;
//! 3. `Filters` narrow the report by any member of the reported account (and a filter name the
//!    operation does not resolve is refused rather than read as "no results"), and `MaxResults`
//!    pages it in account order, a page nothing follows carrying no `NextToken`;
//! 4. a request naming something that is not a resource discovery id, a well-formed id the
//!    account holds no discovery for, an empty `DiscoveryRegion`, a page size outside the
//!    modelled range or a `NextToken` this service did not issue is refused with EC2's own
//!    code, and `DryRun` on a request that would have succeeded reports that.
//!
//! Each table row is a distinct input along what those rules generalise over — which Region is
//! asked about and which Regions the discovery operates in, which member a filter names, in
//! what form (exact, wildcard, several values) and whether it selects the account or nothing,
//! how large a page is asked for, and which member of the request is at fault — so no row is
//! the specification.

use service_com_amazonaws_ec2_amazonec2::state::{
    DRY_RUN_OPERATION, INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
    INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND, INVALID_MAX_RESULTS, INVALID_PAGINATION_TOKEN,
    INVALID_PARAMETER_VALUE, MISSING_PARAMETER,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
use super::wire::{child, child_text, document, Param};

/// The Region every call here is signed for, and the home Region of each IPAM it creates.
const REGION: &str = "us-west-2";

/// A second Region of the same partition, for the discoveries and requests that are NOT the
/// home Region's.
const OTHER_REGION: &str = "us-east-1";

/// A third Region, so a discovery operating in more than two is covered as well.
const THIRD_REGION: &str = "eu-west-1";

/// A Region of the same partition that no discovery here operates in.
const UNMONITORED_REGION: &str = "ap-southeast-1";

/// The action under test.
const ACTION: &str = "GetIpamDiscoveredAccounts";

/// A twelve-digit account id that is NOT the one this emulator holds every resource of, for the
/// filter rows that must select nothing.
const OTHER_ACCOUNT: &str = "123456789012";

/// One member of the modelled `IpamDiscoveryFailureCode` catalog
/// (`smithy_query(query="shape",shape="IpamDiscoveryFailureCode")`), for the filter row that
/// must select nothing because no discovery of this emulator fails.
const ASSUME_ROLE_FAILURE: &str = "assume-role-failure";

fn emulator() -> MultiServiceRouter {
    ec2_query::emulator().0
}

/// One form field: `("DiscoveryRegion", "us-west-2")` as the call helpers want it.
fn p(key: &str, value: impl Into<String>) -> Param {
    (key.to_string(), value.into())
}

/// One call of `action` signed for `region`.
async fn api(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[Param],
) -> (u16, String) {
    ec2_query::call(router, region, action, params).await
}

/// One call of `action` signed for [`REGION`] that must succeed, as the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Create one IPAM operating in `regions` beside its home Region, and answer the id of the
/// default resource discovery that came up with it: "the default resource discovery is the
/// resource discovery automatically created when you create an IPAM"
/// (`IpamResourceDiscovery$IsDefault`).
async fn default_discovery(case: &str, router: &MultiServiceRouter, regions: &[&str]) -> String {
    let params: Vec<Param> = regions
        .iter()
        .enumerate()
        .map(|(index, region)| p(&format!("OperatingRegion.{}.RegionName", index + 1), *region))
        .collect();
    let body = ok(case, router, "CreateIpam", &params).await;
    ec2_query::element(&body, "defaultResourceDiscoveryId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateIpam must report the default resource discovery: {body}")
    })
}

/// The members of one discovered account the assertions below read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Discovered {
    account_id: String,
    discovery_region: Option<String>,
    last_attempted: Option<String>,
    last_successful: Option<String>,
    organizational_unit_id: Option<String>,
    failure_code: Option<String>,
}

/// Every account of one `GetIpamDiscoveredAccounts` response, in the order it reported them.
///
/// Read through the XML parser rather than by substring, so a member of the nested failure
/// reason is never mistaken for one of the account's own.
fn discovered(case: &str, body: &str) -> Vec<Discovered> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .filter(|node| child(*node, "accountId").is_some())
        .map(|item| Discovered {
            account_id: child_text(item, "accountId").unwrap_or_default(),
            discovery_region: child_text(item, "discoveryRegion"),
            last_attempted: child_text(item, "lastAttemptedDiscoveryTime"),
            last_successful: child_text(item, "lastSuccessfulDiscoveryTime"),
            organizational_unit_id: child_text(item, "organizationalUnitId"),
            failure_code: child(item, "failureReason").and_then(|reason| child_text(reason, "code")),
        })
        .collect()
}

/// One report request for `region` through `discovery`, with `extra` beside it.
fn report_params(discovery: &str, region: &str, extra: Vec<Param>) -> Vec<Param> {
    let mut params = vec![
        p("IpamResourceDiscoveryId", discovery),
        p("DiscoveryRegion", region),
    ];
    params.extend(extra);
    params
}

/// The account ids one report named, sorted — the set a row's expectation names.
fn reported_accounts(case: &str, body: &str) -> Vec<String> {
    let mut accounts: Vec<String> = discovered(case, body)
        .into_iter()
        .map(|found| found.account_id)
        .collect();
    accounts.sort();
    accounts
}

// ---------------------------------------------------------------------------
// What the report names
// ---------------------------------------------------------------------------

/// Rule 1: the one account this emulator holds is reported once for the Region asked about,
/// carrying what the store says about the discovery.
///
/// The rows differ along what that generalises over: how many Regions the discovery operates in
/// and which of them is asked about.
#[tokio::test]
async fn report_names_the_emulated_account_once_with_what_the_store_holds() {
    let router = emulator();
    let case = "two discoveries, three Regions";
    let home_only = default_discovery(case, &router, &[]).await;
    let wide = default_discovery(case, &router, &[OTHER_REGION, THIRD_REGION]).await;
    let account = engine::emulator_account_id().to_string();

    // (case, the discovery asked through, the Region asked about)
    let rows: [(&str, &str, &str); 4] = [
        (
            "the home Region of a discovery operating only there",
            &home_only,
            REGION,
        ),
        ("the home Region of a wider discovery", &wide, REGION),
        ("a second operating Region", &wide, OTHER_REGION),
        ("a third operating Region", &wide, THIRD_REGION),
    ];

    for (row, discovery, region) in rows {
        let body = ok(row, &router, ACTION, &report_params(discovery, region, Vec::new())).await;
        let found = discovered(row, &body);
        assert_eq!(
            found.len(),
            1,
            "case {row:?}: expected exactly one discovered account, got {found:?} in {body}"
        );
        let entry = &found[0];
        assert_eq!(
            entry.account_id, account,
            "case {row:?}: expected the emulated account {account}, got {entry:?}"
        );
        assert_eq!(
            entry.discovery_region.as_deref(),
            Some(region),
            "case {row:?}: expected the discovery Region {region}, got {entry:?}"
        );
        // Nothing of this emulator assumes a role, is throttled or is unauthorized, which is
        // every failure `IpamDiscoveryFailureCode` names, so no discovery of it reports one.
        assert_eq!(
            entry.failure_code, None,
            "case {row:?}: a discovery that succeeded reports no failure code, got {entry:?}"
        );
        // "The ID of an Organizational Unit in Amazon Web Services Organizations": the emulated
        // account belongs to no organization, so it sits in no organizational unit.
        assert_eq!(
            entry.organizational_unit_id, None,
            "case {row:?}: expected no organizational unit, got {entry:?}"
        );
        let attempted = entry
            .last_attempted
            .as_deref()
            .and_then(ec2_query::as_timestamp)
            .unwrap_or_else(|| {
                panic!("case {row:?}: expected an ISO 8601 last attempted time, got {entry:?}")
            });
        let successful = entry
            .last_successful
            .as_deref()
            .and_then(ec2_query::as_timestamp)
            .unwrap_or_else(|| {
                panic!("case {row:?}: expected an ISO 8601 last successful time, got {entry:?}")
            });
        // A discovery succeeds on an attempt, so the last success is never later than the last
        // attempt.
        assert!(
            successful <= attempted,
            "case {row:?}: expected the last successful time {successful} to be no later than \
             the last attempted time {attempted}, on {entry:?}"
        );
    }

    // The account the report names is the account the discovery itself is owned by: one store,
    // two reads.
    let described = ok(
        case,
        &router,
        "DescribeIpamResourceDiscoveries",
        &[p("IpamResourceDiscoveryId.1", wide.clone())],
    )
    .await;
    assert_eq!(
        ec2_query::element(&described, "ownerId").as_deref(),
        Some(account.as_str()),
        "case {case:?}: expected the discovery to be owned by {account}, got {described}"
    );
}

/// Rule 2: the report is of the Region asked about, through a discovery that operates in it.
///
/// The rows differ along what that generalises over: which Region is asked about, whether the
/// discovery operates in it, and how many Regions it operates in at all.
#[tokio::test]
async fn report_is_only_of_a_region_the_discovery_operates_in() {
    let router = emulator();
    let case = "a narrow discovery and a wide one";
    // "Operating Regions are Amazon Web Services Regions where the IPAM is allowed to manage IP
    // address CIDRs" (`Ipam$OperatingRegions`), and the default resource discovery discovers in
    // the IPAM's own operating Regions.
    let home_only = default_discovery(case, &router, &[]).await;
    let wide = default_discovery(case, &router, &[OTHER_REGION]).await;
    let account = engine::emulator_account_id().to_string();

    // (case, the discovery asked through, the Region asked about, the accounts expected)
    let rows: [(&str, &str, &str, Vec<String>); 5] = [
        (
            "the home Region of a discovery operating only there",
            &home_only,
            REGION,
            vec![account.clone()],
        ),
        (
            "a Region that discovery does not operate in",
            &home_only,
            OTHER_REGION,
            Vec::new(),
        ),
        (
            "the home Region of a discovery operating in both",
            &wide,
            REGION,
            vec![account.clone()],
        ),
        (
            "the other Region of a discovery operating in both",
            &wide,
            OTHER_REGION,
            vec![account.clone()],
        ),
        (
            "a Region no discovery of the account operates in",
            &wide,
            UNMONITORED_REGION,
            Vec::new(),
        ),
    ];

    for (row, discovery, region, expected) in rows {
        let body = ok(row, &router, ACTION, &report_params(discovery, region, Vec::new())).await;
        assert_eq!(
            reported_accounts(row, &body),
            expected,
            "case {row:?}: expected the accounts {expected:?} in {body}"
        );
        // A page nothing follows carries no token, whether or not it reported anything.
        let token = ec2_query::member(&body, "nextToken");
        assert_eq!(
            token, None,
            "case {row:?}: expected no nextToken on the only page, got {token:?} in {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Narrowing the report and paging it
// ---------------------------------------------------------------------------

/// Rule 3, first half: a filter selects the accounts whose member it names matches, and a name
/// this operation does not resolve is refused.
///
/// The rows differ along what that generalises over: which member the filter names, whether the
/// value is exact, carries EC2's `*` wildcard or is one of several, and whether it selects the
/// account or nothing.
#[tokio::test]
async fn filters_narrow_the_report_and_an_unresolvable_filter_name_is_refused() {
    let router = emulator();
    let case = "filters over the emulated account";
    let discovery = default_discovery(case, &router, &[OTHER_REGION]).await;
    let account = engine::emulator_account_id().to_string();
    let prefix = format!("{}*", &account[..4]);

    // (case, filter name, filter values, the accounts expected)
    let rows: [(&str, &str, Vec<String>, Vec<String>); 9] = [
        (
            "the account by its own id",
            "account-id",
            vec![account.clone()],
            vec![account.clone()],
        ),
        (
            "the account by a wildcard over its id",
            "account-id",
            vec![prefix.clone()],
            vec![account.clone()],
        ),
        (
            "an account id this emulator does not hold",
            "account-id",
            vec![OTHER_ACCOUNT.to_string()],
            Vec::new(),
        ),
        (
            "the Region the report is of",
            "discovery-region",
            vec![REGION.to_string()],
            vec![account.clone()],
        ),
        (
            "another Region of the same discovery",
            "discovery-region",
            vec![OTHER_REGION.to_string()],
            Vec::new(),
        ),
        (
            "several Regions, one of them the report's",
            "discovery-region",
            vec![OTHER_REGION.to_string(), REGION.to_string()],
            vec![account.clone()],
        ),
        (
            "a discovery failure nothing reported",
            "failure-reason-code",
            vec![ASSUME_ROLE_FAILURE.to_string()],
            Vec::new(),
        ),
        (
            "an organizational unit the account does not sit in",
            "organizational-unit-id",
            vec!["ou-1234-12345678".to_string()],
            Vec::new(),
        ),
        (
            "a wildcard over the last successful discovery time",
            "last-successful-discovery-time",
            vec!["*T*".to_string()],
            vec![account.clone()],
        ),
    ];

    for (row, name, values, expected) in rows {
        let mut extra = vec![p("Filter.1.Name", name)];
        for (index, value) in values.iter().enumerate() {
            extra.push(p(&format!("Filter.1.Value.{}", index + 1), value.clone()));
        }
        let body = ok(row, &router, ACTION, &report_params(&discovery, REGION, extra)).await;
        assert_eq!(
            reported_accounts(row, &body),
            expected,
            "case {row:?}: expected the accounts {expected:?} in {body}"
        );
    }

    // Two filters are joined with an AND, so one that holds and one that cannot select nothing
    // together.
    let row = "an AND of two filters";
    let both = ok(
        row,
        &router,
        ACTION,
        &report_params(
            &discovery,
            REGION,
            vec![
                p("Filter.1.Name", "discovery-region"),
                p("Filter.1.Value.1", REGION),
                p("Filter.2.Name", "account-id"),
                p("Filter.2.Value.1", OTHER_ACCOUNT),
            ],
        ),
    )
    .await;
    assert!(
        reported_accounts(row, &both).is_empty(),
        "case {row:?}: the AND of two filters selects nothing, got {both}"
    );

    // A filter name the operation does not resolve is refused — including the `tag:` set an EC2
    // describe of a TAGGABLE resource accepts, which a discovered account has no member for.
    for (row, name) in [
        ("a filter name nothing answers", "discovered-account"),
        ("a tag filter on a resource carrying no tags", "tag-key"),
        ("a tag filter naming a key", "tag:Name"),
    ] {
        let (status, body) = api(
            &router,
            REGION,
            ACTION,
            &report_params(
                &discovery,
                REGION,
                vec![p("Filter.1.Name", name), p("Filter.1.Value.1", "anything")],
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400 for the filter {name:?}, got {status}: {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {row:?}: expected {INVALID_PARAMETER_VALUE} for the filter {name:?}, got {body}"
        );
        assert!(
            ec2_query::error_message(&body).contains(name),
            "case {row:?}: expected the refusal to name the filter {name:?}, got {}",
            ec2_query::error_message(&body)
        );
    }
}

/// Rule 3, second half: `MaxResults` pages the report, and a page nothing follows carries no
/// token, so a walk over the report ends.
///
/// The rows differ along what that generalises over: the page size asked for, at each published
/// bound of the modelled range and between them.
#[tokio::test]
async fn max_results_pages_the_report_and_the_walk_ends() {
    let router = emulator();
    let case = "paging the report";
    let discovery = default_discovery(case, &router, &[]).await;
    let account = engine::emulator_account_id().to_string();

    // The model's own range for the member is 5..1000
    // (`smithy_query(query="operation",operation="GetIpamDiscoveredAccounts")`), so 5 is the
    // smallest page a request may ask for and 1000 the largest.
    for size in [5_usize, 6, 999, 1000] {
        let row = format!("MaxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut extra = vec![p("MaxResults", size.to_string())];
            if let Some(next) = token.clone() {
                extra.push(p("NextToken", next));
            }
            let body = ok(&row, &router, ACTION, &report_params(&discovery, REGION, extra)).await;
            let page = reported_accounts(&row, &body);
            assert!(
                page.len() <= size,
                "case {row:?}: expected at most {size} accounts in a page, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= 2,
                "case {row:?}: the walk must end, got {pages} pages for {walked:?}"
            );
            token = ec2_query::member(&body, "nextToken").filter(|value| !value.is_empty());
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked,
            vec![account.clone()],
            "case {row:?}: expected the whole report once, got {walked:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What the operation refuses
// ---------------------------------------------------------------------------

/// One row of the refusal table below: the request member at fault and the refusal it is owed.
struct Refusal<'ids> {
    /// The row's case label, named by every assertion about it.
    case: &'static str,
    /// The `IpamResourceDiscoveryId` this row sends.
    discovery_id: &'ids str,
    /// The `DiscoveryRegion` this row sends.
    region: &'static str,
    /// Whatever this row sends beside those two.
    extra: Vec<Param>,
    /// The HTTP status the refusal carries.
    status: u16,
    /// The EC2 error code the refusal carries.
    code: &'static str,
}

/// Rule 4: a request whose discovery id, Region, page size or `NextToken` the service cannot
/// resolve is refused with EC2's own code, and a `DryRun` of a request that would have
/// succeeded reports that and reads nothing.
///
/// The rows differ along what that generalises over: which member is at fault and in what way —
/// an id of the wrong form, an id of the right form naming nothing, an id of a NEIGHBOURING
/// resource of the same prefix, a Region naming nothing at all, a page size one past each
/// published bound, and a token this service never issued.
#[tokio::test]
async fn an_unresolvable_request_is_refused_and_a_dry_run_reports_it() {
    let router = emulator();
    let case = "refusals";
    let discovery = default_discovery(case, &router, &[]).await;

    let rows: [Refusal<'_>; 8] = [
        Refusal {
            case: "not an id of any resource",
            discovery_id: "not-an-id",
            region: REGION,
            extra: Vec::new(),
            status: 400,
            code: INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        },
        Refusal {
            case: "an id of the neighbouring association resource",
            discovery_id: "ipam-res-disco-assoc-0123456789abcdef0",
            region: REGION,
            extra: Vec::new(),
            status: 400,
            code: INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        },
        Refusal {
            case: "a well-formed long id naming no discovery",
            discovery_id: "ipam-res-disco-0123456789abcdef0",
            region: REGION,
            extra: Vec::new(),
            status: 400,
            code: INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        },
        Refusal {
            case: "a well-formed short id naming no discovery",
            discovery_id: "ipam-res-disco-01234567",
            region: REGION,
            extra: Vec::new(),
            status: 400,
            code: INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        },
        Refusal {
            case: "a Region naming nothing at all",
            discovery_id: discovery.as_str(),
            region: "",
            extra: Vec::new(),
            status: 400,
            code: MISSING_PARAMETER,
        },
        Refusal {
            case: "a page size one below the modelled minimum",
            discovery_id: discovery.as_str(),
            region: REGION,
            extra: vec![p("MaxResults", "4")],
            status: 400,
            code: INVALID_MAX_RESULTS,
        },
        Refusal {
            case: "a page size one above the modelled maximum",
            discovery_id: discovery.as_str(),
            region: REGION,
            extra: vec![p("MaxResults", "1001")],
            status: 400,
            // Above the modelled `IpamMaxResults` range the boundary validator refuses the
            // member itself, before this operation's handler runs; only the MINIMUM carries
            // EC2's own `InvalidMaxResults` wording (`crate::service`'s refusal override).
            code: "ValidationError",
        },
        Refusal {
            case: "a token this service did not issue",
            discovery_id: discovery.as_str(),
            region: REGION,
            extra: vec![p("NextToken", "not-a-token")],
            status: 400,
            code: INVALID_PAGINATION_TOKEN,
        },
    ];

    for row in rows {
        let Refusal {
            case: row_case,
            discovery_id,
            region,
            extra,
            status: expected_status,
            code: expected_code,
        } = row;
        let (status, body) = api(
            &router,
            REGION,
            ACTION,
            &report_params(discovery_id, region, extra),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {row_case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            ec2_query::error_code(&body),
            expected_code,
            "case {row_case:?}: expected {expected_code}, got {body}"
        );
    }

    // "If you have the required permissions, the error response is DryRunOperation": the request
    // would have succeeded, so that is what it is told.
    let (status, body) = api(
        &router,
        REGION,
        ACTION,
        &report_params(&discovery, REGION, vec![p("DryRun", "true")]),
    )
    .await;
    assert_eq!(
        status, 412,
        "case {case:?}: expected HTTP 412 from a dry run, got {status} with body {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        DRY_RUN_OPERATION,
        "case {case:?}: expected {DRY_RUN_OPERATION} from a dry run, got {body}"
    );
}
