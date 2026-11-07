// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeVpcs`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeVpcs`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! ec2Query is selected by the `ec2` signing name in the request's SigV4 credential
//! scope, so every call here carries one.
//!
//! The rule under test is that the operation REPORTS THE STORE: the set it answers with
//! is the account's VPCs IN THE REGION THE REQUEST WAS SIGNED FOR, narrowed by the `VpcIds`
//! and `Filters` the request names and cut into `MaxResults`-sized pages. Every table row is
//! a distinct input along what that generalises over — which filter, how many values, how
//! many filters, which ids, which page size — so the fresh-account default VPC is one row
//! among many rather than the specification.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::ops::types::{Tag, VpcCidrBlockState, VpcIpv6CidrBlockAssociation};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. `DescribeVpcs` reports the account's VPCs of
/// the Region the request reached, so the Region both selects the ec2Query envelope and
/// decides which of the seeded VPCs are reported at all.
const REGION: &str = "us-east-1";

/// The IPv6 CIDR block associated with the one seeded VPC that has one.
const SEEDED_IPV6_CIDR_BLOCK: &str = "2600:1f14:abc:1200::/56";

/// The IPv6 address pool that block came from.
const SEEDED_IPV6_POOL: &str = "Amazon";

/// One `DescribeVpcs` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeVpcs", params).await
}

/// The VPC ids a successful response reports, in the order it reported them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "vpcId")
}

/// One `MaxResults` parameter.
fn max_results_param(max_results: i64) -> Vec<(String, String)> {
    vec![("MaxResults".to_string(), max_results.to_string())]
}

/// A tag, as the seeded VPCs carry them.
fn tag(key: &str, value: &str) -> Tag {
    Tag {
        key: Some(key.to_string()),
        value: Some(value.to_string()),
    }
}

/// What one seeded VPC is created with.
struct VpcSpec {
    cidr_block: &'static str,
    instance_tenancy: &'static str,
    state: &'static str,
    /// The Region the VPC is created in, which decides the default DHCP option set it
    /// inherits — the `dhcp-options-id` filter's subject.
    region: &'static str,
    tags: Vec<Tag>,
}

/// The Region all but one seeded VPC is created in: the one every call here is signed for.
const SEEDED_REGION: &str = REGION;

/// The Region the one seeded VPC that is not in [`SEEDED_REGION`] is created in, so the
/// store holds two default DHCP option sets — and a VPC no call here may report, EC2 being
/// a regional service.
const OTHER_REGION: &str = "eu-west-1";

/// The index, in store order, of the one seeded VPC created in [`OTHER_REGION`]: the
/// default VPC is index 0 and each spec below follows in order, so the fifth spec is index
/// 5. Every table expectation is written over [`in_region`], which leaves it out.
const OTHER_REGION_INDEX: usize = 5;

/// The indices, in store order, of the VPCs a call signed for [`SEEDED_REGION`] reports:
/// every seeded VPC but the one created in [`OTHER_REGION`].
fn in_region(seeded: &Seeded) -> Vec<usize> {
    (0..seeded.ids.len())
        .filter(|index| *index != OTHER_REGION_INDEX)
        .collect()
}

