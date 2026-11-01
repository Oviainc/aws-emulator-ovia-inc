// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeIpamPools`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=…`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! * a describe REPORTS the resources the store holds, every member as the create stored it,
//!   narrowed to the ids the request names and to every filter it carries (joined with AND,
//!   each filter's own values with OR);
//! * a filter name the operation does not resolve is refused rather than read as "no
//!   results", an id that is not one of this resource's at all is refused with its
//!   `Malformed` code and a well-formed one the account holds no pool for with its
//!   `NotFound` code, and a `DryRun` reports the permission outcome without reading;
//! * the report is paged in a stable id order: at most `MaxResults` items with a `NextToken`
//!   naming where the next page starts, and no token once "there are no more results to
//!   return";
//! * the read is a REPORT: it applies only the state move the clock already owed the pool
//!   (`create-in-progress` -> `create-complete`) and changes nothing else a later caller
//!   sees.
//!
//! Each table row is a distinct input along what those generalise over: which address family
//! the pool holds, which scope it is in, how deep in a tree of pools it sits, which optional
//! member it carries, which KIND of member a filter compares (a string, a number, a boolean,
//! a tag, a member of a nested structure), how many values that filter names, how many ids
//! the request names and how big a page it asks for.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::state::{
    ADDRESS_FAMILY_IPV4, ADDRESS_FAMILY_IPV6, INVALID_IPAM_POOL_ID_MALFORMED,
    INVALID_IPAM_POOL_ID_NOT_FOUND, INVALID_PAGINATION_TOKEN, INVALID_PARAMETER_VALUE,
    IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP, IPAM_POOL_STATE_CREATE_COMPLETE,
    IPAM_POOL_STATE_CREATE_IN_PROGRESS, TOP_LEVEL_IPAM_POOL_DEPTH, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code};
use super::wire::{child, child_text, document, param, set_items, Param};

/// The Region every call here is signed for, which is therefore each IPAM's home Region.
const REGION: &str = "us-east-1";

/// Longer than the span the clock owes a `create-in-progress` pool, so a read after this much
/// has certainly passed the due instant.
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

/// The IPAM a suite creates through `CreateIpam`: what a request names it by, the ARN every
/// pool of it reports, and the two default scopes a pool is created in.
struct CreatedIpam {
    /// The `IpamId` the create minted, which a pool reports only inside its `ipamArn`.
    id: String,
    /// "The ARN of the IPAM" (`IpamPool$IpamArn`), which every pool in either scope reports.
    arn: String,
    /// The IPAM's default public scope.
    public_scope: String,
    /// The IPAM's default private scope.
    private_scope: String,
}

/// Create one IPAM through the operation that owns it, and answer what its pools are created
/// in — "an IPAM contains two default scopes" (`IpamScope`).
async fn create_ipam(case: &str, router: &MultiServiceRouter) -> CreatedIpam {
    let body = ok(case, router, "CreateIpam", &[]).await;
    let parsed = document(case, &body);
    let ipam = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM (no <ipamId>) in {body}"));
    let read = |name: &str| -> String {
        child_text(ipam, name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the IPAM in {body}"))
    };
    CreatedIpam {
        id: read("ipamId"),
        arn: read("ipamArn"),
        public_scope: read("publicDefaultScopeId"),
        private_scope: read("privateDefaultScopeId"),
    }
}

/// One IPAM pool as a response reports it: every member read off the wire, with the two tag
/// lists and the source resource flattened into comparable values.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedPool {
    owner_id: Option<String>,
    pool_id: Option<String>,
    pool_arn: Option<String>,
    source_pool_id: Option<String>,
    scope_arn: Option<String>,
    scope_type: Option<String>,
    ipam_arn: Option<String>,
    ipam_region: Option<String>,
    locale: Option<String>,
    pool_depth: Option<String>,
    state: Option<String>,
    state_message: Option<String>,
    description: Option<String>,
    auto_import: Option<String>,
    publicly_advertisable: Option<String>,
    address_family: Option<String>,
    allocation_min_netmask_length: Option<String>,
    allocation_max_netmask_length: Option<String>,
    allocation_default_netmask_length: Option<String>,
    aws_service: Option<String>,
    public_ip_source: Option<String>,
    /// `(key, value)` per entry of `tagSet`, sorted so two reports compare by content.
    tags: Vec<(String, String)>,
    /// The same for `allocationResourceTagSet`.
    allocation_resource_tags: Vec<(String, String)>,
    /// The four members of `sourceResource`, for a pool that reports one.
    source_resource: Option<ReportedSourceResource>,
}

