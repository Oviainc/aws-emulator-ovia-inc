// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeIpamResourceDiscoveries`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * a describe REPORTS the resource discoveries the store holds, every member as the write
//!   that minted them stored it — which for a default discovery is `CreateIpam`: "the default
//!   resource discovery is the resource discovery automatically created when you create an
//!   IPAM" (`IpamResourceDiscovery$IsDefault`) — narrowed to the ids the request names and to
//!   every filter it carries (joined with AND, each filter's own values with OR);
//! * a filter name the operation does not resolve is refused rather than read as "no
//!   results", an id that is not an IPAM resource discovery id at all is refused with its
//!   `Malformed` code and a well-formed one the account holds no discovery for with its
//!   `NotFound` code, and a `DryRun` reports the permission outcome without reading;
//! * the report is paged in a stable `IpamResourceDiscoveryId` order: at most `MaxResults`
//!   items with a `NextToken` naming where the next page starts, and no token once "there are
//!   no more results to return" (`DescribeIpamResourceDiscoveriesResult$NextToken`);
//! * the read is a REPORT: it applies only the state move the clock already owed the
//!   discovery (`create-in-progress` -> `create-complete`) and changes nothing else a later
//!   caller sees.
//!
//! Each table row is a distinct input along what those generalise over: how many operating
//! Regions the discovery covers, whether it carries a description, which KIND of member a
//! filter compares (a string, a boolean, a member of a nested list, a tag), how many values
//! that filter names, how many ids the request names, how big a page it asks for, and which
//! member of a refused request is at fault.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::state::{
    INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED, INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
    INVALID_PAGINATION_TOKEN, INVALID_PARAMETER_VALUE, IPAM_RESOURCE_DISCOVERY_ID_PREFIX,
    IPAM_STATE_CREATE_COMPLETE, IPAM_STATE_CREATE_IN_PROGRESS,
    RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region and
/// the Region its default resource discovery reports.
const REGION: &str = "us-east-1";

/// One row of the create table: the case, the `CreateIpam` form fields, the description the
/// discovery it comes up with is expected to carry, and the operating Regions it is expected
/// to cover.
type CreateCase<'row> = (&'row str, Vec<Param>, Option<&'row str>, Vec<&'row str>);

/// One row of the joined-filter table: the case, the filter entries the request carries, and
/// the ids they select together.
type JoinedFilterCase<'row> = (&'row str, Vec<(&'row str, Vec<&'row str>)>, Vec<&'row str>);

/// The action under test.
const ACTION: &str = "DescribeIpamResourceDiscoveries";

/// Longer than the span the clock owes a `create-in-progress` discovery, so a read after this
/// much has certainly passed the due instant.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One call of `action` signed for the Region above.
async fn invoke(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One call of `action` requiring success, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = invoke(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One `DescribeIpamResourceDiscoveries` requiring success, and the body it answered.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    ok(case, router, ACTION, params).await
}

/// The form fields for an `OperatingRegions` list of `regions`.
fn operating_region_params(regions: &[&str]) -> Vec<Param> {
    regions
        .iter()
        .enumerate()
        .map(|(index, region)| param(&format!("OperatingRegion.{}.RegionName", index + 1), region))
        .collect()
}

/// Create one IPAM through the operation that owns it, and answer the id of the default
/// resource discovery it came up with — the one resource this suite reports.
async fn create_ipam_discovery(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let body = ok(case, router, "CreateIpam", params).await;
    let parsed = document(case, &body);
    let ipam = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM (no <ipamId>) in {body}"));
    child_text(ipam, "defaultResourceDiscoveryId")
        .unwrap_or_else(|| panic!("case {case:?}: no <defaultResourceDiscoveryId> in {body}"))
}

/// One resource discovery as a response reports it: every member read off the wire, with the
/// two nested lists and the tag list flattened into comparable values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedDiscovery {
    owner_id: Option<String>,
    id: Option<String>,
    arn: Option<String>,
    region: Option<String>,
    description: Option<String>,
    /// The `regionName`s of `operatingRegionSet`, sorted: the Regions are a SET, so the order
    /// they are reported in is not part of the rule, but how many times each appears is.
    operating_regions: Vec<String>,
    is_default: Option<String>,
    state: Option<String>,
    tags: Vec<(String, String)>,
    organizational_unit_exclusions: Vec<String>,
}

