// SPDX-License-Identifier: CC0-1.0

//! The ceilings `DescribeSecurityGroups` puts on the filtering work one request can ask
//! for, driven over the ec2Query wire through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rule under test: matching cost is the PRODUCT of how many filters a request carries,
//! how many values each carries, how long a value is and how many groups the store holds,
//! and the model bounds none of them (`FilterList` and `Filter$Values` carry no `@length`).
//! So the handler applies this crate's own ceilings — the same ones `DescribeInstances`
//! enforces — and a request AT each ceiling is answered while one PAST it is refused with
//! `InvalidParameterValue`. Each row is a distinct factor of that product, named by the
//! constant it is derived from rather than by a literal.
//!
//! The behaviour of the describe itself — what it selects and reports — lives in
//! [`super::describe_security_groups`].

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_FILTER_VALUE_BYTES, MAX_FILTER_VALUES,
    MAX_FILTERS, MAX_RESOURCE_IDS, SecurityGroupSpec,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{elements, error_code, filters, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// One `DescribeSecurityGroups` call carrying `params`, as its HTTP status and XML body.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "DescribeSecurityGroups", params).await
}

/// The group ids a response reports: read off each group's own `securityGroupArn`, since a
/// `groupId` element also appears inside a rule that references another group.
fn reported_ids(xml: &str) -> Vec<String> {
    elements(xml, "securityGroupArn")
        .iter()
        .filter_map(|arn| arn.rsplit('/').next().map(str::to_string))
        .collect()
}

/// A store holding a handful of groups, so a filter has something to compare against, and
/// the ids they were given in store order.
fn seed(engine: &engine::InMemoryEngine) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let vpc_id = guard
        .vpcs
        .values()
        .find(|vpc| vpc.is_default == Some(true))
        .and_then(|vpc| vpc.vpc_id.clone())
        .expect("a fresh account has its default VPC");
    for name in ["alpha", "beta", "gamma"] {
        guard.insert_security_group(SecurityGroupSpec {
            vpc_id: vpc_id.clone(),
            group_name: name.to_string(),
            description: format!("{name} tier"),
            region: REGION.to_string(),
            tags: Vec::new(),
            ip_permissions: Vec::new(),
            ip_permissions_egress: Vec::new(),
        });
    }
    guard.security_groups.keys().cloned().collect()
}

/// A request at each published ceiling is answered: the ceilings bound the work, they do not
/// refuse a request that merely reaches them.
#[tokio::test]
async fn a_request_at_each_filtering_ceiling_is_answered() {
    let (router, engine) = super::ec2_query::emulator();
    let stored = seed(&engine);
    let known = stored[0].clone();

    // `MAX_FILTERS` filters, each selecting everything, so the AND of them all is the whole
    // store: the ceiling itself is accepted and nothing is lost.
    let at_filter_ceiling: Vec<(&str, &[&str])> = (0..MAX_FILTERS)
        .map(|_| ("group-name", &["*"][..]))
        .collect();
    let reported = {
        let (status, body) = describe(&router, &filters(&at_filter_ceiling)).await;
        assert_eq!(
            status, 200,
            "case \"MAX_FILTERS filters\": expected HTTP 200, got {status} with body {body}"
        );
        reported_ids(&body)
    };
    assert_eq!(
        reported, stored,
        "case \"MAX_FILTERS filters\": expected every stored group, got {reported:?}"
    );

    // One filter carrying `MAX_FILTER_VALUES` values, a value of exactly
    // `MAX_FILTER_VALUE_BYTES` bytes, and `MAX_RESOURCE_IDS` ids: one row per factor of the
    // product.
    let many_values: Vec<String> = (0..MAX_FILTER_VALUES).map(|n| format!("group-{n}")).collect();
    let many_values: Vec<&str> = many_values.iter().map(String::as_str).collect();
    let long_value = "a".repeat(MAX_FILTER_VALUE_BYTES);
    let many_ids = vec![known.as_str(); MAX_RESOURCE_IDS];
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "MAX_FILTER_VALUES values in one filter",
            filters(&[("group-name", &many_values)]),
        ),
        (
            "a value of MAX_FILTER_VALUE_BYTES bytes",
            filters(&[("group-name", &[long_value.as_str()])]),
        ),
        ("MAX_RESOURCE_IDS ids", indexed("GroupId", &many_ids)),
    ];
    for (case, params) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 at the ceiling, got {status} with body {body}"
        );
    }
}