/// The nested `sourceResource` of a pool as a response reports it — "the resource used to
/// provision CIDRs to a resource planning pool" (`IpamPool$SourceResource`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedSourceResource {
    resource_id: Option<String>,
    resource_type: Option<String>,
    resource_region: Option<String>,
    resource_owner: Option<String>,
}

/// The `(key, value)` pairs of the `name` tag set of `node`, sorted.
fn tag_pairs(node: roxmltree::Node<'_, '_>, name: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = set_items(node, name)
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// One `<item>`/`<ipamPool>` element as a [`ReportedPool`].
fn read_pool(node: roxmltree::Node<'_, '_>) -> ReportedPool {
    ReportedPool {
        owner_id: child_text(node, "ownerId"),
        pool_id: child_text(node, "ipamPoolId"),
        pool_arn: child_text(node, "ipamPoolArn"),
        source_pool_id: child_text(node, "sourceIpamPoolId"),
        scope_arn: child_text(node, "ipamScopeArn"),
        scope_type: child_text(node, "ipamScopeType"),
        ipam_arn: child_text(node, "ipamArn"),
        ipam_region: child_text(node, "ipamRegion"),
        locale: child_text(node, "locale"),
        pool_depth: child_text(node, "poolDepth"),
        state: child_text(node, "state"),
        state_message: child_text(node, "stateMessage"),
        description: child_text(node, "description"),
        auto_import: child_text(node, "autoImport"),
        publicly_advertisable: child_text(node, "publiclyAdvertisable"),
        address_family: child_text(node, "addressFamily"),
        allocation_min_netmask_length: child_text(node, "allocationMinNetmaskLength"),
        allocation_max_netmask_length: child_text(node, "allocationMaxNetmaskLength"),
        allocation_default_netmask_length: child_text(node, "allocationDefaultNetmaskLength"),
        aws_service: child_text(node, "awsService"),
        public_ip_source: child_text(node, "publicIpSource"),
        tags: tag_pairs(node, "tagSet"),
        allocation_resource_tags: tag_pairs(node, "allocationResourceTagSet"),
        source_resource: child(node, "sourceResource").map(|resource| ReportedSourceResource {
            resource_id: child_text(resource, "resourceId"),
            resource_type: child_text(resource, "resourceType"),
            resource_region: child_text(resource, "resourceRegion"),
            resource_owner: child_text(resource, "resourceOwner"),
        }),
    }
}

/// The one pool a `CreateIpamPool` response carries.
fn created_pool(case: &str, body: &str) -> ReportedPool {
    let parsed = document(case, body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "ipamPoolId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM pool (no <ipamPoolId>) in {body}"));
    read_pool(node)
}

/// Every pool a `DescribeIpamPools` response reports, in the order it reported them.
fn described_pools(case: &str, body: &str) -> Vec<ReportedPool> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "ipamPoolSet")
    else {
        return Vec::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(read_pool)
        .collect()
}

/// The ids a `DescribeIpamPools` body reports, in the order it reported them.
fn described_ids(case: &str, body: &str) -> Vec<String> {
    described_pools(case, body)
        .into_iter()
        .filter_map(|pool| pool.pool_id)
        .collect()
}