/// The VPCs the filter and paging tables are written against, beyond the default VPC a
/// fresh account already has. They differ along every attribute the documented filters
/// select on: CIDR block, tenancy, lifecycle state and tags.
fn seeded_specs() -> Vec<VpcSpec> {
    vec![
        VpcSpec {
            cidr_block: "10.0.0.0/16",
            instance_tenancy: "default",
            state: "available",
            region: SEEDED_REGION,
            tags: vec![tag("Name", "web"), tag("Owner", "TeamA")],
        },
        VpcSpec {
            cidr_block: "10.1.0.0/16",
            instance_tenancy: "dedicated",
            state: "available",
            region: SEEDED_REGION,
            tags: vec![tag("Name", "batch"), tag("Owner", "TeamB")],
        },
        VpcSpec {
            cidr_block: "10.2.0.0/16",
            instance_tenancy: "default",
            state: "pending",
            region: SEEDED_REGION,
            tags: vec![tag("Name", "web")],
        },
        VpcSpec {
            cidr_block: "192.168.0.0/24",
            instance_tenancy: "host",
            state: "available",
            region: SEEDED_REGION,
            tags: Vec::new(),
        },
        // The one VPC of another Region, so it inherits that Region's default DHCP option
        // set rather than the one every row above carries.
        VpcSpec {
            cidr_block: "172.20.0.0/16",
            instance_tenancy: "default",
            state: "available",
            region: OTHER_REGION,
            tags: vec![tag("Owner", "TeamA")],
        },
        VpcSpec {
            cidr_block: "10.3.0.0/16",
            instance_tenancy: "default",
            state: "available",
            region: SEEDED_REGION,
            tags: vec![tag("Project", "atlas")],
        },
        // A seventh VPC, so the Region the calls are signed for holds SIX non-default VPCs
        // and a page of five cuts that set in two.
        VpcSpec {
            cidr_block: "192.168.1.0/24",
            instance_tenancy: "default",
            state: "available",
            region: SEEDED_REGION,
            tags: Vec::new(),
        },
    ]
}

/// What a seeded store holds, for the tables to name rows by.
struct Seeded {
    /// Every VPC id, in the order the store holds them (its key order).
    ids: Vec<String>,
    /// The `DhcpOptionsId` of each VPC, in the same order.
    dhcp_options_ids: Vec<String>,
    /// The IPv4 CIDR association id of each VPC, in the same order.
    association_ids: Vec<String>,
    /// The association id of the one IPv6 CIDR block association seeded.
    ipv6_association_id: String,
}

/// Write the seeded VPCs into the engine's store, the way a creating operation would, and
/// report the ids they were given. The one VPC with an IPv6 CIDR block association gets
/// it here, so the IPv6 filters have something to select on.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    for spec in seeded_specs() {
        guard.insert_vpc(
            spec.cidr_block,
            false,
            spec.instance_tenancy,
            spec.state,
            spec.region,
            spec.tags,
        );
    }

    let ipv6_vpc_id = guard
        .vpcs
        .values()
        .find(|vpc| vpc.cidr_block.as_deref() == Some("192.168.0.0/24"))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("the seeded 192.168.0.0/24 VPC is in the store");
    let ipv6_association_id = guard.next_id("vpc-cidr-assoc");
    if let Some(vpc) = guard.vpcs.get_mut(&ipv6_vpc_id) {
        vpc.ipv6_cidr_block_association_set = Some(vec![VpcIpv6CidrBlockAssociation {
            association_id: Some(ipv6_association_id.clone()),
            ipv6_cidr_block: Some(SEEDED_IPV6_CIDR_BLOCK.to_string()),
            ipv6_cidr_block_state: Some(VpcCidrBlockState {
                state: Some("associated".to_string()),
                status_message: None,
            }),
            network_border_group: None,
            ipv6_pool: Some(SEEDED_IPV6_POOL.to_string()),
            ipv6_address_attribute: None,
            ip_source: None,
        }]);
    }

    Seeded {
        ids: guard.vpcs.keys().cloned().collect(),
        dhcp_options_ids: guard
            .vpcs
            .values()
            .map(|vpc| vpc.dhcp_options_id.clone().unwrap_or_default())
            .collect(),
        association_ids: guard
            .vpcs
            .values()
            .map(|vpc| {
                vpc.cidr_block_association_set
                    .iter()
                    .flatten()
                    .next()
                    .and_then(|association| association.association_id.clone())
                    .unwrap_or_default()
            })
            .collect(),
        ipv6_association_id,
    }
}