/// Every resource discovery the envelope's `ipamResourceDiscoverySet` carries, in the order
/// the response reported them.
fn reported(case: &str, body: &str) -> Vec<ReportedDiscovery> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "ipamResourceDiscoverySet")
        .into_iter()
        .map(|item| {
            let mut operating_regions: Vec<String> = set_items(item, "operatingRegionSet")
                .into_iter()
                .filter_map(|region| child_text(region, "regionName"))
                .collect();
            operating_regions.sort();
            ReportedDiscovery {
                owner_id: child_text(item, "ownerId"),
                id: child_text(item, "ipamResourceDiscoveryId"),
                arn: child_text(item, "ipamResourceDiscoveryArn"),
                region: child_text(item, "ipamResourceDiscoveryRegion"),
                description: child_text(item, "description"),
                operating_regions,
                is_default: child_text(item, "isDefault"),
                state: child_text(item, "state"),
                tags: set_items(item, "tagSet")
                    .into_iter()
                    .map(|tag| {
                        (
                            child_text(tag, "key").unwrap_or_default(),
                            child_text(tag, "value").unwrap_or_default(),
                        )
                    })
                    .collect(),
                organizational_unit_exclusions: set_items(item, "organizationalUnitExclusionSet")
                    .into_iter()
                    .filter_map(|exclusion| child_text(exclusion, "organizationsEntityPath"))
                    .collect(),
            }
        })
        .collect()
}

/// The one discovery a response narrowed to a single id carries.
fn reported_one(case: &str, body: &str) -> ReportedDiscovery {
    let all = reported(case, body);
    assert_eq!(
        all.len(),
        1,
        "case {case:?}: expected exactly one resource discovery, got {body}"
    );
    all.into_iter().next().expect("one discovery")
}

/// The ids a response reported, as a set — what a narrowing assertion compares.
fn reported_ids(case: &str, body: &str) -> BTreeSet<String> {
    reported(case, body)
        .into_iter()
        .filter_map(|discovery| discovery.id)
        .collect()
}

// ---------------------------------------------------------------------------
// What the describe reports
// ---------------------------------------------------------------------------