/// `DescribeIpamPools` narrowed to one id, as the one pool it reports.
async fn describe_one(case: &str, router: &MultiServiceRouter, pool_id: &str) -> ReportedPool {
    let body = ok(
        case,
        router,
        "DescribeIpamPools",
        &[param("IpamPoolId.1", pool_id)],
    )
    .await;
    let mut pools = described_pools(case, &body);
    assert_eq!(
        pools.len(),
        1,
        "case {case:?}: expected exactly the pool {pool_id} back, got {body}"
    );
    pools.remove(0)
}

/// The form fields for one `ipam-pool` tag specification over `tags`.
fn tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![param("TagSpecification.1.ResourceType", "ipam-pool")];
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("TagSpecification.1.Tag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// The form fields for the pool's `AllocationResourceTags` over `tags`.
fn allocation_tag_params(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(
            &format!("AllocationResourceTag.{}.Key", index + 1),
            key,
        ));
        params.push(param(
            &format!("AllocationResourceTag.{}.Value", index + 1),
            value,
        ));
    }
    params
}

/// Create one pool through `CreateIpamPool` and answer the id the service minted for it.
async fn create_pool(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let body = ok(case, router, "CreateIpamPool", params).await;
    created_pool(case, &body)
        .pool_id
        .unwrap_or_else(|| panic!("case {case:?}: the created pool reports no id: {body}"))
}

// ---------------------------------------------------------------------------
// What the describe reports
// ---------------------------------------------------------------------------

/// One row of the round-trip table: the case and the request that creates the pool, as fields
/// resolved against the IPAM the suite created.
struct RoundTripCase {
    case: &'static str,
    /// Built per row, because each one names the scope (and sometimes the source pool) by an
    /// id only the running suite knows.
    params: Vec<Param>,
    /// The depth the pool therefore sits at (`IpamPool$PoolDepth`).
    pool_depth: i32,
}