/// The ids at `indices` of a seeded store, in store order.
fn ids_at(seeded: &Seeded, indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|index| seeded.ids[*index].clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account already has its default VPC — AWS creates one per Region — and
/// describing it is a READ: two calls report the same VPC and the store is unchanged.
#[tokio::test]
async fn a_fresh_account_reports_its_default_vpc_and_describing_it_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let (status, first) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "fresh account: expected HTTP 200, got {status} with body {first}"
    );
    let first_ids = elements(&first, "vpcId");
    assert_eq!(
        first_ids.len(),
        1,
        "fresh account: expected exactly the default VPC, got {first_ids:?} from {first}"
    );
    // The members a default VPC carries: AWS's documented default CIDR block, the
    // `isDefault` flag, a VPC it has finished creating, and this account as its owner.
    let expected_members = [
        ("cidrBlock", "172.31.0.0/16"),
        ("isDefault", "true"),
        ("state", "available"),
        ("ownerId", engine::emulator_account_id()),
        ("instanceTenancy", "default"),
    ];
    for (member, value) in expected_members {
        assert!(
            first.contains(&format!("<{member}>{value}</{member}>")),
            "fresh account: expected <{member}>{value}</{member}>, got {first}"
        );
    }

    let second_ids = described_ids(&router, "second read", &[]).await;
    assert_eq!(
        second_ids, first_ids,
        "a read must not change what a later caller sees: {first_ids:?} then {second_ids:?}"
    );
    assert_eq!(
        engine.state::<Ec2State>().lock().vpcs.len(),
        1,
        "describing VPCs must not write to the store"
    );
}