/// One step past any ceiling is refused with `InvalidParameterValue`, and reports no group:
/// the request asked for more work than one call will do, and is told so rather than served
/// a partial answer.
#[tokio::test]
async fn a_request_one_step_past_a_filtering_ceiling_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let stored = seed(&engine);
    let known = stored[0].clone();

    let past_filter_ceiling: Vec<(&str, &[&str])> = (0..=MAX_FILTERS)
        .map(|_| ("group-name", &["*"][..]))
        .collect();
    let past_value_ceiling: Vec<String> = (0..=MAX_FILTER_VALUES)
        .map(|n| format!("group-{n}"))
        .collect();
    let past_value_ceiling: Vec<&str> = past_value_ceiling.iter().map(String::as_str).collect();
    let past_byte_ceiling = "a".repeat(MAX_FILTER_VALUE_BYTES + 1);
    let past_id_ceiling = vec![known.as_str(); MAX_RESOURCE_IDS + 1];
    let past_name_ceiling = vec!["default"; MAX_RESOURCE_IDS + 1];

    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "one filter past MAX_FILTERS",
            filters(&past_filter_ceiling),
        ),
        (
            "one value past MAX_FILTER_VALUES",
            filters(&[("group-name", &past_value_ceiling)]),
        ),
        (
            "one byte past MAX_FILTER_VALUE_BYTES",
            filters(&[("group-name", &[past_byte_ceiling.as_str()])]),
        ),
        (
            "one id past MAX_RESOURCE_IDS",
            indexed("GroupId", &past_id_ceiling),
        ),
        (
            "one name past MAX_RESOURCE_IDS",
            indexed("GroupName", &past_name_ceiling),
        ),
    ];
    for (case, params) in cases {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            reported_ids(&body).is_empty(),
            "case {case:?}: a refused request reports no group, got {body}"
        );
    }
}

/// A VPC created through `CreateVpc` comes with its own default security group — "when you
/// create a VPC, it comes with a default security group" — so the describe reports one more
/// group than before, in the new VPC, named `default`.
#[tokio::test]
async fn a_vpc_created_over_the_wire_brings_its_own_default_group() {
    let (router, engine) = super::ec2_query::emulator();
    let before = seed(&engine);

    // Rows: two VPCs, each of which must bring exactly one group of its own.
    for cidr_block in ["10.9.0.0/16", "192.168.64.0/20"] {
        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "CreateVpc",
            &[("CidrBlock".to_string(), cidr_block.to_string())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {cidr_block:?}: expected HTTP 200 from CreateVpc, got {status} with body {body}"
        );
        let vpc_id = super::ec2_query::element(&body, "vpcId")
            .unwrap_or_else(|| panic!("case {cidr_block:?}: expected a vpcId in {body}"));

        let (status, body) = describe(&router, &filters(&[("vpc-id", &[vpc_id.as_str()])])).await;
        assert_eq!(
            status, 200,
            "case {cidr_block:?}: expected HTTP 200, got {status} with body {body}"
        );
        let in_new_vpc = reported_ids(&body);
        assert_eq!(
            in_new_vpc.len(),
            1,
            "case {cidr_block:?}: expected exactly the new VPC's default group, got {in_new_vpc:?}"
        );
        assert!(
            !before.contains(&in_new_vpc[0]),
            "case {cidr_block:?}: expected a group minted with the VPC, got {in_new_vpc:?}"
        );
        assert!(
            body.contains("<groupName>default</groupName>"),
            "case {cidr_block:?}: expected the group AWS names `default`, got {body}"
        );
    }
}