/// The rule: `DescribeIpamPools` reports the pools the store holds with every member as the
/// create stored it — nothing dropped, nothing invented — so a pool read back through the
/// describe equals the one the create answered.
#[tokio::test]
async fn describe_ipam_pools_reports_every_member_the_create_stored() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the pools belong to", &router).await;

    // A parent to create a child pool within, which is what puts a pool at depth 2.
    let parent = create_pool(
        "the parent pool",
        &router,
        &[
            param("IpamScopeId", &ipam.private_scope),
            param("AddressFamily", ADDRESS_FAMILY_IPV4),
            param("Description", "the parent of the nested pool"),
        ],
    )
    .await;

    let mut rows = vec![
        RoundTripCase {
            case: "an ipv4 pool in the private scope with nothing optional",
            params: vec![
                param("IpamScopeId", &ipam.private_scope),
                param("AddressFamily", ADDRESS_FAMILY_IPV4),
            ],
            pool_depth: TOP_LEVEL_IPAM_POOL_DEPTH,
        },
        RoundTripCase {
            case: "an ipv4 pool carrying every optional member of its own",
            params: {
                let mut params = vec![
                    param("IpamScopeId", &ipam.private_scope),
                    param("AddressFamily", ADDRESS_FAMILY_IPV4),
                    param("Description", "the development network"),
                    param("Locale", REGION),
                    param("AutoImport", "true"),
                    param("AwsService", "ec2"),
                    param("AllocationMinNetmaskLength", "16"),
                    param("AllocationMaxNetmaskLength", "28"),
                    param("AllocationDefaultNetmaskLength", "24"),
                ];
                params.extend(tag_params(&[("Name", "dev"), ("Team", "network")]));
                params.extend(allocation_tag_params(&[("Env", "dev"), ("Owner", "ops")]));
                params
            },
            pool_depth: TOP_LEVEL_IPAM_POOL_DEPTH,
        },
        RoundTripCase {
            case: "an ipv6 pool in the public scope advertised through byoip",
            params: vec![
                param("IpamScopeId", &ipam.public_scope),
                param("AddressFamily", ADDRESS_FAMILY_IPV6),
                param("PublicIpSource", IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP),
                param("PubliclyAdvertisable", "true"),
                param("AllocationMinNetmaskLength", "48"),
                param("AllocationMaxNetmaskLength", "64"),
                param("AllocationDefaultNetmaskLength", "56"),
            ],
            pool_depth: TOP_LEVEL_IPAM_POOL_DEPTH,
        },
        RoundTripCase {
            // `IpamPoolPublicIpSource` has a second member (`smithy_query(query="shape",
            // shape="IpamPoolPublicIpSource")` is `amazon | byoip`), which the issue's own
            // cases never name.
            case: "an ipv6 pool in the public scope from Amazon-provided space",
            params: vec![
                param("IpamScopeId", &ipam.public_scope),
                param("AddressFamily", ADDRESS_FAMILY_IPV6),
                param("PublicIpSource", "amazon"),
            ],
            pool_depth: TOP_LEVEL_IPAM_POOL_DEPTH,
        },
        RoundTripCase {
            case: "a pool created within another pool",
            params: vec![
                param("IpamScopeId", &ipam.private_scope),
                param("AddressFamily", ADDRESS_FAMILY_IPV4),
                param("SourceIpamPoolId", &parent),
                param("Description", "the nested pool"),
            ],
            pool_depth: TOP_LEVEL_IPAM_POOL_DEPTH + 1,
        },
        RoundTripCase {
            // "The resource used to provision CIDRs to a resource planning pool"
            // (`IpamPool$SourceResource`), whose `ResourceType` the model gives one member
            // (`smithy_query(query="shape",shape="IpamPoolSourceResourceType")` is `vpc`).
            case: "a resource planning pool with a source resource",
            params: vec![
                param("IpamScopeId", &ipam.private_scope),
                param("AddressFamily", ADDRESS_FAMILY_IPV4),
                param("SourceResource.ResourceId", "vpc-0fe1aa2bb3cc4dd5e"),
                param("SourceResource.ResourceType", "vpc"),
                param("SourceResource.ResourceRegion", "eu-west-2"),
                param("SourceResource.ResourceOwner", engine::emulator_account_id()),
            ],
            pool_depth: TOP_LEVEL_IPAM_POOL_DEPTH,
        },
    ];

    let mut minted: BTreeSet<String> = BTreeSet::from([parent.clone()]);
    for row in &mut rows {
        let case = row.case;
        let body = ok(case, &router, "CreateIpamPool", &row.params).await;
        let answered = created_pool(case, &body);
        let pool_id = answered
            .pool_id
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: the created pool reports no id: {body}"));

        // The facts the service owns, taken from the stored container rather than the request.
        assert_eq!(
            answered.ipam_arn.as_deref(),
            Some(ipam.arn.as_str()),
            "case {case:?}: the pool reports the ARN of the IPAM its scope belongs to, \
             got {answered:?}"
        );
        assert_eq!(
            answered.ipam_region.as_deref(),
            Some(REGION),
            "case {case:?}: the pool reports its IPAM's Region, got {answered:?}"
        );
        assert_eq!(
            answered.pool_depth,
            Some(row.pool_depth.to_string()),
            "case {case:?}: the pool reports its depth in its tree, got {answered:?}"
        );
        assert_eq!(
            answered.owner_id.as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: the pool reports the account that owns it, got {answered:?}"
        );

        // The round trip: every member reads back through the describe, unchanged.
        let read_back = describe_one(case, &router, &pool_id).await;
        assert_eq!(
            read_back, answered,
            "case {case:?}: the describe reports the pool the create answered"
        );
        minted.insert(pool_id);
    }

    // The whole store reports exactly the pools that were created, and nothing else.
    let all = ok("the whole store", &router, "DescribeIpamPools", &[]).await;
    assert_eq!(
        described_ids("the whole store", &all)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        minted,
        "the describe reports every created pool and nothing else, got {all}"
    );
}