/// Every VPC the store holds is reported, in store order, and a VPC written after a call
/// shows up in the next one — the answer is computed from the store, not fixed.
#[tokio::test]
async fn every_stored_vpc_is_reported_and_a_later_write_is_visible() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let reported = described_ids(&router, "all VPCs", &[]).await;
    assert_eq!(
        reported,
        ids_at(&seeded, &in_region(&seeded)),
        "all VPCs: expected every stored VPC of the signed-for Region in store order, got \
         {reported:?}"
    );

    let late_id = engine.state::<Ec2State>().lock().insert_vpc(
        "10.9.0.0/16",
        false,
        "default",
        "available",
        SEEDED_REGION,
        vec![tag("Name", "late")],
    );
    let after = described_ids(&router, "after a later write", &[]).await;
    let mut expected = ids_at(&seeded, &in_region(&seeded));
    expected.push(late_id.clone());
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each documented filter selects on its own attribute, values within one filter are
/// OR'd, and the comparison is exact and case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_vpcs_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let account = engine::emulator_account_id().to_string();
    let all: Vec<usize> = in_region(&seeded);

    // One row per filter name `DescribeVpcsRequest$Filters` documents (plus the
    // variable-key `tag:<key>` form), each asserted on a VPC the issue never cited.
    let cases: Vec<(&str, Vec<String>, Vec<usize>)> = vec![
        ("cidr", vec!["10.0.0.0/16".into()], vec![1]),
        // Two values for one filter: joined with OR.
        (
            "cidr",
            vec!["10.0.0.0/16".into(), "10.1.0.0/16".into()],
            vec![1, 2],
        ),
        // A CIDR block no VPC has: an empty set, not an error.
        ("cidr", vec!["10.0.0.0/24".into()], vec![]),
        (
            "cidr-block-association.cidr-block",
            vec!["192.168.0.0/24".into()],
            vec![4],
        ),
        (
            "cidr-block-association.association-id",
            vec![seeded.association_ids[1].clone()],
            vec![1],
        ),
        (
            "cidr-block-association.state",
            vec!["associated".into()],
            all.clone(),
        ),
        // The Region's default DHCP option set is what a VPC inherits, so this selects
        // every VPC the signed-for Region reports.
        (
            "dhcp-options-id",
            vec![seeded.dhcp_options_ids[0].clone()],
            all.clone(),
        ),
        // The other Region's set selects nothing here: the one VPC that inherited it lives
        // in that Region, and this call cannot see it.
        (
            "dhcp-options-id",
            vec![seeded.dhcp_options_ids[OTHER_REGION_INDEX].clone()],
            vec![],
        ),
        (
            "ipv6-cidr-block-association.ipv6-cidr-block",
            vec![SEEDED_IPV6_CIDR_BLOCK.into()],
            vec![4],
        ),
        (
            "ipv6-cidr-block-association.ipv6-pool",
            vec![SEEDED_IPV6_POOL.into()],
            vec![4],
        ),
        (
            "ipv6-cidr-block-association.association-id",
            vec![seeded.ipv6_association_id.clone()],
            vec![4],
        ),
        (
            "ipv6-cidr-block-association.state",
            vec!["associated".into()],
            vec![4],
        ),
        ("is-default", vec!["true".into()], vec![0]),
        ("is-default", vec!["false".into()], vec![1, 2, 3, 4, 6, 7]),
        ("owner-id", vec![account], all.clone()),
        ("owner-id", vec!["111122223333".into()], vec![]),
        ("state", vec!["pending".into()], vec![3]),
        (
            "state",
            vec!["available".into(), "pending".into()],
            all.clone(),
        ),
        ("tag-key", vec!["Owner".into()], vec![1, 2]),
        ("tag-key", vec!["Project".into()], vec![6]),
        ("tag:Owner", vec!["TeamA".into()], vec![1]),
        ("tag:Name", vec!["web".into()], vec![1, 3]),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("tag:Owner", vec!["teama".into()], vec![]),
        ("vpc-id", vec![seeded.ids[6].clone()], vec![6]),
        (
            "vpc-id",
            vec![seeded.ids[0].clone(), seeded.ids[3].clone()],
            vec![0, 3],
        ),
        // Filter values carry EC2's wildcards: `*` for zero or more characters, `?` for
        // exactly one ("List and filter using the CLI and API", the guide the modelled
        // `Filter` shape links to).
        ("cidr", vec!["10.*".into()], vec![1, 2, 3, 6]),
        ("cidr", vec!["10.?.0.0/16".into()], vec![1, 2, 3, 6]),
        ("cidr", vec!["10.??.0.0/16".into()], vec![]),
        ("tag:Name", vec!["we*".into()], vec![1, 3]),
        ("tag:Name", vec!["*atch".into()], vec![2]),
        ("tag-key", vec!["*".into()], vec![1, 2, 3, 6]),
        // A documented filter carrying no value at all: there is no value for the
        // attribute to match, so it selects nothing rather than everything.
        ("state", vec![], vec![]),
        ("vpc-id", vec![], vec![]),
    ];

    for (name, values, expected_indices) in cases {
        let case = format!("{name}={values:?}");
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let reported = described_ids(&router, &case, &filters(&[(name, &borrowed)])).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the indices of the VPCs that must
/// come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Several filters are joined with an AND: only the VPCs matching every one are reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![("is-default", &["false"][..]), ("tag:Name", &["web"][..])],
            vec![1, 3],
        ),
        (
            vec![
                ("state", &["available"][..]),
                ("tag:Name", &["web"][..]),
                ("cidr", &["10.0.0.0/16"][..]),
            ],
            vec![1],
        ),
        // Each filter matches something, but no VPC matches both.
        (
            vec![("is-default", &["true"][..]), ("state", &["pending"][..])],
            vec![],
        ),
    ];

    for (entries, expected_indices) in cases {
        let case = format!("{entries:?}");
        let reported = described_ids(&router, &case, &filters(&entries)).await;
        let expected = ids_at(&seeded, &expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name the operation does not define is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a near-miss of a defined name, the underscore spelling, a defined name with
    // the wrong case, the tag filter without its key separator, the tag filter with a
    // separator but no key, and an empty name.
    let cases = [
        "cidr-block",
        "is_default",
        "Is-Default",
        "tag",
        "tag:",
        "vpc-ids",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        // The EC2 error envelope is XML, so the message's apostrophes arrive escaped.
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// VpcIds
// ---------------------------------------------------------------------------

/// `VpcIds` narrows the report to the ids named, in store order whatever order they were
/// asked for.
#[tokio::test]
async fn vpc_ids_narrow_the_report_to_the_ids_named() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows differ in count and in the order the ids are asked for.
    let cases: Vec<Vec<usize>> = vec![
        vec![0],
        vec![6],
        vec![2, 4],
        vec![4, 2],
        vec![7, 1, 3],
        in_region(&seeded),
    ];
    for asked in cases {
        let ids = ids_at(&seeded, &asked);
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("VpcIds={borrowed:?}");
        let reported = described_ids(&router, &case, &indexed("VpcId", &borrowed)).await;
        let mut expected = asked.clone();
        expected.sort_unstable();
        let expected = ids_at(&seeded, &expected);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A `VpcIds` entry that names no VPC of the account is refused with
/// `InvalidVpcID.NotFound` — even alongside ids that do exist.
#[tokio::test]
async fn a_vpc_id_that_names_no_vpc_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let known = seeded.ids[0].clone();

    // Rows: a well-formed id nothing has, the 8-hex-digit id form, a malformed id, an
    // unknown id carried beside a known one, and the id of a VPC that exists in ANOTHER
    // Region — which this call cannot see, so it names no VPC here either.
    let cases: Vec<Vec<String>> = vec![
        vec!["vpc-0ffffffffffffffff".to_string()],
        vec!["vpc-0abc1234".to_string()],
        vec!["not-a-vpc-id".to_string()],
        vec![known.clone(), "vpc-0ffffffffffffffff".to_string()],
        vec!["vpc-0ffffffffffffffff".to_string(), known.clone()],
        vec![seeded.ids[OTHER_REGION_INDEX].clone()],
        vec![known, seeded.ids[OTHER_REGION_INDEX].clone()],
    ];
    for ids in cases {
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let case = format!("VpcIds={borrowed:?}");
        let (status, body) = describe(&router, &indexed("VpcId", &borrowed)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVpcID.NotFound",
            "case {case:?}: expected InvalidVpcID.NotFound, got {body}"
        );
        assert!(
            elements(&body, "vpcId").is_empty(),
            "case {case:?}: a refused request reports no VPC, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every VPC exactly once, in store
/// order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_vpc_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `DescribeVpcsMaxResults` is modelled `range 5..1000`, so the rows are its published
    // bounds, a size that divides the store exactly, one that does not, and the size that
    // holds everything.
    for max_results in [5, 6, 7, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut params = max_results_param(max_results);
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "vpcId");
            assert!(
                page.len() <= usize::try_from(max_results).unwrap_or_default(),
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= seeded.ids.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = max_results_param(max_results);
                    params.push(("NextToken".to_string(), token.clone()));
                }
                None => break,
            }
        }
        assert_eq!(
            walked,
            ids_at(&seeded, &in_region(&seeded)),
            "case {case:?}: expected every VPC exactly once in store order, got {walked:?}"
        );
    }
}

/// A page size is a cut of the filtered set, not of the whole store: `MaxResults` and a
/// filter compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let mut params = filters(&[("is-default", &["false"])]);
    params.extend(max_results_param(5));
    let first = described_ids(&router, "filtered first page", &params).await;
    assert_eq!(
        first,
        ids_at(&seeded, &[1, 2, 3, 4, 6]),
        "filtered first page: expected the first five non-default VPCs, got {first:?}"
    );

    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "filtered first page: got {status} {body}");
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("filtered first page: expected a NextToken in {body}"));

    let mut next = filters(&[("is-default", &["false"])]);
    next.extend(max_results_param(5));
    next.push(("NextToken".to_string(), token));
    let second = described_ids(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        ids_at(&seeded, &[7]),
        "filtered second page: expected the remaining non-default VPC, got {second:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`range 5..1000`) and one past each.
    let cases = [(4, 400), (5, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let (status, body) = describe(&router, &max_results_param(max_results)).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, and that id hex-encoded (the shape
    // the token takes, without the marker and checksum a minted token carries).
    let forged_marker = ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded.ids[0]));
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.ids[0].clone(),
        ::hex::encode(&seeded.ids[0]),
        forged_marker,
    ];
    for token in cases {
        let params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = describe(
        &router,
        &[("DryRun".to_string(), "true".to_string())],
    )
    .await;
    assert_eq!(
        status, 412,
        "DryRun=true: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "DryRun=true: expected DryRunOperation, got {body}"
    );
    assert!(
        elements(&body, "vpcId").is_empty(),
        "DryRun=true: a dry run reports no VPC, got {body}"
    );

    let reported = described_ids(
        &router,
        "DryRun=false",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported,
        ids_at(&seeded, &in_region(&seeded)),
        "DryRun=false: expected the ordinary report, got {reported:?}"
    );
}