/// The rule: `DescribeIpamResourceDiscoveries` reports the discoveries the store holds, every
/// member as the write that minted them stored it — the owning account, the id, the ARN
/// derived from that id, the home Region, the description and operating Regions of the IPAM
/// the default discovery belongs to, the fact that it IS that IPAM's default one, and the
/// lifecycle state the create left it in — and nothing the store does not hold.
///
/// The rows differ along what that generalises over: whether the discovery carries a
/// description, and how many operating Regions it covers (its home Region alone, one more,
/// three more).
#[tokio::test]
async fn describe_reports_every_stored_discovery_as_the_create_stored_it() {
    let (router, _engine) = emulator();
    let account = engine::emulator_account_id().to_string();

    // (case, CreateIpam fields, expected description, expected operating Regions)
    let cases: [CreateCase; 4] = [
        (
            "the default discovery of an IPAM created with nothing but the action",
            Vec::new(),
            None,
            vec![REGION],
        ),
        (
            "a described IPAM's default discovery",
            vec![param("Description", "corp address space")],
            Some("corp address space"),
            vec![REGION],
        ),
        (
            "one operating Region beyond the home Region",
            operating_region_params(&["eu-west-1"]),
            None,
            vec!["eu-west-1", REGION],
        ),
        (
            "a description and three operating Regions beyond the home Region",
            {
                let mut params = vec![param("Description", "global address space")];
                params.extend(operating_region_params(&[
                    "eu-west-1",
                    "ap-south-1",
                    "us-west-2",
                ]));
                params
            },
            Some("global address space"),
            vec!["ap-south-1", "eu-west-1", "us-west-2", REGION],
        ),
    ];

    let mut every_id: BTreeSet<String> = BTreeSet::new();
    for (case, params, description, operating_regions) in &cases {
        let discovery_id = create_ipam_discovery(case, &router, params).await;
        assert!(
            is_long_id(IPAM_RESOURCE_DISCOVERY_ID_PREFIX, &discovery_id),
            "case {case:?}: expected an id of the form \
             {IPAM_RESOURCE_DISCOVERY_ID_PREFIX}-<17 hex digits>, got {discovery_id:?}"
        );
        every_id.insert(discovery_id.clone());

        let body = describe_ok(
            case,
            &router,
            &[param("IpamResourceDiscoveryId.1", &discovery_id)],
        )
        .await;
        let mut expected_regions: Vec<String> =
            operating_regions.iter().map(|r| (*r).to_string()).collect();
        expected_regions.sort();
        let expected = ReportedDiscovery {
            owner_id: Some(account.clone()),
            id: Some(discovery_id.clone()),
            arn: Some(engine::emulator_arn(
                "ec2",
                "",
                &format!("{RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY}/{discovery_id}"),
            )),
            region: Some(REGION.to_string()),
            description: description.map(str::to_string),
            operating_regions: expected_regions,
            is_default: Some("true".to_string()),
            state: Some(IPAM_STATE_CREATE_IN_PROGRESS.to_string()),
            // "The tags of the request are the IPAM's" — nothing tagged the discovery, and no
            // organizational unit is excluded from one IPAM created for itself.
            tags: Vec::new(),
            organizational_unit_exclusions: Vec::new(),
        };
        assert_eq!(
            reported_one(case, &body),
            expected,
            "case {case:?}: expected the discovery {expected:?}, got {body}"
        );
    }

    // Every discovery the creates minted, and nothing else, with no narrowing at all.
    let body = describe_ok("the whole store", &router, &[]).await;
    assert_eq!(
        reported_ids("the whole store", &body),
        every_id,
        "the describe reports the discoveries that were created and nothing else, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Narrowing
// ---------------------------------------------------------------------------

/// The rule: a describe narrows its report to the ids the request names and to every filter it
/// carries — the filters joined with an AND, each filter's own values with an OR, each value
/// matched with EC2's wildcards against the member the filter name selects.
///
/// The rows differ along the KIND of member compared (an id, an ARN, a Region, a description,
/// a boolean, an entry of a nested list, a tag), how many values the filter names, and whether
/// the value is exact or a wildcard.
#[tokio::test]
async fn describe_narrows_by_ids_and_by_every_filter() {
    let (router, _engine) = emulator();
    let account = engine::emulator_account_id().to_string();

    let alpha = create_ipam_discovery(
        "alpha",
        &router,
        &[param("Description", "alpha address space")],
    )
    .await;
    let beta = create_ipam_discovery("beta", &router, &{
        let mut params = vec![param("Description", "beta address space")];
        params.extend(operating_region_params(&["eu-west-1"]));
        params
    })
    .await;
    let gamma = create_ipam_discovery(
        "gamma",
        &router,
        &operating_region_params(&["eu-west-1", "ap-south-1"]),
    )
    .await;
    let every: BTreeSet<String> = [alpha.clone(), beta.clone(), gamma.clone()]
        .into_iter()
        .collect();

    let arn_of = |id: &str| -> String {
        engine::emulator_arn(
            "ec2",
            "",
            &format!("{RESOURCE_TYPE_IPAM_RESOURCE_DISCOVERY}/{id}"),
        )
    };
    let gamma_arn = arn_of(&gamma);

    // The ids the request names, which narrow the report without any filter at all.
    let by_ids: [(&str, Vec<&str>, Vec<&str>); 3] = [
        ("one id", vec![&alpha], vec![&alpha]),
        ("two ids", vec![&beta, &gamma], vec![&beta, &gamma]),
        (
            "the same id twice",
            vec![&alpha, &alpha],
            vec![&alpha],
        ),
    ];
    for (case, ids, expected) in &by_ids {
        let params: Vec<Param> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| param(&format!("IpamResourceDiscoveryId.{}", index + 1), id))
            .collect();
        let body = describe_ok(case, &router, &params).await;
        let wanted: BTreeSet<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            reported_ids(case, &body),
            wanted,
            "case {case:?}: expected the ids to select {wanted:?}, got {body}"
        );
    }

    // (case, filter name, filter values, the ids it selects)
    let by_filter: Vec<(&str, &str, Vec<&str>, Vec<&str>)> = vec![
        (
            "by its own id",
            "ipam-resource-discovery-id",
            vec![&alpha],
            vec![&alpha],
        ),
        (
            "by two ids, joined with OR",
            "ipam-resource-discovery-id",
            vec![&alpha, &beta],
            vec![&alpha, &beta],
        ),
        (
            "by an id no discovery has",
            "ipam-resource-discovery-id",
            vec!["ipam-res-disco-00000000000000000"],
            Vec::new(),
        ),
        ("by ARN", "ipam-resource-discovery-arn", vec![&gamma_arn], vec![&gamma]),
        (
            "by home Region, which every discovery of this account shares",
            "ipam-resource-discovery-region",
            vec![REGION],
            vec![&alpha, &beta, &gamma],
        ),
        (
            "by a Region no discovery is in",
            "ipam-resource-discovery-region",
            vec!["eu-west-1"],
            Vec::new(),
        ),
        (
            "by an exact description",
            "description",
            vec!["alpha address space"],
            vec![&alpha],
        ),
        (
            "by a description wildcard",
            "description",
            vec!["*address space"],
            vec![&alpha, &beta],
        ),
        (
            // The IPAM gamma belongs to was created without one, so its discovery carries no
            // description at all and no value of the filter can select it.
            "by any description at all",
            "description",
            vec!["*"],
            vec![&alpha, &beta],
        ),
        ("by owner", "owner-id", vec![&account], vec![&alpha, &beta, &gamma]),
        (
            "by being the IPAM's default discovery",
            "is-default",
            vec!["true"],
            vec![&alpha, &beta, &gamma],
        ),
        (
            // Only `CreateIpamResourceDiscovery` makes a non-default one, and nothing has.
            "by not being a default discovery",
            "is-default",
            vec!["false"],
            Vec::new(),
        ),
        (
            "by an operating Region beyond the home Region",
            "operating-region",
            vec!["eu-west-1"],
            vec![&beta, &gamma],
        ),
        (
            "by one of three operating Regions",
            "operating-region",
            vec!["ap-south-1"],
            vec![&gamma],
        ),
        (
            "by two operating Regions, joined with OR",
            "operating-region",
            vec!["ap-south-1", REGION],
            vec![&alpha, &beta, &gamma],
        ),
        (
            "by a lifecycle state every discovery being created is in",
            "state",
            vec!["create-*"],
            vec![&alpha, &beta, &gamma],
        ),
        (
            "by a state no discovery is in",
            "state",
            vec!["delete-complete"],
            Vec::new(),
        ),
        (
            // The tags of a `CreateIpam` request are the IPAM's: nothing tagged the default
            // discovery, so no tag filter selects one.
            "by any tag key at all",
            "tag-key",
            vec!["*"],
            Vec::new(),
        ),
        ("by a tag value", "tag-value", vec!["shared"], Vec::new()),
        ("by a named tag", "tag:Name", vec!["shared"], Vec::new()),
        (
            // An IPAM created for its own account excludes no organizational unit.
            "by any organizational unit exclusion at all",
            "organizational-unit-exclusion",
            vec!["*"],
            Vec::new(),
        ),
    ];
    for (case, name, values, expected) in &by_filter {
        let body = describe_ok(case, &router, &filters(&[(name, values)])).await;
        let wanted: BTreeSet<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            reported_ids(case, &body),
            wanted,
            "case {case:?}: expected the filter {name:?} over {values:?} to select {wanted:?}, \
             got {body}"
        );
    }

    // (case, filter entries, the ids they select together)
    let joined: Vec<JoinedFilterCase> = vec![
        (
            "two filters that both hold",
            vec![
                ("operating-region", vec!["eu-west-1"]),
                ("is-default", vec!["true"]),
            ],
            vec![&beta, &gamma],
        ),
        (
            "two filters only one discovery satisfies",
            vec![
                ("operating-region", vec!["eu-west-1"]),
                ("description", vec!["*address space"]),
            ],
            vec![&beta],
        ),
        (
            "two filters no discovery satisfies at once",
            vec![
                ("description", vec!["alpha address space"]),
                ("operating-region", vec!["eu-west-1"]),
            ],
            Vec::new(),
        ),
        (
            "three filters that all hold",
            vec![
                ("owner-id", vec![&account]),
                ("ipam-resource-discovery-region", vec![REGION]),
                ("ipam-resource-discovery-id", vec![&gamma]),
            ],
            vec![&gamma],
        ),
    ];
    for (case, entries, expected) in &joined {
        let fields: Vec<(&str, &[&str])> = entries
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let body = describe_ok(case, &router, &filters(&fields)).await;
        let wanted: BTreeSet<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            reported_ids(case, &body),
            wanted,
            "case {case:?}: expected the filters to select {wanted:?} with an AND, got {body}"
        );
    }

    // An id narrows what the filters would otherwise have selected: the two meet with an AND.
    let body = describe_ok(
        "an id beside a filter",
        &router,
        &{
            let mut params = vec![param("IpamResourceDiscoveryId.1", &alpha)];
            params.extend(filters(&[("operating-region", &["eu-west-1"])]));
            params
        },
    )
    .await;
    assert_eq!(
        reported_ids("an id beside a filter", &body),
        BTreeSet::new(),
        "an id and a filter narrow together, got {body}"
    );

    // None of the narrowing changed what the store reports.
    let body = describe_ok("the whole store", &router, &[]).await;
    assert_eq!(
        reported_ids("the whole store", &body),
        every,
        "a narrowed describe changes nothing, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// The rule: the report is paged in a stable `IpamResourceDiscoveryId` order — at most
/// `MaxResults` items, a `NextToken` naming where the next page starts, and no token once
/// there are no more results to return — so a walk of the pages reports every discovery
/// exactly once.
///
/// The rows are page sizes: the model's smallest (`IpamMaxResults` has a minimum of 5), one
/// that divides the store unevenly, and one at the store's size.
#[tokio::test]
async fn describe_pages_in_a_stable_id_order() {
    let (router, _engine) = emulator();
    let mut created: BTreeSet<String> = BTreeSet::new();
    for nth in 0..7 {
        let case = format!("IPAM {nth}");
        created.insert(create_ipam_discovery(&case, &router, &[]).await);
    }

    for page_size in [5usize, 6, 7] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = describe_ok(&case, &router, &params).await;
            let page = elements(&body, "ipamResourceDiscoveryId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} discoveries, got {body}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 4,
                "case {case:?}: the paging walk must terminate, got {seen:?}"
            );
            match element(&body, "nextToken") {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "case {case:?}: the pages come in IpamResourceDiscoveryId order, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            created,
            "case {case:?}: paging reports every discovery exactly once, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            created.len(),
            "case {case:?}: no discovery is reported twice, got {seen:?}"
        );
    }

    // A page that holds everything left carries no token at all.
    let body = describe_ok("one page over the whole store", &router, &[]).await;
    assert_eq!(
        element(&body, "nextToken"),
        None,
        "no token once there are no more results to return, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The rule: a request this operation's contract does not admit is refused with the code that
/// names what was wrong — this resource's `Malformed` code for a value that is not an IPAM
/// resource discovery id at all, its `NotFound` code for a well-formed id the account holds no
/// discovery for, `InvalidParameterValue` for a filter name the operation does not resolve or
/// for more filtering than it will do, `InvalidPaginationToken` for a page token it did not
/// mint, and `DryRunOperation` for a permission check, which reads nothing.
#[tokio::test]
async fn describe_refuses_what_the_contract_does_not_admit() {
    let (router, _engine) = emulator();
    let discovery = create_ipam_discovery("the one IPAM the store holds", &router, &[]).await;

    let many_filters = |count: usize| -> Vec<Param> {
        (0..count)
            .flat_map(|index| {
                vec![
                    param(
                        &format!("Filter.{}.Name", index + 1),
                        "ipam-resource-discovery-id",
                    ),
                    param(&format!("Filter.{}.Value.1", index + 1), &discovery),
                ]
            })
            .collect()
    };
    let many_values = |count: usize| -> Vec<Param> {
        let mut params = vec![param("Filter.1.Name", "ipam-resource-discovery-id")];
        // Synthesised ids for every position but the last, which carries the id the store
        // really minted — so a list AT the cap exercises the cap and still selects the
        // discovery.
        params.extend((0..count.saturating_sub(1)).map(|index| {
            param(
                &format!("Filter.1.Value.{}", index + 1),
                &format!("{IPAM_RESOURCE_DISCOVERY_ID_PREFIX}-{:017x}", index + 1),
            )
        }));
        params.push(param(&format!("Filter.1.Value.{count}"), &discovery));
        params
    };
    let many_ids = |count: usize| -> Vec<Param> {
        (0..count)
            .map(|index| param(&format!("IpamResourceDiscoveryId.{}", index + 1), &discovery))
            .collect()
    };

    // (case, params, expected status, expected code). A refused request is a 400 except for
    // the permission check, which EC2 answers 412 `DryRunOperation`.
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "an id that is not an IPAM resource discovery id at all",
            vec![param("IpamResourceDiscoveryId.1", "not-a-discovery-id")],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        ),
        (
            // An ASSOCIATION id is an id of another resource, however much of the prefix it
            // shares.
            "an id of another resource of the same family",
            vec![param(
                "IpamResourceDiscoveryId.1",
                "ipam-res-disco-assoc-0123456789abcdef0",
            )],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        ),
        (
            "a malformed id beside a well-formed one",
            vec![
                param("IpamResourceDiscoveryId.1", &discovery),
                param("IpamResourceDiscoveryId.2", "ipam-res-disco-not-hexadecimal"),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        ),
        (
            "a well-formed id the account holds no discovery for",
            vec![param(
                "IpamResourceDiscoveryId.1",
                "ipam-res-disco-00000000000000000",
            )],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        ),
        (
            "an unknown id beside a known one",
            vec![
                param("IpamResourceDiscoveryId.1", &discovery),
                param(
                    "IpamResourceDiscoveryId.2",
                    "ipam-res-disco-00000000000000000",
                ),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_NOT_FOUND,
        ),
        (
            "a filter name the operation does not resolve",
            vec![
                param("Filter.1.Name", "discovery-colour"),
                param("Filter.1.Value.1", "blue"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            // "Filter names are case-sensitive" (`Filter$Name`).
            "a filter name spelled in another case",
            vec![
                param("Filter.1.Name", "Is-Default"),
                param("Filter.1.Value.1", "true"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a tag filter naming no key",
            vec![
                param("Filter.1.Name", "tag:"),
                param("Filter.1.Value.1", "anything"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a page token this service did not mint",
            vec![param("NextToken", "not-a-token-this-service-minted")],
            400,
            INVALID_PAGINATION_TOKEN,
        ),
        (
            "one filter past the cap",
            many_filters(ec2::state::MAX_FILTERS + 1),
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "one filter value past the cap",
            many_values(ec2::state::MAX_FILTER_VALUES + 1),
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a filter value one byte past the cap",
            vec![
                param("Filter.1.Name", "description"),
                param(
                    "Filter.1.Value.1",
                    &"d".repeat(ec2::state::MAX_FILTER_VALUE_BYTES + 1),
                ),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "one id past the cap",
            many_ids(ec2::state::MAX_RESOURCE_IDS + 1),
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a permission check",
            vec![param("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        (
            // The form of the ids is a fact about the REQUEST, so it is answered before the
            // permission check rather than after it.
            "a permission check beside a malformed id",
            vec![
                param("DryRun", "true"),
                param("IpamResourceDiscoveryId.1", "not-a-discovery-id"),
            ],
            400,
            INVALID_IPAM_RESOURCE_DISCOVERY_ID_MALFORMED,
        ),
        (
            // …and so is a filter name, so a dry run that would have been refused for one is
            // refused with that reason instead of reporting that it would have succeeded.
            "a permission check beside an unresolvable filter",
            vec![
                param("DryRun", "true"),
                param("Filter.1.Name", "discovery-colour"),
                param("Filter.1.Value.1", "blue"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
    ];
    for (case, params, status, code) in &cases {
        let (answered, body) = invoke(&router, ACTION, params).await;
        assert_eq!(
            answered, *status,
            "case {case:?}: expected HTTP {status}, got {answered} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected the code {code}, got {body}"
        );
    }

    // A request AT each cap is served rather than refused, so the caps are not refusing
    // requests they should serve.
    let at_the_caps: [(&str, Vec<Param>); 3] = [
        ("filters at the cap", many_filters(ec2::state::MAX_FILTERS)),
        (
            "filter values at the cap",
            many_values(ec2::state::MAX_FILTER_VALUES),
        ),
        ("ids at the cap", many_ids(ec2::state::MAX_RESOURCE_IDS)),
    ];
    for (case, params) in &at_the_caps {
        let body = describe_ok(case, &router, params).await;
        assert_eq!(
            reported_ids(case, &body),
            [discovery.clone()].into_iter().collect::<BTreeSet<_>>(),
            "case {case:?}: a request at the cap still selects the discovery, got {body}"
        );
    }

    // None of the refusals changed what the store reports.
    let body = describe_ok("the whole store", &router, &[]).await;
    assert_eq!(
        reported_ids("the whole store", &body),
        [discovery].into_iter().collect::<BTreeSet<_>>(),
        "a refused describe changes nothing, got {body}"
    );
}

// ---------------------------------------------------------------------------
// The read is a report
// ---------------------------------------------------------------------------

/// The rule: a describe is a REPORT — it applies the state move the clock already owed the
/// discovery and nothing else, so a caller reading straight back still sees the transient
/// state, reading twice does not move it, and one reading after the due instant sees the
/// settled one.
#[tokio::test]
async fn describe_reports_the_state_the_clock_owes() {
    let case = "the clock completes the discovery";
    let (router, _engine) = emulator();
    let discovery = create_ipam_discovery(case, &router, &[]).await;
    let narrowed = [param("IpamResourceDiscoveryId.1", &discovery)];

    // Straight back, before the due instant: still being created. Reading it twice is still
    // the transient state — a read does not move it.
    for attempt in 0..2 {
        let body = describe_ok(case, &router, &narrowed).await;
        assert_eq!(
            reported_one(case, &body).state.as_deref(),
            Some(IPAM_STATE_CREATE_IN_PROGRESS),
            "case {case:?}: read {attempt} before the due instant sees the transient state, \
             got {body}"
        );
    }

    tokio::time::sleep(past_the_transition()).await;
    let settled = describe_ok(case, &router, &narrowed).await;
    let after = reported_one(case, &settled);
    assert_eq!(
        after.state.as_deref(),
        Some(IPAM_STATE_CREATE_COMPLETE),
        "case {case:?}: a read past the due instant sees the settled state, got {settled}"
    );

    // Nothing else about the discovery moved with the state, and a further read reports it
    // identically.
    let again = describe_ok(case, &router, &narrowed).await;
    assert_eq!(
        reported_one(case, &again),
        after,
        "case {case:?}: a second read past the due instant reports the same discovery, \
         got {again}"
    );
}