/// The rule: `DescribeIpamPools` narrows its report to the ids the request names and to every
/// filter it carries (joined with AND, each filter's own values with OR), and refuses a filter
/// name it does not resolve rather than reporting it as no results.
#[tokio::test]
async fn describe_ipam_pools_narrows_by_ids_and_filters() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the pools belong to", &router).await;

    // Four pools that differ along every kind of member a filter compares: a string, a
    // number, a boolean, a tag, and a member of a nested structure.
    let plain = create_pool(
        "the plain private pool",
        &router,
        &[
            param("IpamScopeId", &ipam.private_scope),
            param("AddressFamily", ADDRESS_FAMILY_IPV4),
            param("Description", "the plain pool"),
        ],
    )
    .await;
    let tagged = create_pool(
        "the tagged private pool",
        &router,
        &{
            let mut params = vec![
                param("IpamScopeId", &ipam.private_scope),
                param("AddressFamily", ADDRESS_FAMILY_IPV4),
                param("Description", "the tagged pool"),
                param("Locale", REGION),
                param("AutoImport", "true"),
                param("AllocationMinNetmaskLength", "20"),
                param("AllocationMaxNetmaskLength", "28"),
            ];
            params.extend(tag_params(&[("Name", "tagged"), ("Env", "prod")]));
            params.extend(allocation_tag_params(&[("Owner", "TeamA")]));
            params
        },
    )
    .await;
    let child_pool = create_pool(
        "the pool within the plain one",
        &router,
        &[
            param("IpamScopeId", &ipam.private_scope),
            param("AddressFamily", ADDRESS_FAMILY_IPV4),
            param("SourceIpamPoolId", &plain),
        ],
    )
    .await;
    let advertised = create_pool(
        "the advertised public pool",
        &router,
        &[
            param("IpamScopeId", &ipam.public_scope),
            param("AddressFamily", ADDRESS_FAMILY_IPV6),
            param("PublicIpSource", IPAM_POOL_PUBLIC_IP_SOURCE_BYOIP),
            param("PubliclyAdvertisable", "true"),
            param("SourceResource.ResourceId", "vpc-07d9e4b1c2a3f5068"),
            param("SourceResource.ResourceType", "vpc"),
            param("SourceResource.ResourceRegion", "ap-south-1"),
        ],
    )
    .await;

    // (case, filter name, filter values, the pools the filter selects)
    let cases: Vec<(&str, &str, Vec<&str>, Vec<String>)> = vec![
        (
            "by the address family a pool holds",
            "address-family",
            vec![ADDRESS_FAMILY_IPV4],
            vec![plain.clone(), tagged.clone(), child_pool.clone()],
        ),
        (
            "by either address family, two values joined with OR",
            "address-family",
            vec![ADDRESS_FAMILY_IPV4, ADDRESS_FAMILY_IPV6],
            vec![
                plain.clone(),
                tagged.clone(),
                child_pool.clone(),
                advertised.clone(),
            ],
        ),
        (
            "by the scope type a pool is in",
            "ipam-scope-type",
            vec!["public"],
            vec![advertised.clone()],
        ),
        (
            "by one id, which the filter names as well as IpamPoolIds does",
            "ipam-pool-id",
            vec![tagged.as_str()],
            vec![tagged.clone()],
        ),
        (
            "by a description with a wildcard",
            "description",
            vec!["the * pool"],
            vec![plain.clone(), tagged.clone()],
        ),
        (
            "by the depth a pool sits at, a number compared as the wire spells it",
            "pool-depth",
            vec!["2"],
            vec![child_pool.clone()],
        ),
        (
            "by a boolean member",
            "auto-import",
            vec!["true"],
            vec![tagged.clone()],
        ),
        (
            "by the other value of that boolean",
            "auto-import",
            vec!["false"],
            vec![plain.clone(), child_pool.clone(), advertised.clone()],
        ),
        (
            "by a netmask bound",
            "allocation-max-netmask-length",
            vec!["28"],
            vec![tagged.clone()],
        ),
        (
            "by the pool a pool was created within",
            "source-ipam-pool-id",
            vec![plain.as_str()],
            vec![child_pool.clone()],
        ),
        (
            "by a tag key",
            "tag-key",
            vec!["Env"],
            vec![tagged.clone()],
        ),
        (
            "by a tag value",
            "tag-value",
            vec!["tagged"],
            vec![tagged.clone()],
        ),
        (
            "by a tag the key names",
            "tag:Name",
            vec!["tagged"],
            vec![tagged.clone()],
        ),
        (
            "by a required allocation resource tag key",
            "allocation-resource-tag-key",
            vec!["Owner"],
            vec![tagged.clone()],
        ),
        (
            "by a required allocation resource tag value",
            "allocation-resource-tag-value",
            vec!["TeamA"],
            vec![tagged.clone()],
        ),
        (
            "by a member of the nested source resource",
            "source-resource-region",
            vec!["ap-south-1"],
            vec![advertised.clone()],
        ),
        (
            "by the locale a pool reports",
            "locale",
            vec![REGION],
            vec![tagged.clone()],
        ),
        (
            "by the IPAM every pool belongs to",
            "ipam-arn",
            vec![ipam.arn.as_str()],
            vec![
                plain.clone(),
                tagged.clone(),
                child_pool.clone(),
                advertised.clone(),
            ],
        ),
        (
            // The pool reports its IPAM and its scope only by ARN, and the id of each is the
            // resource segment of that ARN, which is what a client holds.
            "by the id of the IPAM every pool belongs to",
            "ipam-id",
            vec![ipam.id.as_str()],
            vec![
                plain.clone(),
                tagged.clone(),
                child_pool.clone(),
                advertised.clone(),
            ],
        ),
        (
            "by the id of the scope a pool is in",
            "ipam-scope-id",
            vec![ipam.private_scope.as_str()],
            vec![plain.clone(), tagged.clone(), child_pool.clone()],
        ),
        (
            "by the id of the other scope",
            "ipam-scope-id",
            vec![ipam.public_scope.as_str()],
            vec![advertised.clone()],
        ),
        (
            "by the id of a scope no pool is in",
            "ipam-scope-id",
            vec!["ipam-scope-00000000000000000"],
            Vec::new(),
        ),
        (
            "by a state no pool is in",
            "state",
            vec!["delete-complete"],
            Vec::new(),
        ),
        (
            "by an owner no pool has",
            "owner-id",
            vec!["111122223333"],
            Vec::new(),
        ),
        (
            "by a locale no pool has",
            "locale",
            vec!["eu-central-1"],
            Vec::new(),
        ),
    ];

    for (case, name, values, expected) in &cases {
        let mut params = vec![param("Filter.1.Name", name)];
        for (index, value) in values.iter().enumerate() {
            params.push(param(&format!("Filter.1.Value.{}", index + 1), value));
        }
        let body = ok(case, &router, "DescribeIpamPools", &params).await;
        let found: BTreeSet<String> = described_ids(case, &body).into_iter().collect();
        let wanted: BTreeSet<String> = expected.iter().cloned().collect();
        assert_eq!(
            found, wanted,
            "case {case:?}: expected the filter to select {wanted:?}, got {body}"
        );
    }

    // Two filters hold together with AND.
    let case = "two filters";
    let body = ok(
        case,
        &router,
        "DescribeIpamPools",
        &[
            param("Filter.1.Name", "address-family"),
            param("Filter.1.Value.1", ADDRESS_FAMILY_IPV4),
            param("Filter.2.Name", "tag-key"),
            param("Filter.2.Value.1", "Name"),
        ],
    )
    .await;
    assert_eq!(
        described_ids(case, &body),
        vec![tagged.clone()],
        "case {case:?}: two filters are joined with AND, got {body}"
    );

    // The id list narrows the report as well, however many ids it names, and a filter then
    // narrows that.
    let case = "two ids";
    let body = ok(
        case,
        &router,
        "DescribeIpamPools",
        &[
            param("IpamPoolId.1", &tagged),
            param("IpamPoolId.2", &advertised),
        ],
    )
    .await;
    assert_eq!(
        described_ids(case, &body).into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([tagged.clone(), advertised.clone()]),
        "case {case:?}: the id list narrows the report to the pools it names, got {body}"
    );
    let case = "an id list and a filter that excludes one of them";
    let body = ok(
        case,
        &router,
        "DescribeIpamPools",
        &[
            param("IpamPoolId.1", &tagged),
            param("IpamPoolId.2", &advertised),
            param("Filter.1.Name", "ipam-scope-type"),
            param("Filter.1.Value.1", "private"),
        ],
    )
    .await;
    assert_eq!(
        described_ids(case, &body),
        vec![tagged.clone()],
        "case {case:?}: the ids and the filter both narrow the report, got {body}"
    );

    // Reading changed nothing a later caller sees: the store still reports the same pools.
    let all = ok("the whole store", &router, "DescribeIpamPools", &[]).await;
    assert_eq!(
        described_ids("the whole store", &all)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([plain, tagged, child_pool, advertised]),
        "the reads report the pools that were created and nothing else, got {all}"
    );
}

/// The rule: `DescribeIpamPools` answers at most `MaxResults` pools and a `NextToken` naming
/// where the next page starts, that token continues from the end of the page before it, and
/// "this value is null when there are no more results to return"
/// (`DescribeIpamPoolsResult$NextToken`).
#[tokio::test]
async fn describe_ipam_pools_pages_in_a_stable_order() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the pages come from", &router).await;
    // The model's `IpamMaxResults` has a minimum of 5, so a second page needs more than five
    // pools to exist at all.
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for nth in 0..7 {
        let case = format!("pool {nth}");
        expected.insert(
            create_pool(
                &case,
                &router,
                &[
                    param("IpamScopeId", &ipam.private_scope),
                    param("AddressFamily", ADDRESS_FAMILY_IPV4),
                    param("ClientToken", &format!("page-{nth}")),
                ],
            )
            .await,
        );
    }

    for page_size in [5, 6, 7] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = ok(&case, &router, "DescribeIpamPools", &params).await;
            let page = described_ids(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size} pools, got {body}"
            );
            seen.extend(page);
            token = element(&body, "nextToken");
            pages += 1;
            assert!(
                pages <= 5,
                "case {case:?}: the paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "case {case:?}: no pool is reported twice across the pages, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            expected,
            "case {case:?}: the pages together report every stored pool, got {seen:?}"
        );
        assert_eq!(
            seen,
            sorted,
            "case {case:?}: the pages are reported in a stable id order, got {seen:?}"
        );
    }

    // A page the store can serve whole carries no token at all.
    let case = "one page for the whole store";
    let body = ok(
        case,
        &router,
        "DescribeIpamPools",
        &[param("MaxResults", "1000")],
    )
    .await;
    assert_eq!(
        element(&body, "nextToken"),
        None,
        "case {case:?}: no token once there are no more results to return, got {body}"
    );
}

/// The rule: a request `DescribeIpamPools`'s contract does not admit is refused with the code
/// that names what was wrong — this resource's `Malformed` code for a value that is not an
/// IPAM pool id at all, its `NotFound` code for a well-formed id the account holds no pool
/// for, `InvalidParameterValue` for a filter name the operation does not resolve or a page
/// `InvalidPaginationToken` for a page token it did not mint, and `DryRunOperation` for a
/// permission check, which reads nothing.
#[tokio::test]
async fn describe_ipam_pools_refuses_what_the_contract_does_not_admit() {
    let (router, _engine) = emulator();
    let ipam = create_ipam("the IPAM the refusals are aimed at", &router).await;
    let pool = create_pool(
        "the one pool the store holds",
        &router,
        &[
            param("IpamScopeId", &ipam.private_scope),
            param("AddressFamily", ADDRESS_FAMILY_IPV4),
        ],
    )
    .await;

    // (case, params, expected status, expected code). A refused request is a 400 except for
    // the permission check, which EC2 answers 412 `DryRunOperation`.
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "an id that is not an IPAM pool id at all",
            vec![param("IpamPoolId.1", "not-an-ipam-pool-id")],
            400,
            INVALID_IPAM_POOL_ID_MALFORMED,
        ),
        (
            "an id of another resource of the same family",
            vec![param("IpamPoolId.1", "ipam-scope-0123456789abcdef0")],
            400,
            INVALID_IPAM_POOL_ID_MALFORMED,
        ),
        (
            "a malformed id beside a well-formed one",
            vec![
                param("IpamPoolId.1", &pool),
                param("IpamPoolId.2", "ipam-pool-not-hexadecimal"),
            ],
            400,
            INVALID_IPAM_POOL_ID_MALFORMED,
        ),
        (
            "a well-formed id the account holds no pool for",
            vec![param("IpamPoolId.1", "ipam-pool-00000000000000000")],
            400,
            INVALID_IPAM_POOL_ID_NOT_FOUND,
        ),
        (
            "an unknown id beside a known one",
            vec![
                param("IpamPoolId.1", &pool),
                param("IpamPoolId.2", "ipam-pool-00000000000000000"),
            ],
            400,
            INVALID_IPAM_POOL_ID_NOT_FOUND,
        ),
        (
            "a filter name the operation does not resolve",
            vec![
                param("Filter.1.Name", "pool-colour"),
                param("Filter.1.Value.1", "blue"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
        (
            // "Filter names are case-sensitive" (`Filter$Name`).
            "a filter name spelled in another case",
            vec![
                param("Filter.1.Name", "Address-Family"),
                param("Filter.1.Value.1", ADDRESS_FAMILY_IPV4),
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
                param("IpamPoolId.1", "not-an-ipam-pool-id"),
            ],
            400,
            INVALID_IPAM_POOL_ID_MALFORMED,
        ),
        (
            // …and so is a filter name, so a dry run that would have been refused for one is
            // refused with that reason instead of reporting that it would have succeeded.
            "a permission check beside an unresolvable filter",
            vec![
                param("DryRun", "true"),
                param("Filter.1.Name", "pool-colour"),
                param("Filter.1.Value.1", "blue"),
            ],
            400,
            INVALID_PARAMETER_VALUE,
        ),
    ];

    for (case, params, status, code) in &cases {
        let (answered, body) = invoke(&router, "DescribeIpamPools", params).await;
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

    // None of the refusals changed what the store reports.
    let all = ok("the whole store", &router, "DescribeIpamPools", &[]).await;
    assert_eq!(
        described_ids("the whole store", &all),
        vec![pool],
        "a refused describe changes nothing, got {all}"
    );
}

/// The rule: a describe is a REPORT — it applies the state move the clock already owed the
/// pool and nothing else, so a caller reading straight back still sees the transient state and
/// one reading after the due instant sees the settled one.
#[tokio::test]
async fn describe_ipam_pools_reports_the_state_the_clock_owes() {
    let case = "the clock completes the pool";
    let (router, _engine) = emulator();
    let ipam = create_ipam(case, &router).await;
    let pool = create_pool(
        case,
        &router,
        &[
            param("IpamScopeId", &ipam.private_scope),
            param("AddressFamily", ADDRESS_FAMILY_IPV4),
        ],
    )
    .await;

    // Straight back, before the due instant: still being created. Reading it twice is still
    // the transient state — a read does not move it.
    for attempt in 0..2 {
        assert_eq!(
            describe_one(case, &router, &pool).await.state.as_deref(),
            Some(IPAM_POOL_STATE_CREATE_IN_PROGRESS),
            "case {case:?}: read {attempt} before the due instant sees the transient state"
        );
    }

    tokio::time::sleep(past_the_transition()).await;
    assert_eq!(
        describe_one(case, &router, &pool).await.state.as_deref(),
        Some(IPAM_POOL_STATE_CREATE_COMPLETE),
        "case {case:?}: the pool completes once the transition is due"
    );
}
